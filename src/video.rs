use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use once_cell::sync::OnceCell;
use serde_json::json;

#[cfg(any(unix, target_os = "windows"))]
use rand::{distributions::Alphanumeric, Rng};
#[cfg(unix)]
use std::os::unix::net::UnixStream;

use crate::reddit::{self, PostMedia, RedditVideo};

fn video_debug_enabled() -> bool {
    static FLAG: OnceCell<bool> = OnceCell::new();
    *FLAG.get_or_init(|| {
        std::env::var("HN_TUI_DEBUG_VIDEO")
            .map(|val| {
                let trimmed = val.trim();
                !(trimmed.is_empty()
                    || trimmed.eq_ignore_ascii_case("0")
                    || trimmed.eq_ignore_ascii_case("false")
                    || trimmed.eq_ignore_ascii_case("no")
                    || trimmed.eq_ignore_ascii_case("off"))
            })
            .unwrap_or(false)
    })
}

fn video_debug_writer() -> Option<&'static Mutex<std::fs::File>> {
    static WRITER: OnceCell<Option<Mutex<std::fs::File>>> = OnceCell::new();
    WRITER
        .get_or_init(|| {
            std::env::var("HN_TUI_DEBUG_VIDEO_LOG")
                .ok()
                .and_then(|path| {
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map(Mutex::new)
                        .ok()
                })
        })
        .as_ref()
}

