use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::eligibility::release_channel;
use super::release::{FetchResult, ReleaseChannel, ensure_published, fetch_latest};
use super::{CheckOutcome, Release, current_version};

const CACHE_SCHEMA: u32 = 3;
const SUCCESS_INTERVAL: u64 = 24 * 60 * 60;
const DISMISS_INTERVAL: u64 = 7 * 24 * 60 * 60;
const FAILURE_BACKOFF: [u64; 4] = [15 * 60, 60 * 60, 6 * 60 * 60, 24 * 60 * 60];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UpdateCache {
    #[serde(default)]
    schema: u32,
    etag: Option<String>,
    release: Option<Release>,
    #[serde(default)]
    unavailable: bool,
    last_success_at: Option<u64>,
    last_attempt_at: Option<u64>,
    next_attempt_after: Option<u64>,
    #[serde(default)]
    failure_count: u32,
    #[serde(default)]
    dismissed_version: Option<semver::Version>,
    #[serde(default)]
    dismissed_until: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateDismissal {
    version: semver::Version,
    until: u64,
}

impl UpdateDismissal {
    pub(crate) fn applies_to(&self, version: &semver::Version) -> bool {
        self.applies_to_at(version, now_secs())
    }

    fn applies_to_at(&self, version: &semver::Version, now: u64) -> bool {
        self.version == *version && self.until > now
    }

    #[cfg(test)]
    pub(crate) fn for_test(version: semver::Version, until: u64) -> Self {
        Self { version, until }
    }
}

pub(crate) fn background_check_due() -> bool {
    let cache = load_cache();
    check_due_at(&cache, now_secs())
}

pub(crate) fn cached_update() -> Option<Release> {
    let cache = load_cache();
    cache
        .release
        .filter(|release| release.version > current_version())
}

#[cfg(not(test))]
pub(crate) fn cached_dismissal() -> Option<UpdateDismissal> {
    let cache = load_cache();
    let dismissal = UpdateDismissal {
        version: cache.dismissed_version?,
        until: cache.dismissed_until?,
    };
    (dismissal.until > now_secs()).then_some(dismissal)
}

pub(crate) fn dismiss_update(version: semver::Version) -> UpdateDismissal {
    let mut cache = load_cache();
    let dismissal = dismissal_at(version, now_secs());
    cache.dismissed_version = Some(dismissal.version.clone());
    cache.dismissed_until = Some(dismissal.until);
    save_cache(&cache);
    dismissal
}

fn dismissal_at(version: semver::Version, now: u64) -> UpdateDismissal {
    UpdateDismissal {
        version,
        until: now.saturating_add(DISMISS_INTERVAL),
    }
}

pub(crate) async fn check_for_update(
    client: &reqwest::Client,
    force: bool,
) -> Result<CheckOutcome> {
    ensure_published(release_channel())?;
    let mut cache = load_cache();
    let now = now_secs();
    if !force && !check_due_at(&cache, now) {
        return outcome_from_cache(&cache, true);
    }

    cache.last_attempt_at = Some(now);
    save_cache(&cache);
    let etag = cache.release.as_ref().and(cache.etag.as_deref());
    match fetch_latest(client, etag, release_channel()).await {
        Ok(fetched) => {
            apply_fetched(&mut cache, fetched);
            cache.last_success_at = Some(now);
            cache.next_attempt_after = Some(now.saturating_add(SUCCESS_INTERVAL));
            cache.failure_count = 0;
            save_cache(&cache);
            outcome_from_cache(&cache, false)
        }
        Err(error) => {
            cache.failure_count = cache.failure_count.saturating_add(1);
            cache.next_attempt_after = Some(now.saturating_add(failure_delay(cache.failure_count)));
            save_cache(&cache);
            Err(error)
        }
    }
}

fn apply_fetched(cache: &mut UpdateCache, fetched: FetchResult) {
    cache.etag = match fetched {
        FetchResult::Unavailable => {
            cache.release = None;
            cache.unavailable = true;
            None
        }
        FetchResult::NotModified { etag } => etag,
        FetchResult::Release { release, etag } => {
            cache.release = Some(release);
            cache.unavailable = false;
            etag
        }
    };
}

fn outcome_from_cache(cache: &UpdateCache, cached: bool) -> Result<CheckOutcome> {
    if cache.unavailable {
        return Ok(CheckOutcome::Current {
            version: current_version(),
            cached,
        });
    }
    let release = cache
        .release
        .clone()
        .context("no cached release information is available")?;
    if release.version > current_version() {
        Ok(CheckOutcome::Available { release, cached })
    } else {
        Ok(CheckOutcome::Current {
            version: release.version,
            cached,
        })
    }
}

fn failure_delay(failure_count: u32) -> u64 {
    let index = failure_count
        .saturating_sub(1)
        .min((FAILURE_BACKOFF.len() - 1) as u32) as usize;
    FAILURE_BACKOFF[index]
}

fn check_due_at(cache: &UpdateCache, now: u64) -> bool {
    !matches!(
        cache.next_attempt_after,
        Some(next) if next >= now && next.saturating_sub(now) <= SUCCESS_INTERVAL
    )
}

/// Each channel caches its own release, since each selects different archives.
fn cache_path() -> Option<PathBuf> {
    cache_base(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        dirs::home_dir().as_deref(),
    )
    .map(|base| base.join("aven").join(cache_file(release_channel())))
}

fn cache_file(channel: ReleaseChannel) -> &'static str {
    match channel {
        ReleaseChannel::Terminal => "update.json",
        ReleaseChannel::Desktop => "update-desktop.json",
    }
}

fn cache_base(xdg_cache_home: Option<&std::ffi::OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    xdg_cache_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|path| path.join(".cache")))
}

