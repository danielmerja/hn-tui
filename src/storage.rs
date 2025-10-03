use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row};

#[derive(Debug, Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
pub struct Account {
    pub id: i64,
    pub reddit_id: String,
    pub username: String,
    pub display_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub account_id: i64,
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub scope: Vec<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct MediaEntry {
    pub id: i64,
    pub url: String,
    pub media_type: String,
    pub file_path: String,
    pub width: i64,
    pub height: i64,
    pub size_bytes: i64,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub checksum: String,
}

#[derive(Debug, Default, Clone)]
pub struct Options {
    pub path: Option<PathBuf>,
}

impl Store {
    pub fn open(opts: Options) -> Result<Self> {
        let path = if let Some(path) = opts.path {
            path
        } else {
            default_path().context("storage: resolve default path")?
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("storage: create directory {}", parent.display()))?;
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("storage: open database at {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", &"WAL")
            .context("storage: set WAL")?;
        conn.pragma_update(None, "foreign_keys", &"ON")
            .context("storage: enable foreign keys")?;
        conn.pragma_update(None, "busy_timeout", &5000)
            .context("storage: set busy timeout")?;
        migrate(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn close(self) -> Result<()> {
        let conn = Arc::try_unwrap(self.conn)
            .map_err(|_| anyhow!("storage: connection still in use"))?
            .into_inner();
        conn.close()
            .map_err(|(_, err)| err)
            .context("storage: close connection")
    }

    pub fn upsert_account(&self, mut account: Account) -> Result<i64> {
        if account.reddit_id.is_empty() {
            bail!("storage: reddit id required");
        }
        let now = Utc::now();
        if account.created_at.timestamp() == 0 {
            account.created_at = now;
        }
        account.updated_at = now;

        let conn = self.conn.lock();
        let id: i64 = conn.query_row(
            r#"
INSERT INTO accounts (reddit_id, username, display_name, created_at, updated_at)
VALUES (?1, ?2, ?3, ?4, ?5)
ON CONFLICT(reddit_id) DO UPDATE SET
  username = excluded.username,
  display_name = excluded.display_name,
  updated_at = excluded.updated_at
RETURNING id
"#,
            params![
                account.reddit_id,
                account.username,
                account.display_name,
                account.created_at.timestamp(),
                account.updated_at.timestamp(),
            ],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn get_account_by_reddit_id(&self, reddit_id: &str) -> Result<Option<Account>> {
        let conn = self.conn.lock();
        conn.query_row(
            r#"
SELECT id, reddit_id, username, display_name, created_at, updated_at
FROM accounts
WHERE reddit_id = ?1
"#,
            params![reddit_id],
            account_from_row,
        )
        .optional()
        .context("storage: query account by reddit id")
    }

    pub fn get_account_by_id(&self, id: i64) -> Result<Option<Account>> {
        let conn = self.conn.lock();
        conn.query_row(
            r#"
SELECT id, reddit_id, username, display_name, created_at, updated_at
FROM accounts
WHERE id = ?1
"#,
            params![id],
            account_from_row,
        )
        .optional()
        .context("storage: query account by id")
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
SELECT id, reddit_id, username, display_name, created_at, updated_at
FROM accounts
ORDER BY updated_at DESC
"#,
        )?;
        let rows = stmt
            .query_map([], account_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn upsert_token(&self, token: Token) -> Result<()> {
        if token.account_id == 0 {
            bail!("storage: account id required for token");
        }
        let scope = token.scope.join(" ");
        let conn = self.conn.lock();
        conn.execute(
            r#"
INSERT INTO tokens (account_id, access_token, refresh_token, token_type, scope, expires_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6)
ON CONFLICT(account_id) DO UPDATE SET
  access_token = excluded.access_token,
  refresh_token = excluded.refresh_token,
  token_type = excluded.token_type,
  scope = excluded.scope,
  expires_at = excluded.expires_at
"#,
            params![
                token.account_id,
                token.access_token,
                token.refresh_token,
                token.token_type,
                scope,
                token.expires_at.timestamp(),
            ],
        )?;
        Ok(())
    }

    pub fn get_token(&self, account_id: i64) -> Result<Option<Token>> {
        let conn = self.conn.lock();
        conn.query_row(
            r#"
SELECT account_id, access_token, refresh_token, token_type, scope, expires_at
FROM tokens
WHERE account_id = ?1
"#,
            params![account_id],
            |row| {
                let expires: i64 = row.get(5)?;
                let scope: String = row.get(4)?;
                Ok(Token {
                    account_id: row.get(0)?,
                    access_token: row.get(1)?,
                    refresh_token: row.get(2)?,
                    token_type: row.get(3)?,
                    scope: if scope.is_empty() {
                        Vec::new()
                    } else {
                        scope.split(' ').map(|s| s.to_owned()).collect()
                    },
                    expires_at: Utc
                        .timestamp_opt(expires, 0)
                        .single()
                        .unwrap_or_else(Utc::now),
                })
            },
        )
        .optional()
        .context("storage: query token")
    }

    pub fn upsert_media_entry(&self, mut entry: MediaEntry) -> Result<i64> {
        if entry.url.is_empty() {
            bail!("storage: media url required");
        }
        if entry.fetched_at.timestamp() == 0 {
            entry.fetched_at = Utc::now();
        }
        let expires = entry.expires_at.map(|dt| dt.timestamp());
        let conn = self.conn.lock();
        let id: i64 = conn.query_row(
            r#"
INSERT INTO media_cache (url, media_type, file_path, width, height, size_bytes, fetched_at, expires_at, checksum)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
ON CONFLICT(url) DO UPDATE SET
  media_type = excluded.media_type,
  file_path = excluded.file_path,
  width = excluded.width,
  height = excluded.height,
  size_bytes = excluded.size_bytes,
  fetched_at = excluded.fetched_at,
  expires_at = excluded.expires_at,
  checksum = excluded.checksum
RETURNING id
"#,
            params![
                entry.url,
                entry.media_type,
                entry.file_path,
                entry.width,
                entry.height,
                entry.size_bytes,
                entry.fetched_at.timestamp(),
                expires,
                entry.checksum,
            ],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn get_media_entry_by_url(&self, url: &str) -> Result<Option<MediaEntry>> {
        let conn = self.conn.lock();
        conn.query_row(
            r#"
SELECT id, url, media_type, file_path, width, height, size_bytes, fetched_at, expires_at, checksum
FROM media_cache
WHERE url = ?1
"#,
            params![url],
            media_entry_from_row,
        )
        .optional()
        .context("storage: query media entry")
    }

    pub fn total_media_size(&self) -> Result<i64> {
        let conn = self.conn.lock();
        let total: Option<i64> = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM media_cache",
            [],
            |row| row.get(0),
        )?;
        Ok(total.unwrap_or(0))
    }

    pub fn list_expired_media(
        &self,
        cutoff: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<MediaEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
SELECT id, url, media_type, file_path, width, height, size_bytes, fetched_at, expires_at, checksum
FROM media_cache
WHERE expires_at IS NOT NULL AND expires_at <= ?1
ORDER BY expires_at ASC
LIMIT ?2
"#,
        )?;
        let rows = stmt
            .query_map(
                params![cutoff.timestamp(), limit as i64],
                media_entry_from_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn list_oldest_media(&self, limit: usize) -> Result<Vec<MediaEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"
SELECT id, url, media_type, file_path, width, height, size_bytes, fetched_at, expires_at, checksum
FROM media_cache
ORDER BY fetched_at ASC
LIMIT ?1
"#,
        )?;
        let rows = stmt
            .query_map(params![limit as i64], media_entry_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn delete_media_entries(&self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!(
            "DELETE FROM media_cache WHERE id IN ({})",
            placeholders
        ))?;
        let params_vec = ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect::<Vec<_>>();
        stmt.execute(rusqlite::params_from_iter(params_vec))?;
        Ok(())
    }
}

fn account_from_row(row: &Row<'_>) -> rusqlite::Result<Account> {
    let created: i64 = row.get(4)?;
    let updated: i64 = row.get(5)?;
    Ok(Account {
        id: row.get(0)?,
        reddit_id: row.get(1)?,
        username: row.get(2)?,
        display_name: row.get(3)?,
        created_at: Utc
            .timestamp_opt(created, 0)
            .single()
            .unwrap_or_else(Utc::now),
        updated_at: Utc
            .timestamp_opt(updated, 0)
            .single()
            .unwrap_or_else(Utc::now),
    })
}

fn media_entry_from_row(row: &Row<'_>) -> rusqlite::Result<MediaEntry> {
    let fetched: i64 = row.get(7)?;
    let expires: Option<i64> = row.get(8)?;
    Ok(MediaEntry {
        id: row.get(0)?,
        url: row.get(1)?,
        media_type: row.get(2)?,
        file_path: row.get(3)?,
        width: row.get(4)?,
        height: row.get(5)?,
        size_bytes: row.get(6)?,
        fetched_at: Utc
            .timestamp_opt(fetched, 0)
            .single()
            .unwrap_or_else(Utc::now),
        expires_at: expires.and_then(|ts| Utc.timestamp_opt(ts, 0).single()),
        checksum: row.get(9)?,
    })
}

fn migrate(conn: &Connection) -> Result<()> {
    conn.execute(
        r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL
)
"#,
        [],
    )?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let migrations = migrations();
    for (idx, sql) in migrations.iter().enumerate() {
        let version = (idx + 1) as i64;
        if version <= current {
            continue;
        }
        conn.execute_batch(sql)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![
                version,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or(Duration::from_secs(0))
                    .as_secs() as i64,
            ],
        )?;
    }
    Ok(())
}

fn migrations() -> Vec<&'static str> {
    vec![
        r#"
CREATE TABLE IF NOT EXISTS accounts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  reddit_id TEXT NOT NULL UNIQUE,
  username TEXT NOT NULL,
  display_name TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tokens (
  account_id INTEGER PRIMARY KEY,
  access_token TEXT NOT NULL,
  refresh_token TEXT NOT NULL,
  token_type TEXT NOT NULL,
  scope TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  FOREIGN KEY(account_id) REFERENCES accounts(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS media_cache (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  url TEXT NOT NULL UNIQUE,
  media_type TEXT NOT NULL,
  file_path TEXT NOT NULL,
  width INTEGER,
  height INTEGER,
  size_bytes INTEGER,
  fetched_at INTEGER NOT NULL,
  expires_at INTEGER,
  checksum TEXT
);

CREATE INDEX IF NOT EXISTS idx_media_cache_fetched_at ON media_cache(fetched_at);
CREATE INDEX IF NOT EXISTS idx_media_cache_expires_at ON media_cache(expires_at);
"#,
    ]
}

pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("reddix").join("state.db"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_in_memory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.db");
        let store = Store::open(Options {
            path: Some(path.clone()),
        })
        .unwrap();
        assert!(path.exists());
        store.close().unwrap();
    }
}
