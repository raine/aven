use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::release::{FetchResult, fetch_latest};
use super::{CheckOutcome, Release, current_version};

const CACHE_SCHEMA: u32 = 1;
const SUCCESS_INTERVAL: u64 = 24 * 60 * 60;
const FAILURE_BACKOFF: [u64; 4] = [15 * 60, 60 * 60, 6 * 60 * 60, 24 * 60 * 60];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UpdateCache {
    #[serde(default)]
    schema: u32,
    etag: Option<String>,
    release: Option<Release>,
    last_success_at: Option<u64>,
    last_attempt_at: Option<u64>,
    next_attempt_after: Option<u64>,
    #[serde(default)]
    failure_count: u32,
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

pub(crate) async fn check_for_update(
    client: &reqwest::Client,
    force: bool,
) -> Result<CheckOutcome> {
    let mut cache = load_cache();
    let now = now_secs();
    if !force && !check_due_at(&cache, now) {
        return outcome_from_cache(&cache, true);
    }

    cache.last_attempt_at = Some(now);
    save_cache(&cache);
    let etag = cache.release.as_ref().and(cache.etag.as_deref());
    match fetch_latest(client, etag).await {
        Ok(FetchResult::NotModified { etag }) => {
            cache.etag = etag;
            cache.last_success_at = Some(now);
            cache.next_attempt_after = Some(now.saturating_add(SUCCESS_INTERVAL));
            cache.failure_count = 0;
            save_cache(&cache);
            outcome_from_cache(&cache, false)
        }
        Ok(FetchResult::Release { release, etag }) => {
            cache.release = Some(release);
            cache.etag = etag;
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

fn outcome_from_cache(cache: &UpdateCache, cached: bool) -> Result<CheckOutcome> {
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
    if cache.schema != CACHE_SCHEMA {
        return true;
    }
    !matches!(
        cache.next_attempt_after,
        Some(next) if next >= now && next.saturating_sub(now) <= SUCCESS_INTERVAL
    )
}

fn cache_path() -> Option<PathBuf> {
    cache_base(
        std::env::var_os("XDG_CACHE_HOME").as_deref(),
        dirs::home_dir().as_deref(),
    )
    .map(|base| base.join("aven").join("update.json"))
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
        });
        assert!(matches!(
            outcome_from_cache(&cache, true).unwrap(),
            CheckOutcome::Available { cached: true, .. }
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
