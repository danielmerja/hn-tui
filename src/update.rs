use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use semver::Version;
use serde::Deserialize;

const RELEASES_URL: &str = "https://api.github.com/repos/ck-zhang/reddix/releases/latest";

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
