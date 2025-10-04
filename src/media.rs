use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use crossbeam_channel::{unbounded, Receiver, Sender};
use image::ImageFormat;
use parking_lot::Mutex;
use reqwest::blocking::Client;
use sha1::{Digest, Sha1};

use crate::storage::{self, MediaEntry};

#[derive(Debug, Clone)]
pub struct Config {
    pub cache_dir: Option<PathBuf>,
    pub max_size_bytes: i64,
    pub default_ttl: Duration,
    pub workers: usize,
    pub http_client: Option<Client>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cache_dir: None,
            max_size_bytes: 500 * 1024 * 1024,
            default_ttl: Duration::from_secs(6 * 60 * 60),
            workers: 2,
            http_client: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    pub url: String,
    pub media_type: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub ttl: Option<Duration>,
    pub force: bool,
}

#[derive(Debug)]
pub struct ResultEntry {
    pub entry: Option<MediaEntry>,
    pub error: Option<anyhow::Error>,
}

struct Job {
    request: Request,
    tx: Sender<ResultEntry>,
}

struct Inner {
    store: Arc<storage::Store>,
    cfg: Config,
    client: Client,
    jobs: Sender<Job>,
    stop: Sender<()>,
    pruning: Mutex<()>,
}

pub struct Manager {
    inner: Arc<Inner>,
    handles: Vec<thread::JoinHandle<()>>,
}

impl Manager {
    pub fn new(store: Arc<storage::Store>, cfg: Config) -> Result<Self> {
        let mut cfg = cfg;
        if cfg.workers == 0 {
            cfg.workers = 2;
        }
        let cache_dir = cfg
            .cache_dir
            .clone()
            .or_else(default_cache_dir)
            .context("media: cache dir not configured")?;
        fs::create_dir_all(&cache_dir)?;
        cfg.cache_dir = Some(cache_dir);

        let client = if let Some(client) = cfg.http_client.clone() {
            client
        } else {
            Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .context("media: build http client")?
        };

        let (job_tx, job_rx) = unbounded();
        let (stop_tx, stop_rx) = unbounded();

        let inner = Arc::new(Inner {
            store,
            cfg,
            client,
            jobs: job_tx,
            stop: stop_tx,
            pruning: Mutex::new(()),
        });

        let mut handles = Vec::new();
        for _ in 0..inner.cfg.workers {
            let rx_jobs = job_rx.clone();
            let rx_stop = stop_rx.clone();
            let worker_inner = inner.clone();
            handles.push(thread::spawn(move || worker_inner.worker(rx_jobs, rx_stop)));
        }

        Ok(Self { inner, handles })
    }

    pub fn enqueue(&self, request: Request) -> Receiver<ResultEntry> {
        let (tx, rx) = unbounded();
        let job = Job { request, tx };
        let _ = self.inner.jobs.send(job);
        rx
    }

    fn shutdown(&mut self) {
        for _ in &self.handles {
            let _ = self.inner.stop.send(());
        }
        while let Some(handle) = self.handles.pop() {
            let _ = handle.join();
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Inner {
    fn worker(&self, jobs: Receiver<Job>, stop: Receiver<()>) {
        loop {
            crossbeam_channel::select! {
                recv(stop) -> _ => break,
                recv(jobs) -> msg => {
                    match msg {
                        Ok(job) => self.process(job),
                        Err(_) => break,
                    }
                }
            }
        }
    }

    fn process(&self, job: Job) {
        let result = match self.fetch(job.request) {
            Ok(entry) => ResultEntry {
                entry: Some(entry),
                error: None,
            },
            Err(err) => ResultEntry {
                entry: None,
                error: Some(err),
            },
        };
        let _ = job.tx.send(result);
    }

    fn fetch(&self, request: Request) -> Result<MediaEntry> {
        if request.url.is_empty() {
            return Err(anyhow!("media: url required"));
        }

        if let Some(entry) = self.store.get_media_entry_by_url(&request.url)? {
            if !request.force
                && self.is_fresh(&entry, request.ttl)
                && Path::new(&entry.file_path).exists()
            {
                return Ok(entry);
            }
        }

        let response = self
            .client
            .get(&request.url)
            .send()
            .context("media: download")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("media: request failed: {} - {}", status, body));
        }

        let headers = response.headers().clone();
        let bytes = response.bytes().context("media: body")?.to_vec();
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|val| val.to_str().ok())
            .map(|s| s.to_string())
            .or(request.media_type.clone())
            .unwrap_or_else(|| detect_mime(&bytes));

        let file_path = self.write_file(&bytes)?;
        let checksum = sha1_hex(&bytes);
        let width = request.width.unwrap_or_default();
        let height = request.height.unwrap_or_default();
        let ttl = request.ttl.unwrap_or(self.cfg.default_ttl);
        let expires_at = SystemTime::now().checked_add(ttl);

        let media_entry = MediaEntry {
            id: 0,
            url: request.url.clone(),
            media_type: content_type,
            file_path,
            width,
            height,
            size_bytes: bytes.len() as i64,
            fetched_at: Utc::now(),
            expires_at: expires_at.map(DateTime::<Utc>::from),
            checksum,
        };

        self.prune_if_needed(media_entry.size_bytes)?;
        let id = self.store.upsert_media_entry(media_entry.clone())?;
        Ok(MediaEntry { id, ..media_entry })
    }

    fn is_fresh(&self, entry: &MediaEntry, ttl: Option<Duration>) -> bool {
        let ttl = ttl.unwrap_or(self.cfg.default_ttl);
        if ttl.is_zero() {
            return false;
        }
        let expiry = entry.fetched_at.checked_add_signed(
            chrono::Duration::from_std(ttl).unwrap_or_else(|_| chrono::Duration::seconds(0)),
        );
        match expiry {
            Some(expiry) => Utc::now() < expiry,
            None => false,
        }
    }

    fn write_file(&self, data: &[u8]) -> Result<String> {
        let cache_dir = self.cfg.cache_dir.as_ref().expect("cache dir");
        let filename = format!("{}.bin", sha1_hex(data));
        let path = cache_dir.join(filename);
        fs::write(&path, data).context("media: write")?;
        Ok(path.to_string_lossy().to_string())
    }

    fn prune_if_needed(&self, new_bytes: i64) -> Result<()> {
        let _guard = self.pruning.lock();
        let mut total = self.store.total_media_size()? + new_bytes;
        if total <= self.cfg.max_size_bytes {
            return Ok(());
        }

        let mut ids = Vec::new();
        let mut paths = Vec::new();

        for entry in self.store.list_oldest_media(100)? {
            total -= entry.size_bytes;
            ids.push(entry.id);
            paths.push(entry.file_path);
            if total <= self.cfg.max_size_bytes {
                break;
            }
        }

        self.store.delete_media_entries(&ids)?;
        for path in paths {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }
}

fn default_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("reddix"))
}

fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn detect_mime(bytes: &[u8]) -> String {
    match image::guess_format(bytes) {
        Ok(ImageFormat::Jpeg) => "image/jpeg".into(),
        Ok(ImageFormat::Png) => "image/png".into(),
        Ok(ImageFormat::Gif) => "image/gif".into(),
        Ok(ImageFormat::WebP) => "image/webp".into(),
        _ => {
            let mut buffer = [0u8; 512];
            let mut cursor = std::io::Cursor::new(bytes);
            let read = cursor.read(&mut buffer).unwrap_or(0);
            tree_magic_mini::from_u8(&buffer[..read]).to_string()
        }
    }
}
