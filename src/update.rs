use std::env;
#[cfg(unix)]
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use semver::Version;
use serde::Deserialize;
use tempfile::Builder as TempFileBuilder;

#[cfg(target_os = "windows")]
const INSTALLER_NAME: &str = "hn-tui-installer.ps1";
#[cfg(not(target_os = "windows"))]
const INSTALLER_NAME: &str = "hn-tui-installer.sh";

pub const SKIP_UPDATE_ENV: &str = "HN_TUI_SKIP_UPDATE_CHECK";

const RELEASES_URL: &str = "https://api.github.com/repos/danielmerja/hn-tui/releases/latest";
const FORCE_VERSION_ENV: &str = "HN_TUI_FORCE_UPDATE_VERSION";
const FORCE_URL_ENV: &str = "HN_TUI_FORCE_UPDATE_URL";

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: Version,
    pub release_url: String,
    pub tag: String,
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
            "hn-tui/{version} (update-check)",
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
            release_url: release.html_url,
            tag: tag.to_string(),
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

    let release_url = env::var(FORCE_URL_ENV)
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("https://github.com/danielmerja/hn-tui/releases/tag/v{version}"));
    let tag = format!("v{version}");

    Ok(Some(UpdateInfo {
        version,
        release_url,
        tag,
    }))
}

impl UpdateInfo {
    pub fn assets_base_url(&self) -> String {
        release_download_base(&self.release_url, &self.tag)
    }

    pub fn installer_url(&self) -> String {
        format!("{}/{}", self.assets_base_url(), INSTALLER_NAME)
    }
}

fn release_download_base(release_url: &str, tag: &str) -> String {
    if let Some(idx) = release_url.find("/releases/tag/") {
        let prefix = &release_url[..idx];
        format!("{}/releases/download/{}", prefix.trim_end_matches('/'), tag)
    } else {
        format!(
            "https://github.com/danielmerja/hn-tui/releases/download/{}",
            tag
        )
    }
}

pub fn install_update(info: &UpdateInfo) -> Result<()> {
    let installer_url = info.installer_url();

    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(format!(
            "hn-tui/{version} (update-install)",
            version = crate::VERSION
        ))
        .build()
        .context("build HTTP client for installer download")?;

    let mut response = client
        .get(&installer_url)
        .send()
        .with_context(|| format!("download installer from {installer_url}"))?
        .error_for_status()
        .with_context(|| format!("installer request returned error for {installer_url}"))?;

    let suffix = if cfg!(target_os = "windows") {
        ".ps1"
    } else {
        ".sh"
    };
    let mut temp = TempFileBuilder::new()
        .prefix("hn-tui-installer-")
        .suffix(suffix)
        .tempfile()
        .context("create temporary file for installer")?;

    response
        .copy_to(temp.as_file_mut())
        .context("write installer to temporary file")?;
    temp.as_file_mut()
        .flush()
        .context("flush installer to disk")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
            .context("mark installer as executable")?;
    }

    let temp_path: PathBuf = temp.path().to_path_buf();
    run_installer(&temp_path)?;
    temp.close().context("remove temporary installer file")?;
    Ok(())
}

fn run_installer(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(path)
            .output()
            .context("run hn-tui PowerShell installer")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "installer exited with status {}: {}",
                output.status,
                stderr.trim()
            );
        }
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("sh")
            .arg(path)
            .output()
            .context("run hn-tui installer script")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "installer exited with status {}: {}",
                output.status,
                stderr.trim()
            );
        }
        Ok(())
    }
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
        env::set_var(FORCE_VERSION_ENV, "0.2.1");
        env::set_var(FORCE_URL_ENV, "https://example.com/releases/0.2.1");
        let current = Version::parse("0.1.0").unwrap();
        let info = forced_update(&current).unwrap().unwrap();
        assert_eq!(info.version, Version::parse("0.2.1").unwrap());
        assert_eq!(info.release_url, "https://example.com/releases/0.2.1");
        assert_eq!(info.tag, "v0.2.1");
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
