use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, bail};
use base64::Engine;
use tokio::task::JoinSet;

const MAX_PREVIEW_CONCURRENCY: usize = 2;
const MAX_READY_PREVIEWS: usize = 8;
const MAX_PREVIEW_FAILURES: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PreviewKey {
    blob_dir: PathBuf,
    source_hash: String,
    preview_quota_bytes: u64,
    profile: &'static str,
}

impl PreviewKey {
    pub(crate) fn new(blob_dir: &Path, source_hash: &str, preview_quota_bytes: u64) -> Self {
        Self {
            blob_dir: blob_dir.to_path_buf(),
            source_hash: source_hash.to_string(),
            preview_quota_bytes,
            profile: crate::attachments::preview::PREVIEW_PROFILE,
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreviewPayload {
    encoded_png: String,
    byte_len: usize,
}

impl PreviewPayload {
    pub(crate) fn encoded_png(&self) -> &str {
        &self.encoded_png
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.byte_len
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PreviewLease(Arc<PreviewPayload>);

impl PreviewLease {
    pub(crate) fn payload(&self) -> &PreviewPayload {
        &self.0
    }
}

#[derive(Debug)]
struct CachedPreview {
    payload: Arc<PreviewPayload>,
    last_used: u64,
}

#[derive(Debug)]
struct PreviewWorkResult {
    generation: u64,
    key: PreviewKey,
    payload: Option<PreviewPayload>,
}

pub(crate) struct PreviewController {
    generation: u64,
    desired: HashSet<PreviewKey>,
    queued: VecDeque<(u64, PreviewKey)>,
    queued_keys: HashSet<PreviewKey>,
    pending: HashMap<PreviewKey, u64>,
    ready: HashMap<PreviewKey, CachedPreview>,
    failures: HashMap<PreviewKey, u8>,
    tasks: JoinSet<PreviewWorkResult>,
    usage_clock: u64,
    max_concurrency: usize,
    max_ready: usize,
}

impl PreviewController {
    pub(crate) fn new() -> Self {
        Self::with_limits(MAX_PREVIEW_CONCURRENCY, MAX_READY_PREVIEWS)
    }

    fn with_limits(max_concurrency: usize, max_ready: usize) -> Self {
        Self {
            generation: 0,
            desired: HashSet::new(),
            queued: VecDeque::new(),
            queued_keys: HashSet::new(),
            pending: HashMap::new(),
            ready: HashMap::new(),
            failures: HashMap::new(),
            tasks: JoinSet::new(),
            usage_clock: 0,
            max_concurrency,
            max_ready,
        }
    }

    pub(crate) fn has_desired(&self) -> bool {
        !self.desired.is_empty()
    }

    pub(crate) fn set_desired(&mut self, desired: impl IntoIterator<Item = PreviewKey>) {
        let desired = desired.into_iter().collect::<HashSet<_>>();
        if desired != self.desired {
            self.generation = self.generation.wrapping_add(1);
            self.desired = desired;
            self.queued.clear();
            self.queued_keys.clear();
        }
        let keys = self.desired.iter().cloned().collect::<Vec<_>>();
        for key in keys {
            self.enqueue_if_needed(key);
        }
        self.start_queued_work();
        self.evict_ready();
    }

    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Some(result) = self.tasks.try_join_next() {
            let Ok(result) = result else {
                continue;
            };
            changed |= self.accept_result(result);
        }
        self.start_queued_work();
        changed
    }

    pub(crate) fn lease(&mut self, key: &PreviewKey) -> Option<PreviewLease> {
        self.usage_clock = self.usage_clock.wrapping_add(1);
        let cached = self.ready.get_mut(key)?;
        cached.last_used = self.usage_clock;
        Some(PreviewLease(Arc::clone(&cached.payload)))
    }

    pub(crate) fn work_pending(&self) -> bool {
        !self.queued.is_empty() || !self.pending.is_empty()
    }

    pub(crate) fn suppressed_hashes(&self, blob_dir: &Path) -> HashSet<String> {
        self.failures
            .iter()
            .filter(|(key, failures)| {
                key.blob_dir == blob_dir && **failures >= MAX_PREVIEW_FAILURES
            })
            .map(|(key, _)| key.source_hash.clone())
            .collect()
    }

    fn enqueue_if_needed(&mut self, key: PreviewKey) {
        if self.ready.contains_key(&key)
            || self.pending.contains_key(&key)
            || self.queued_keys.contains(&key)
            || self.failures.get(&key).copied().unwrap_or(0) >= MAX_PREVIEW_FAILURES
        {
            return;
        }
        self.queued_keys.insert(key.clone());
        self.queued.push_back((self.generation, key));
    }

    fn start_queued_work(&mut self) {
        while self.pending.len() < self.max_concurrency {
            let Some((generation, key)) = self.queued.pop_front() else {
                break;
            };
            self.queued_keys.remove(&key);
            if generation != self.generation || !self.desired.contains(&key) {
                continue;
            }
            self.pending.insert(key.clone(), generation);
            self.tasks.spawn(load_preview(generation, key));
        }
    }

    fn accept_result(&mut self, result: PreviewWorkResult) -> bool {
        if self.pending.get(&result.key) == Some(&result.generation) {
            self.pending.remove(&result.key);
        }
        if result.generation != self.generation || !self.desired.contains(&result.key) {
            if self.desired.contains(&result.key) {
                self.enqueue_if_needed(result.key);
            }
            return false;
        }
        match result.payload {
            Some(payload) => {
                self.failures.remove(&result.key);
                self.usage_clock = self.usage_clock.wrapping_add(1);
                self.ready.insert(
                    result.key,
                    CachedPreview {
                        payload: Arc::new(payload),
                        last_used: self.usage_clock,
                    },
                );
                self.evict_ready();
                true
            }
            None => {
                let failures = self.failures.entry(result.key.clone()).or_default();
                *failures = failures.saturating_add(1);
                let suppressed = *failures >= MAX_PREVIEW_FAILURES;
                self.enqueue_if_needed(result.key);
                suppressed
            }
        }
    }

    fn evict_ready(&mut self) {
        while self.ready.len() > self.max_ready {
            let candidate = self
                .ready
                .iter()
                .filter(|(key, cached)| {
                    !self.desired.contains(*key) && Arc::strong_count(&cached.payload) == 1
                })
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(key, _)| key.clone());
            let Some(candidate) = candidate else {
                break;
            };
            self.ready.remove(&candidate);
        }
    }
}

fn encode_preview_png(bytes: Vec<u8>) -> Result<PreviewPayload> {
    if bytes.len() > crate::attachments::preview::MAX_PREVIEW_BYTES
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        bail!("invalid terminal preview payload");
    }
    let byte_len = bytes.len();
    let encoded_png = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(PreviewPayload {
        encoded_png,
        byte_len,
    })
}

async fn load_preview(generation: u64, key: PreviewKey) -> PreviewWorkResult {
    let bytes = crate::attachments::preview::load_preview_png(
        &key.blob_dir,
        &key.source_hash,
        key.preview_quota_bytes,
    )
    .await;
    let payload = match bytes {
        Ok(bytes) => crate::attachments::blocking::run_preview(move || encode_preview_png(bytes))
            .await
            .ok(),
        Err(_) => None,
    };
    PreviewWorkResult {
        generation,
        key,
        payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> PreviewKey {
        PreviewKey::new(Path::new("/tmp/blobs"), &format!("{name:0<64}"), u64::MAX)
    }

    fn payload(value: &str) -> PreviewPayload {
        PreviewPayload {
            encoded_png: value.to_string(),
            byte_len: value.len(),
        }
    }

    #[test]
    fn invalid_png_is_rejected_before_protocol_encoding() {
        assert!(encode_preview_png(b"private attachment bytes".to_vec()).is_err());
    }

    #[tokio::test]
    async fn preview_work_uses_small_bounded_concurrency() {
        let mut controller = PreviewController::with_limits(2, 8);

        controller.set_desired([key("a"), key("b"), key("c")]);

        assert_eq!(controller.pending.len(), 2);
        assert_eq!(controller.queued.len(), 1);
        controller.tasks.abort_all();
    }

    #[test]
    fn requests_are_deduplicated_by_hash_and_profile() {
        let mut controller = PreviewController::with_limits(0, 8);
        let image = key("a");

        controller.set_desired([image.clone(), image.clone()]);
        controller.set_desired([image]);

        assert_eq!(controller.queued.len(), 1);
    }

    #[test]
    fn stale_results_are_rejected() {
        let mut controller = PreviewController::with_limits(0, 8);
        let stale = key("a");
        controller.set_desired([stale.clone()]);
        let generation = controller.generation;
        controller.set_desired([key("b")]);

        assert!(!controller.accept_result(PreviewWorkResult {
            generation,
            key: stale.clone(),
            payload: Some(payload("stale")),
        }));
        assert!(controller.lease(&stale).is_none());
    }

    #[test]
    fn returning_to_a_key_requeues_after_stale_work_finishes() {
        let mut controller = PreviewController::with_limits(0, 8);
        let image = key("a");
        controller.set_desired([image.clone()]);
        let stale_generation = controller.generation;
        controller.queued.clear();
        controller.queued_keys.clear();
        controller.pending.insert(image.clone(), stale_generation);
        controller.set_desired([key("b")]);
        controller.set_desired([image.clone()]);

        controller.accept_result(PreviewWorkResult {
            generation: stale_generation,
            key: image.clone(),
            payload: Some(payload("stale")),
        });

        assert_eq!(
            controller.queued.front(),
            Some(&(controller.generation, image))
        );
    }

    #[test]
    fn repeated_failures_are_suppressed_for_the_session() {
        let mut controller = PreviewController::with_limits(0, 8);
        let image = key("a");
        controller.set_desired([image.clone()]);
        let generation = controller.generation;

        for _ in 0..MAX_PREVIEW_FAILURES {
            controller.accept_result(PreviewWorkResult {
                generation,
                key: image.clone(),
                payload: None,
            });
        }

        controller.queued.clear();
        controller.queued_keys.clear();
        controller.set_desired([image.clone()]);
        assert!(controller.queued.is_empty());
        assert_eq!(controller.failures.get(&image), Some(&MAX_PREVIEW_FAILURES));
    }

    #[test]
    fn active_lease_prevents_ready_cache_eviction() {
        let mut controller = PreviewController::with_limits(0, 1);
        let first = key("a");
        controller.set_desired([first.clone()]);
        let generation = controller.generation;
        controller.accept_result(PreviewWorkResult {
            generation,
            key: first.clone(),
            payload: Some(payload("first")),
        });
        let lease = controller.lease(&first).unwrap();

        let second = key("b");
        controller.set_desired([second.clone()]);
        let generation = controller.generation;
        controller.accept_result(PreviewWorkResult {
            generation,
            key: second,
            payload: Some(payload("second")),
        });
        assert!(controller.ready.contains_key(&first));

        drop(lease);
        controller.evict_ready();
        assert!(!controller.ready.contains_key(&first));
    }
}