fn load_cache() -> UpdateCache {
    let Some(path) = cache_path() else {
        return UpdateCache::default();
    };
    let Ok(contents) = std::fs::read_to_string(path) else {
        return UpdateCache::default();
    };
    let Ok(cache) = serde_json::from_str::<UpdateCache>(&contents) else {
        return UpdateCache::default();
    };
    if cache.schema == CACHE_SCHEMA {
        cache
    } else {
        UpdateCache::default()
    }
}

fn save_cache(cache: &UpdateCache) {
    let Some(path) = cache_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(mut file) = tempfile::NamedTempFile::new_in(parent) else {
        return;
    };
    let mut value = cache.clone();
    value.schema = CACHE_SCHEMA;
    let Ok(json) = serde_json::to_vec(&value) else {
        return;
    };
    if file.write_all(&json).is_err() || file.as_file().sync_all().is_err() {
        return;
    }
    let _ = file.persist(path);
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    fn cache(next: Option<u64>) -> UpdateCache {
        UpdateCache {
            schema: CACHE_SCHEMA,
            next_attempt_after: next,
            ..UpdateCache::default()
        }
    }

    #[test]
    fn cache_base_uses_xdg_then_dot_cache_on_every_platform() {
        assert_eq!(
            cache_base(
                Some(std::ffi::OsStr::new("/tmp/custom-cache")),
                Some(Path::new("/home/raine")),
            ),
            Some(PathBuf::from("/tmp/custom-cache"))
        );
        assert_eq!(
            cache_base(None, Some(Path::new("/home/raine"))),
            Some(PathBuf::from("/home/raine/.cache"))
        );
        assert_eq!(cache_base(Some(std::ffi::OsStr::new("")), None), None);
    }

    #[test]
    fn scheduling_respects_future_attempt_and_clock_skew() {
        assert!(!check_due_at(&cache(Some(2_000)), 1_000));
        assert!(check_due_at(&cache(Some(1_000)), 2_000));
        assert!(check_due_at(
            &cache(Some(1_000 + SUCCESS_INTERVAL + 1)),
            1_000
        ));
        assert!(check_due_at(&UpdateCache::default(), 1_000));
    }

    #[test]
    fn later_hides_one_version_for_seven_days() {
        let version = Version::new(2, 0, 0);
        let dismissal = dismissal_at(version.clone(), 1_000);

        assert!(dismissal.applies_to_at(&version, 1_000 + DISMISS_INTERVAL - 1));
        assert!(!dismissal.applies_to_at(&version, 1_000 + DISMISS_INTERVAL));
        assert!(!dismissal.applies_to_at(&Version::new(3, 0, 0), 1_001));
    }

    #[test]
    fn failures_back_off_to_a_daily_cap() {
        assert_eq!(failure_delay(1), 15 * 60);
        assert_eq!(failure_delay(2), 60 * 60);
        assert_eq!(failure_delay(3), 6 * 60 * 60);
        assert_eq!(failure_delay(4), 24 * 60 * 60);
        assert_eq!(failure_delay(100), 24 * 60 * 60);
    }

    #[test]
    fn cached_outcome_uses_semver_precedence() {
        let mut cache = cache(None);
        cache.release = Some(Release {
            version: Version::new(99, 0, 0),
            tag: "v99.0.0".to_string(),
            archive_name: "aven-test.tar.gz".to_string(),
            archive_url: "https://github.com/raine/aven/releases/download/v99.0.0/a".to_string(),
            checksum_url: "https://github.com/raine/aven/releases/download/v99.0.0/b".to_string(),
            app: None,
        });
        assert!(matches!(
            outcome_from_cache(&cache, true).unwrap(),
            CheckOutcome::Available { cached: true, .. }
        ));
    }

    #[test]
    fn confirmed_answers_replace_or_withdraw_cached_offers() {
        let release = |major| Release {
            version: Version::new(major, 0, 0),
            tag: format!("v{major}.0.0"),
            archive_name: String::new(),
            archive_url: String::new(),
            checksum_url: String::new(),
            app: None,
        };
        let mut cache = cache(None);
        for major in [99, 98] {
            apply_fetched(
                &mut cache,
                FetchResult::Release {
                    release: release(major),
                    etag: Some(format!("etag-{major}")),
                },
            );
            assert!(matches!(outcome_from_cache(&cache, false).unwrap(),
                CheckOutcome::Available { release, .. } if release.version == Version::new(major, 0, 0)));
        }
        apply_fetched(
            &mut cache,
            FetchResult::NotModified {
                etag: Some("etag-98".into()),
            },
        );
        assert_eq!(
            cache.release.as_ref().unwrap().version,
            Version::new(98, 0, 0)
        );
        apply_fetched(&mut cache, FetchResult::Unavailable);
        assert!(cache.release.is_none());
        assert!(cache.etag.is_none());
        assert!(matches!(
            outcome_from_cache(&cache, true).unwrap(),
            CheckOutcome::Current { .. }
        ));
        apply_fetched(
            &mut cache,
            FetchResult::Release {
                release: release(97),
                etag: None,
            },
        );
        assert!(matches!(
            outcome_from_cache(&cache, false).unwrap(),
            CheckOutcome::Available { .. }
        ));
        apply_fetched(
            &mut cache,
            FetchResult::Release {
                release: release(0),
                etag: None,
            },
        );
        assert!(matches!(
            outcome_from_cache(&cache, false).unwrap(),
            CheckOutcome::Current { .. }
        ));
    }

    #[test]
    fn empty_cache_has_no_outcome() {
        let error = outcome_from_cache(&cache(None), true).unwrap_err();
        assert_eq!(
            error.to_string(),
            "no cached release information is available"
        );
    }
}