pub fn debug_log(message: impl AsRef<str>) {
    if !video_debug_enabled() {
        return;
    }
    if let Some(writer) = video_debug_writer() {
        if let Ok(mut file) = writer.lock() {
            let _ = writeln!(file, "{}", message.as_ref());
            return;
        }
    }
    eprintln!("{}", message.as_ref());
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoSource {
    pub playback_url: String,
    pub label: String,
    pub is_gif: bool,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

impl VideoSource {
    fn from_reddit_video(video: &RedditVideo, label: Cow<'_, str>) -> Option<Self> {
        if video.transcoding_status.eq_ignore_ascii_case("error") {
            return None;
        }

        let playback_url = sanitize_url(if !video.fallback_url.trim().is_empty() {
            &video.fallback_url
        } else if !video.hls_url.trim().is_empty() {
            &video.hls_url
        } else if !video.dash_url.trim().is_empty() {
            &video.dash_url
        } else if !video.scrubber_media_url.trim().is_empty() {
            &video.scrubber_media_url
        } else {
            return None;
        });

        if playback_url.is_empty() {
            return None;
        }

        Some(Self {
            playback_url,
            label: label.into_owned(),
            is_gif: video.is_gif,
            width: some_positive(video.width),
            height: some_positive(video.height),
        })
    }
}

fn some_positive(value: i64) -> Option<i64> {
    if value > 0 {
        Some(value)
    } else {
        None
    }
}

fn sanitize_url(raw: &str) -> String {
    raw.trim().replace("&amp;", "&")
}

fn push_http_headers(args: &mut Vec<String>) {
    let ua = std::env::var("REDDIX_MPV_USER_AGENT").unwrap_or_else(|_| {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
        (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36"
            .to_string()
    });
    args.push(format!("--http-header-fields=User-Agent: {}", ua));
    if let Ok(referer) = std::env::var("REDDIX_MPV_REFERER") {
        if !referer.trim().is_empty() {
            args.push(format!("--http-header-fields=Referer: {}", referer.trim()));
        }
    } else {
        args.push("--http-header-fields=Referer: https://www.reddit.com/".to_string());
    }
}

pub fn find_video_source(post: &reddit::Post) -> Option<VideoSource> {
    video_from_media(post.secure_media.as_ref(), &post.title)
        .or_else(|| video_from_media(post.media.as_ref(), &post.title))
        .or_else(|| {
            post.crosspost_parent_list.iter().find_map(|parent| {
                video_from_media(parent.secure_media.as_ref(), &post.title)
                    .or_else(|| video_from_media(parent.media.as_ref(), &post.title))
            })
        })
}

fn video_from_media(media: Option<&PostMedia>, title: &str) -> Option<VideoSource> {
    let media = media?;
    let video = media.reddit_video.as_ref()?;
    let label = if title.trim().is_empty() {
        Cow::Borrowed("Reddit video")
    } else {
        Cow::Owned(title.trim().to_string())
    };
    VideoSource::from_reddit_video(video, label)
}

pub struct InlineLaunchOptions<'a> {
    pub mpv_path: &'a str,
    pub source: &'a VideoSource,
    pub playback: Cow<'a, str>,
    pub cols: i32,
    pub rows: i32,
    pub col: u16,
    pub row: u16,
    pub term_cols: i32,
    pub term_rows: i32,
    pub pixel_width: i32,
    pub pixel_height: i32,
}

pub struct ExternalLaunchOptions<'a> {
    pub mpv_path: &'a str,
    pub source: &'a VideoSource,
    pub playback: &'a str,
    pub fullscreen: bool,
}

pub struct InlineSession {
    kill_tx: Sender<()>,
    status_rx: Receiver<Result<ExitStatus>>,
    handle: Option<thread::JoinHandle<()>>,
    ipc_path: Option<Arc<String>>,
}

impl InlineSession {
    fn finalize(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    pub fn try_status(&mut self) -> Option<Result<ExitStatus>> {
        match self.status_rx.try_recv() {
            Ok(res) => {
                self.finalize();
                Some(res)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.finalize();
                Some(Err(anyhow!("video session closed unexpectedly")))
            }
        }
    }

    pub fn stop_blocking(mut self) -> Option<Result<ExitStatus>> {
        let _ = self.kill_tx.send(());
        let res = self.status_rx.recv().ok();
        self.finalize();
        res
    }

    pub fn controls_supported(&self) -> bool {
        self.ipc_path.is_some()
    }

    pub fn send_command(&self, command: VideoCommand) -> Result<()> {
        let Some(path) = &self.ipc_path else {
            return Err(anyhow!(
                "Inline video controls are not supported on this platform."
            ));
        };
        send_ipc_command(path, command)
    }
}

impl Drop for InlineSession {
    fn drop(&mut self) {
        if self.handle.is_some() {
            let _ = self.kill_tx.send(());
            let _ = self.status_rx.recv().ok();
            self.finalize();
        }
    }
}

pub fn spawn_inline_player(opts: InlineLaunchOptions<'_>) -> Result<InlineSession> {
    if opts.source.playback_url.trim().is_empty() {
        return Err(anyhow!("video URL missing"));
    }

    let (kill_tx, kill_rx) = bounded::<()>(1);
    let (status_tx, status_rx) = bounded::<Result<ExitStatus>>(1);

    let mpv_path = opts.mpv_path.to_string();
    let playback_target = opts.playback.into_owned();
    let remote_url = opts.source.playback_url.clone();
    let label = opts.source.label.clone();
    let debug_enabled = video_debug_enabled();
    #[cfg(unix)]
    let ipc_path = unique_ipc_path();
    #[cfg(not(unix))]
    let ipc_path: Option<String> = None;
    let ipc_path_for_session = ipc_path.clone();
    debug_log(format!(
        "spawning inline mpv rows={} cols={} term={}x{} pixels={}x{} url={} playback={} ipc={}",
        opts.rows,
        opts.cols,
        opts.term_cols,
        opts.term_rows,
        opts.pixel_width,
        opts.pixel_height,
        remote_url,
        playback_target,
        ipc_path.as_deref().unwrap_or("n/a")
    ));
    #[cfg(unix)]
    if let Some(path) = &ipc_path {
        if let Err(err) = fs::remove_file(path) {
            if err.kind() != std::io::ErrorKind::NotFound && video_debug_enabled() {
                debug_log(format!("failed to remove stale mpv ipc path {path}: {err}"));
            }
        }
    }
    let ipc_arg = ipc_path
        .as_ref()
        .map(|path| format!("--input-ipc-server={path}"));
    let handle = thread::spawn(move || {
        let ipc_cleanup = ipc_path.clone();
        let result = (|| -> Result<ExitStatus> {
            let mut args = Vec::new();
            args.push(playback_target.clone());
            args.push("--vo=kitty".to_string());
            args.push(format!("--vo-kitty-cols={}", opts.term_cols.max(1)));
            args.push(format!("--vo-kitty-rows={}", opts.term_rows.max(1)));
            let left = u32::from(opts.col).saturating_add(1);
            let top = u32::from(opts.row).saturating_add(1);
            args.push(format!("--vo-kitty-left={}", left));
            args.push(format!("--vo-kitty-top={}", top));
            let pixel_width = opts.pixel_width.max(1);
            let pixel_height = opts.pixel_height.max(1);
            args.push(format!("--vo-kitty-width={}", pixel_width));
            args.push(format!("--vo-kitty-height={}", pixel_height));
            args.push("--vo-kitty-config-clear=no".to_string());
            args.push("--force-window=no".to_string());
            args.push("--keep-open=no".to_string());
            args.push("--loop-file=inf".to_string());
            args.push("--really-quiet".to_string());
            args.push("--idle=no".to_string());
            args.push("--terminal=no".to_string());
            args.push("--input-terminal=no".to_string());
            args.push("--no-config".to_string());
            args.push("--ytdl=no".to_string());
            args.push("--osc=no".to_string());
            args.push("--osd-level=0".to_string());
            args.push("--osd-duration=0".to_string());
            if let Some(arg) = &ipc_arg {
                args.push(arg.clone());
            }

            if !label.is_empty() {
                args.push(format!("--force-media-title={}", label));
            }

            push_http_headers(&mut args);

            if debug_enabled {
                debug_log(format!("mpv args: {:?}", args));
            }

            let mut command = Command::new(&mpv_path);
            for arg in &args {
                command.arg(arg);
            }

            command.stdin(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::io::{AsRawFd, FromRawFd};

                let stdout = std::io::stdout();
                let fd = stdout.as_raw_fd();
                let dup_fd = unsafe { libc::dup(fd) };
                if dup_fd >= 0 {
                    let stdio = unsafe { Stdio::from_raw_fd(dup_fd) };
                    command.stdout(stdio);
                } else {
                    command.stdout(Stdio::inherit());
                }
            }
            #[cfg(not(unix))]
            {
                command.stdout(Stdio::inherit());
            }
            if debug_enabled {
                command.stderr(Stdio::piped());
            } else {
                command.stderr(Stdio::null());
            }

            let mut child = command
                .spawn()
                .with_context(|| format!("launch mpv to play {}", remote_url))?;
            let mut stderr_handle = None;
            if debug_enabled {
                if let Some(stderr) = child.stderr.take() {
                    stderr_handle = Some(thread::spawn(move || {
                        let reader = BufReader::new(stderr);
                        for line in reader.lines().map_while(Result::ok) {
                            debug_log(format!("mpv stderr: {}", line));
                        }
                    }));
                }
            }

            loop {
                if kill_rx.try_recv().is_ok() {
                    let _ = child.kill();
                    let status = child.wait().context("wait for mpv after stop request")?;
                    if debug_enabled {
                        debug_log(format!("mpv stopped with status {:?}", status.code()));
                    }
                    if let Some(handle) = stderr_handle.take() {
                        let _ = handle.join();
                    }
                    return Ok(status);
                }

                match child.try_wait() {
                    Ok(Some(status)) => {
                        if debug_enabled {
                            debug_log(format!("mpv exited with status {:?}", status.code()));
                        }
                        if let Some(handle) = stderr_handle.take() {
                            let _ = handle.join();
                        }
                        return Ok(status);
                    }
                    Ok(None) => thread::sleep(Duration::from_millis(30)),
                    Err(err) => {
                        if debug_enabled {
                            debug_log(format!("mpv poll error: {}", err));
                        }
                        if let Some(handle) = stderr_handle.take() {
                            let _ = handle.join();
                        }
                        return Err(anyhow!(err)).context("poll mpv status");
                    }
                }
            }
        })();
        #[cfg(unix)]
        if let Some(path) = ipc_cleanup {
            cleanup_ipc_path(&path);
        }

        let _ = status_tx.send(result);
    });

    Ok(InlineSession {
        kill_tx,
        status_rx,
        handle: Some(handle),
        ipc_path: ipc_path_for_session.map(Arc::new),
    })
}

pub fn spawn_external_player(opts: ExternalLaunchOptions<'_>) -> Result<()> {
    if opts.playback.trim().is_empty() {
        return Err(anyhow!("video playback path missing"));
    }

    let mut args = Vec::new();
    args.push(opts.playback.to_string());
    if opts.fullscreen {
        args.push("--fullscreen".to_string());
    }
    args.push("--force-window=yes".to_string());
    args.push("--keep-open=no".to_string());
    args.push("--loop-file=inf".to_string());
    args.push("--really-quiet".to_string());
    args.push("--no-config".to_string());
    args.push("--ytdl=no".to_string());

    push_http_headers(&mut args);

    if !opts.source.label.is_empty() {
        args.push(format!("--force-media-title={}", opts.source.label));
    }

    let mut command = Command::new(opts.mpv_path);
    for arg in &args {
        command.arg(arg);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    command
        .spawn()
        .with_context(|| format!("launch mpv fullscreen for {}", opts.playback))?;
    Ok(())
}

#[derive(Clone, Copy)]
pub enum VideoCommand {
    TogglePause,
    SeekRelative(f64),
}

fn send_ipc_command(path: &str, command: VideoCommand) -> Result<()> {
    let payload = json!({
        "command": command_payload(command),
    });
    let serialized = serde_json::to_string(&payload).context("serialize mpv command")?;
    send_ipc_command_inner(path, &serialized)
}

#[cfg(unix)]
fn send_ipc_command_inner(path: &str, serialized: &str) -> Result<()> {
    let mut stream =
        UnixStream::connect(path).with_context(|| format!("connect to mpv IPC socket {path}"))?;
    stream
        .write_all(serialized.as_bytes())
        .context("write mpv IPC command")?;
    stream
        .write_all(b"\n")
        .context("write mpv IPC command terminator")?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn send_ipc_command_inner(path: &str, serialized: &str) -> Result<()> {
    use std::fs::OpenOptions;
    use std::io::ErrorKind;

    const PIPE_RETRIES: usize = 5;
    const PIPE_RETRY_DELAY: Duration = Duration::from_millis(100);

    for attempt in 0..PIPE_RETRIES {
        match OpenOptions::new().read(true).write(true).open(path) {
            Ok(mut pipe) => {
                pipe.write_all(serialized.as_bytes())
                    .with_context(|| format!("write mpv IPC command to {path}"))?;
                pipe.write_all(b"\n")
                    .with_context(|| format!("write mpv IPC command terminator to {path}"))?;
                pipe.flush().ok();
                return Ok(());
            }
            Err(err) if err.kind() == ErrorKind::NotFound && attempt + 1 < PIPE_RETRIES => {
                thread::sleep(PIPE_RETRY_DELAY);
            }
            Err(err) => {
                return Err(anyhow!(err)).context(format!("connect to mpv IPC named pipe {path}"));
            }
        }
    }

    Err(anyhow!("connect to mpv IPC named pipe {}", path))
}

#[cfg(all(not(unix), not(target_os = "windows")))]
fn send_ipc_command_inner(_path: &str, _serialized: &str) -> Result<()> {
    Err(anyhow!(
        "Inline video controls are not supported on this platform."
    ))
}

#[cfg(unix)]
fn unique_ipc_path() -> Option<String> {
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(10)
        .map(char::from)
        .collect();
    let mut path = std::env::temp_dir();
    path.push(format!("reddix-mpv-{}-{suffix}.sock", std::process::id()));
    Some(path.to_string_lossy().to_string())
}

#[cfg(target_os = "windows")]
fn unique_ipc_path() -> Option<String> {
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(10)
        .map(char::from)
        .collect();
    Some(format!(
        r"\\.\pipe\reddix-mpv-{}-{suffix}",
        std::process::id()
    ))
}

#[cfg(all(not(unix), not(target_os = "windows")))]
fn unique_ipc_path() -> Option<String> {
    None
}

#[cfg(unix)]
fn cleanup_ipc_path(path: &str) {
    if let Err(err) = fs::remove_file(path) {
        if err.kind() != std::io::ErrorKind::NotFound && video_debug_enabled() {
            debug_log(format!("failed to remove mpv ipc path {path}: {err}"));
        }
    }
}

#[cfg(target_os = "windows")]
fn cleanup_ipc_path(_path: &str) {}

#[cfg(all(not(unix), not(target_os = "windows")))]
fn cleanup_ipc_path(_path: &str) {}

fn command_payload(command: VideoCommand) -> serde_json::Value {
    match command {
        VideoCommand::TogglePause => json!(["cycle", "pause"]),
        VideoCommand::SeekRelative(offset) => json!(["seek", offset, "relative"]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_fallback_url_when_available() {
        let video = RedditVideo {
            hls_url: "https://stream.test/hls.m3u8".into(),
            dash_url: "https://stream.test/dash.mpd".into(),
            fallback_url: "https://stream.test/fallback.mp4".into(),
            ..RedditVideo::default()
        };
        let source = VideoSource::from_reddit_video(&video, Cow::Borrowed("Sample title")).unwrap();
        assert_eq!(source.playback_url, "https://stream.test/fallback.mp4");
    }

    #[test]
    fn sanitizes_encoded_urls() {
        let video = RedditVideo {
            fallback_url: "https://stream.test/video.mp4?token=a&amp;b=1".into(),
            ..RedditVideo::default()
        };
        let source = VideoSource::from_reddit_video(&video, Cow::Borrowed("Sample title")).unwrap();
        assert_eq!(
            source.playback_url,
            "https://stream.test/video.mp4?token=a&b=1"
        );
    }

    #[test]
    fn prefers_hls_when_fallback_missing() {
        let video = RedditVideo {
            hls_url: "https://stream.test/hls.m3u8".into(),
            dash_url: "https://stream.test/dash.mpd".into(),
            ..RedditVideo::default()
        };
        let source = VideoSource::from_reddit_video(&video, Cow::Borrowed("Sample title")).unwrap();
        assert_eq!(source.playback_url, "https://stream.test/hls.m3u8");
    }

    #[test]
    fn falls_back_to_dash_then_scrubber() {
        let video = RedditVideo {
            dash_url: "https://stream.test/dash.mpd".into(),
            fallback_url: "https://stream.test/fallback.mp4".into(),
            ..RedditVideo::default()
        };
        let source = VideoSource::from_reddit_video(&video, Cow::Borrowed("Sample title")).unwrap();
        assert_eq!(source.playback_url, "https://stream.test/fallback.mp4");

        let video = RedditVideo {
            scrubber_media_url: "https://stream.test/scrubber.mp4".into(),
            ..RedditVideo::default()
        };
        let source = VideoSource::from_reddit_video(&video, Cow::Borrowed("Sample title")).unwrap();
        assert_eq!(source.playback_url, "https://stream.test/scrubber.mp4");

        let video = RedditVideo {
            dash_url: "https://stream.test/dash.mpd".into(),
            ..RedditVideo::default()
        };
        let source = VideoSource::from_reddit_video(&video, Cow::Borrowed("Sample title")).unwrap();
        assert_eq!(source.playback_url, "https://stream.test/dash.mpd");

        let video = RedditVideo {
            ..RedditVideo::default()
        };
        assert!(VideoSource::from_reddit_video(&video, Cow::Borrowed("Sample title")).is_none());
    }
}
