use anyhow::{anyhow, Context, Result};
use reqwest::header::{ACCEPT, USER_AGENT};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app_settings::{AppSettings, CachedUpdateRelease};

pub const PROJECT_GITHUB_URL: &str = "https://github.com/kanice888-max/LanBridge";
const RELEASES_API_URL: &str =
    "https://api.github.com/repos/kanice888-max/LanBridge/releases?per_page=100";
const UPDATE_CHECK_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateCheckStatus {
    NotChecked,
    UpdateAvailable,
    UpToDate,
    NoRelease,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdateRelease {
    pub version: String,
    pub tag_name: String,
    pub name: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub status: UpdateCheckStatus,
    pub release: Option<UpdateRelease>,
    pub checked_at_unix_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    draft: bool,
    tag_name: String,
    name: Option<String>,
    published_at: Option<String>,
}

#[derive(Debug)]
struct ParsedRelease {
    release: UpdateRelease,
    version: Version,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn is_check_due(settings: &AppSettings, now_unix_ms: i64) -> bool {
    settings
        .last_update_check_unix_ms
        .map(|last| now_unix_ms.saturating_sub(last) >= UPDATE_CHECK_INTERVAL_MS)
        .unwrap_or(true)
}

pub fn cached_result(settings: &AppSettings) -> Result<UpdateCheckResult> {
    let current = Version::parse(current_version()).context("invalid current app version")?;
    let checked_at_unix_ms = settings.last_update_check_unix_ms;
    let release = settings.latest_release.as_ref().map(to_update_release);
    let status = match (checked_at_unix_ms, release.as_ref()) {
        (None, _) => UpdateCheckStatus::NotChecked,
        (Some(_), None) => UpdateCheckStatus::NoRelease,
        (Some(_), Some(release)) => {
            let latest =
                Version::parse(&release.version).context("invalid cached release version")?;
            if latest > current {
                UpdateCheckStatus::UpdateAvailable
            } else {
                UpdateCheckStatus::UpToDate
            }
        }
    };

    Ok(UpdateCheckResult {
        current_version: current_version().to_string(),
        status,
        release,
        checked_at_unix_ms,
    })
}

pub async fn fetch_latest_release() -> Result<Option<UpdateRelease>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .context("failed to create update client")?;
    let releases = client
        .get(RELEASES_API_URL)
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, format!("LanBridge/{}", current_version()))
        .send()
        .await
        .context("failed to request GitHub releases")?
        .error_for_status()
        .context("GitHub releases request failed")?
        .json::<Vec<GithubRelease>>()
        .await
        .context("failed to parse GitHub releases")?;

    Ok(select_latest_release(releases))
}

pub fn result_from_release(
    latest_release: Option<UpdateRelease>,
    checked_at_unix_ms: i64,
) -> Result<UpdateCheckResult> {
    let current = Version::parse(current_version()).context("invalid current app version")?;
    let status = match latest_release.as_ref() {
        None => UpdateCheckStatus::NoRelease,
        Some(release) if Version::parse(&release.version)? > current => {
            UpdateCheckStatus::UpdateAvailable
        }
        Some(_) => UpdateCheckStatus::UpToDate,
    };

    Ok(UpdateCheckResult {
        current_version: current_version().to_string(),
        status,
        release: latest_release,
        checked_at_unix_ms: Some(checked_at_unix_ms),
    })
}

pub fn now_unix_ms() -> Result<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow!("system clock is before Unix epoch"))?
        .as_millis() as i64)
}

pub fn cache_release(release: Option<&UpdateRelease>) -> Option<CachedUpdateRelease> {
    release.map(|release| CachedUpdateRelease {
        version: release.version.clone(),
        tag_name: release.tag_name.clone(),
        name: release.name.clone(),
        published_at: release.published_at.clone(),
    })
}

pub fn release_page_url(tag_name: &str) -> Result<String> {
    if tag_name.is_empty()
        || !tag_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+')
        })
    {
        return Err(anyhow!("invalid release tag"));
    }
    Ok(format!("{PROJECT_GITHUB_URL}/releases/tag/{tag_name}"))
}

fn to_update_release(release: &CachedUpdateRelease) -> UpdateRelease {
    UpdateRelease {
        version: release.version.clone(),
        tag_name: release.tag_name.clone(),
        name: release.name.clone(),
        published_at: release.published_at.clone(),
    }
}

fn select_latest_release(releases: Vec<GithubRelease>) -> Option<UpdateRelease> {
    releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| parse_release(release).ok())
        .max_by(|left, right| left.version.cmp(&right.version))
        .map(|release| release.release)
}

fn parse_release(release: GithubRelease) -> Result<ParsedRelease> {
    let version_text = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    let version = Version::parse(version_text)
        .with_context(|| format!("release tag is not semver: {}", release.tag_name))?;
    Ok(ParsedRelease {
        release: UpdateRelease {
            version: version.to_string(),
            tag_name: release.tag_name,
            name: release.name,
            published_at: release.published_at,
        },
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag_name: &str, draft: bool) -> GithubRelease {
        GithubRelease {
            draft,
            tag_name: tag_name.to_string(),
            name: None,
            published_at: None,
        }
    }

    #[test]
    fn selects_highest_semver_and_includes_prereleases() {
        let latest = select_latest_release(vec![
            release("v0.1.11", false),
            release("v0.2.0-beta.1", false),
            release("v0.2.0-alpha.1", false),
        ])
        .unwrap();

        assert_eq!(latest.tag_name, "v0.2.0-beta.1");
        assert_eq!(latest.version, "0.2.0-beta.1");
    }

    #[test]
    fn ignores_drafts_and_invalid_tags() {
        let latest = select_latest_release(vec![
            release("not-a-version", false),
            release("v9.0.0", true),
            release("v0.2.0", false),
        ])
        .unwrap();

        assert_eq!(latest.tag_name, "v0.2.0");
    }

    #[test]
    fn compares_latest_release_to_current_version() {
        let result = result_from_release(
            Some(UpdateRelease {
                version: "0.2.1".to_string(),
                tag_name: "v0.2.1".to_string(),
                name: None,
                published_at: None,
            }),
            1,
        )
        .unwrap();

        assert_eq!(result.status, UpdateCheckStatus::UpdateAvailable);
    }

    #[test]
    fn caches_no_release_as_a_successful_check() {
        let result = result_from_release(None, 1).unwrap();
        assert_eq!(result.status, UpdateCheckStatus::NoRelease);
        assert_eq!(result.checked_at_unix_ms, Some(1));
    }

    #[test]
    fn only_allows_safe_release_page_tags() {
        assert!(release_page_url("v0.2.0-beta.1").is_ok());
        assert!(release_page_url("v0.2.0/other").is_err());
    }
}
