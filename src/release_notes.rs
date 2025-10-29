use once_cell::sync::Lazy;
use semver::Version;
use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct ReleaseNote {
    pub version: Version,
    pub title: String,
    pub banner: String,
    pub summary: String,
    pub details: Vec<String>,
    pub release_url: String,
}

#[derive(Deserialize)]
struct FileNotes {
    #[serde(default)]
    release: Vec<RawRelease>,
}

#[derive(Deserialize)]
struct RawRelease {
    version: String,
    title: String,
    banner: String,
    summary: String,
    #[serde(default)]
    details: Vec<String>,
    #[serde(default)]
    url: Option<String>,
}

static NOTES: Lazy<Vec<ReleaseNote>> = Lazy::new(load_release_notes);

fn load_release_notes() -> Vec<ReleaseNote> {
    const RAW: &str = include_str!("../docs/release-notes.yaml");
    let parsed: FileNotes = match serde_yaml::from_str(RAW) {
        Ok(file) => file,
        Err(err) => {
            eprintln!("Failed to parse release notes file: {err}");
            return Vec::new();
        }
    };

    parsed
        .release
        .into_iter()
        .filter_map(|raw| {
            let version = match Version::parse(raw.version.trim()) {
                Ok(v) => v,
                Err(err) => {
                    eprintln!(
                        "Skipping release note with invalid version '{}': {err}",
                        raw.version
                    );
                    return None;
                }
            };
            let url = raw
                .url
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| {
                    format!(
                        "https://github.com/ck-zhang/reddix/releases/tag/v{}",
                        version
                    )
                });
            Some(ReleaseNote {
                version,
                title: raw.title.trim().to_string(),
                banner: raw.banner.trim().to_string(),
                summary: raw.summary.trim().to_string(),
                details: raw
                    .details
                    .into_iter()
                    .map(|line| line.trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect(),
                release_url: url,
            })
        })
        .collect()
}

pub fn latest_for(version: &Version) -> Option<ReleaseNote> {
    NOTES
        .iter()
        .filter(|note| &note.version <= version)
        .max_by(|a, b| a.version.cmp(&b.version))
        .cloned()
}

pub fn by_version(version: &Version) -> Option<ReleaseNote> {
    NOTES.iter().find(|note| &note.version == version).cloned()
}
