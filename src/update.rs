use std::{env, time::Duration};

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use semver::Version;
use serde::Deserialize;

const RELEASES_URL: &str = "https://api.github.com/repos/ck-zhang/reddix/releases/latest";
const FORCE_VERSION_ENV: &str = "REDDIX_FORCE_UPDATE_VERSION";
const FORCE_URL_ENV: &str = "REDDIX_FORCE_UPDATE_URL";

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: Version,
    pub url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    draft: bool,
    prerelease: bool,
}

pub fn check_for_update(current: &Version) -> Result<Option<UpdateInfo>> {
    if let Some(update) = forced_update(current)? {
        return Ok(Some(update));
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(format!(
            "reddix/{version} (update-check)",
            version = crate::VERSION
        ))
        .build()
        .context("build update HTTP client")?;

    let response = client
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("request latest release metadata")?;

    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if response.status() == StatusCode::FORBIDDEN {
        bail!("rate limited by GitHub while checking for updates");
    }

    if !response.status().is_success() {
        bail!("update check failed with status {}", response.status());
    }

    let release: Release = response
        .json()
        .context("decode release response from GitHub")?;

    if release.draft || release.prerelease {
        return Ok(None);
    }

    let tag = release.tag_name.trim();
    let normalized = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    let version = Version::parse(normalized)
        .with_context(|| format!("parse release tag {tag:?} as semantic version"))?;

    if &version > current {
        Ok(Some(UpdateInfo {
            version,
            url: release.html_url,
        }))
    } else {
        Ok(None)
    }
}

fn forced_update(current: &Version) -> Result<Option<UpdateInfo>> {
    let forced_version = match env::var(FORCE_VERSION_ENV) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(err) => {
            bail!("read {FORCE_VERSION_ENV}: {err}");
        }
    };

    let trimmed = forced_version.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let version = Version::parse(trimmed)
        .with_context(|| format!("parse {FORCE_VERSION_ENV}={trimmed:?} as semantic version"))?;

    if &version <= current {
        return Ok(None);
    }

    let url = env::var(FORCE_URL_ENV)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("https://github.com/ck-zhang/reddix/releases/tag/v{version}"));

    Ok(Some(UpdateInfo { version, url }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn guard_env() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn forced_update_returns_info_when_version_is_newer() {
        let _guard = guard_env();
        env::set_var(FORCE_VERSION_ENV, "0.2.0");
        env::set_var(FORCE_URL_ENV, "https://example.com/releases/0.2.0");
        let current = Version::parse("0.1.0").unwrap();
        let info = forced_update(&current).unwrap().unwrap();
        assert_eq!(info.version, Version::parse("0.2.0").unwrap());
        assert_eq!(info.url, "https://example.com/releases/0.2.0");
        env::remove_var(FORCE_VERSION_ENV);
        env::remove_var(FORCE_URL_ENV);
    }

    #[test]
    fn forced_update_is_ignored_when_version_not_newer() {
        let _guard = guard_env();
        env::set_var(FORCE_VERSION_ENV, "0.1.0");
        let current = Version::parse("0.1.0").unwrap();
        assert!(forced_update(&current).unwrap().is_none());
        env::remove_var(FORCE_VERSION_ENV);
    }
}
