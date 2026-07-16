use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use dashmap::DashMap;

#[derive(Clone, Debug)]
pub struct CachedEntry {
    pub response_body: Vec<u8>,
    pub created_at: Instant,
    pub ttl: Duration,
}

#[derive(Clone, Debug)]
pub struct CacheItem {
    pub prompt: String,
    pub vector: Vec<f32>,
    pub completion_response: Vec<u8>,
    pub created_at: Instant,
    pub ttl: Duration,
}

/// Evict expired entries from a semantic cache bucket, then oldest entries
/// if the bucket still exceeds `max_bucket_items`. Returns number evicted.
pub fn evict_bucket(bucket: &mut Vec<CacheItem>, max_bucket_items: usize) -> usize {
    let before = bucket.len();
    // Remove expired entries
    bucket.retain(|item| item.created_at.elapsed() < item.ttl);
    // Remove oldest entries until under the cap
    while bucket.len() > max_bucket_items {
        let oldest_idx = bucket
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.created_at.cmp(&b.created_at))
            .map(|(i, _)| i)
            .unwrap_or(0);
        bucket.remove(oldest_idx);
    }
    before.saturating_sub(bucket.len())
}

/// Remove a specific item by index from a semantic cache bucket.
/// Returns true if an item was removed, false if the bucket doesn't exist
/// or the index is out of bounds.
pub fn evict_item(
    index: &DashMap<String, Vec<CacheItem>>,
    bucket_id: &str,
    item_id: usize,
) -> bool {
    if let Some(mut bucket) = index.get_mut(bucket_id) {
        if item_id < bucket.len() {
            bucket.remove(item_id);
            true
        } else {
            false
        }
    } else {
        false
    }
}

/// Scan all shards in the semantic index and evict entries if total exceeds
/// `max_items`. Expired entries are removed first, then the oldest entries
/// globally. Returns total number of entries evicted.
pub fn evict_global(index: &DashMap<String, Vec<CacheItem>>, max_items: usize) -> usize {
    // First pass: remove expired entries from all buckets
    let mut total_removed = 0usize;
    index.iter_mut().for_each(|mut bucket| {
        let before = bucket.len();
        bucket.retain(|item| item.created_at.elapsed() < item.ttl);
        total_removed += before.saturating_sub(bucket.len());
    });

    // Count total entries across ALL buckets (not bucket count)
    let total_entries: usize = index.iter().map(|e| e.value().len()).sum();
    if total_entries <= max_items {
        return total_removed;
    }

    let overage = total_entries.saturating_sub(max_items);
    let mut to_remove = Vec::new(); // (context_key, index_in_bucket)

    // Collect candidate entries sorted by age across all shards
    for entry in index.iter() {
        let ctx = entry.key().clone();
        for (i, item) in entry.value().iter().enumerate() {
            to_remove.push((ctx.clone(), i, item.created_at));
        }
    }

    // Sort by age (oldest first)
    to_remove.sort_by_key(|item| item.2);

    // Remove the oldest entries (process newest-first to preserve indices)
    let remove_count = overage.min(to_remove.len());
    let to_remove_set: Vec<_> = to_remove[..remove_count]
        .iter()
        .map(|(ctx, idx, _)| (ctx.clone(), *idx))
        .collect();

    // Group removals by context key, sort indices descending so removal
    // doesn't shift remaining indices
    use std::collections::HashMap as StdHashMap;
    let mut by_key: StdHashMap<String, Vec<usize>> = StdHashMap::new();
    for (ctx, idx) in to_remove_set {
        by_key.entry(ctx).or_default().push(idx);
    }
    for (ctx, mut indices) in by_key {
        indices.sort_by(|a, b| b.cmp(a)); // descending
        if let Some(mut bucket) = index.get_mut(&ctx) {
            for idx in indices {
                if idx < bucket.len() {
                    bucket.remove(idx);
                    total_removed += 1;
                }
            }
        }
    }

    total_removed
}

pub struct ExactCache {
    entries: HashMap<String, CachedEntry>,
    max_entries: usize,
    default_ttl: Duration,
}

/// Recursively sort JSON object keys for deterministic serialization.
fn sort_json_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // Collect all entries, sort by key, rebuild the map
            let mut sorted: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            for (_, v) in &mut sorted {
                sort_json_keys(v);
            }
            *map = sorted.into_iter().collect();
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                sort_json_keys(v);
            }
        }
        _ => {}
    }
}

/// Serialize to canonical (deterministic, key-sorted) JSON.
pub fn canonical_json(value: &Value) -> String {
    let mut cloned = value.clone();
    sort_json_keys(&mut cloned);
    serde_json::to_string(&cloned).unwrap_or_default()
}

/// Extract the hostname from an upstream base URL.
/// e.g. "https://api.deepseek.com" -> "api.deepseek.com"
fn extract_hostname(upstream_base_url: &str) -> String {
    upstream_base_url
        .strip_prefix("https://")
        .or_else(|| upstream_base_url.strip_prefix("http://"))
        .and_then(|s| s.split('/').next())
        .unwrap_or(upstream_base_url)
        .to_string()
}

/// Build a deterministic SHA256 hash of the full canonical payload for exact cache lookup.
/// Returns None if the request is not cache-eligible.
/// `routing_namespace` distinguishes routed vs passthrough responses (preventing
/// cross-contamination between different routing decisions for the same payload).
pub fn cache_key_hash(
    payload: &Value,
    tenant_id: Option<String>,
    upstream_base_url: &str,
    routing_namespace: Option<&str>,
) -> Option<String> {
    if !is_eligible(payload) {
        return None;
    }

    let mut hasher = Sha256::new();

    // Provider hostname
    hasher.update(extract_hostname(upstream_base_url).as_bytes());

    // Tenant
    if let Some(t) = tenant_id {
        hasher.update(t.as_bytes());
    }

    // Routing namespace — prevents cache cross-contamination
    if let Some(ns) = routing_namespace {
        hasher.update(ns.as_bytes());
    }

    // Canonical full payload JSON (recursively sorted keys for determinism)
    hasher.update(canonical_json(payload).as_bytes());

    Some(format!("{:x}", hasher.finalize()))
}

/// Check whether a request is eligible for caching at all.
pub fn is_eligible(payload: &Value) -> bool {
    let has_no_store = payload["cache_control"].as_str() == Some("no_store");
    if has_no_store {
        return false;
    }

    let temp = payload["temperature"].as_f64();
    if temp.is_some_and(|t| t != 0.0) {
        return false;
    }

    if let Some(tools) = payload["tools"].as_array() {
        if !tools.is_empty() {
            return false;
        }
    }

    true
}

impl ExactCache {
    pub fn new(max_entries: usize, default_ttl_secs: u64) -> Self {
        Self {
            entries: HashMap::with_capacity(max_entries.min(1024)),
            max_entries,
            default_ttl: Duration::from_secs(default_ttl_secs),
        }
    }

    pub fn get(&self, key: &str) -> Option<&CachedEntry> {
        if let Some(entry) = self.entries.get(key) {
            if entry.created_at.elapsed() < entry.ttl {
                return Some(entry);
            }
        }
        None
    }

    pub fn insert(&mut self, key: String, body: Vec<u8>) {
        // Evict if at capacity
        if self.entries.len() >= self.max_entries {
            let expired_key = self
                .entries
                .iter()
                .find(|(_, e)| e.created_at.elapsed() >= e.ttl)
                .map(|(k, _)| k.clone());

            if let Some(k) = expired_key {
                self.entries.remove(&k);
            } else {
                // Evict the entry with the oldest created_at (FIFO eviction)
                let oldest = self
                    .entries
                    .iter()
                    .min_by(|(_, a), (_, b)| a.created_at.cmp(&b.created_at))
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest {
                    self.entries.remove(&k);
                }
            }
        }
        self.entries.insert(
            key,
            CachedEntry {
                response_body: body,
                created_at: Instant::now(),
                ttl: self.default_ttl,
            },
        );
    }

    /// Export entries for snapshot serialization (key, body, epoch_secs, ttl_secs).
    pub fn snapshot_entries(&self) -> Vec<(String, Vec<u8>, u64, u64)> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.entries
            .iter()
            .map(|(k, e)| {
                let elapsed = e.created_at.elapsed().as_secs();
                let epoch = now.saturating_sub(elapsed);
                (k.clone(), e.response_body.clone(), epoch, e.ttl.as_secs())
            })
            .collect()
    }

    /// Restore entries from a snapshot.
    pub fn restore_from_snapshot(&mut self, entries: Vec<(String, Vec<u8>, u64, u64)>) {
        let snapshot_ttl = self.default_ttl;
        for (key, body, epoch_secs, ttl_secs) in entries {
            if self.entries.len() >= self.max_entries {
                break;
            }
            let ttl = Duration::from_secs(ttl_secs);
            let created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .and_then(|d| {
                    let secs = d.as_secs();
                    if secs >= epoch_secs {
                        Some(Instant::now() - Duration::from_secs(secs - epoch_secs))
                    } else {
                        None // epoch in the future, skip
                    }
                })
                .unwrap_or_else(Instant::now);
            // Skip expired entries
            if created_at.elapsed() >= ttl && ttl != snapshot_ttl {
                continue;
            }
            self.entries.insert(
                key,
                CachedEntry {
                    response_body: body,
                    created_at,
                    ttl,
                },
            );
        }
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn default_ttl_secs(&self) -> u64 {
        self.default_ttl.as_secs()
    }
}

// ── Snapshot types for disk persistence ──

#[derive(serde::Serialize, serde::Deserialize)]
pub struct SnapshotEntry {
    pub key: String,
    pub response_body: Vec<u8>,
    pub created_at_epoch: u64,
    pub ttl_secs: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct SnapshotItem {
    pub context_key: String,
    pub prompt: String,
    pub vector: Vec<f32>,
    pub completion_response: Vec<u8>,
    pub created_at_epoch: u64,
    pub ttl_secs: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub exact_entries: Vec<SnapshotEntry>,
    pub semantic_entries: Vec<SnapshotItem>,
}

/// Save the full cache state to disk atomically.
/// Writes to `path.tmp`, then renames to `path` (atomic on most filesystems).
pub fn save_snapshot(
    path: &str,
    exact_cache: &ExactCache,
    semantic_index: &DashMap<String, Vec<CacheItem>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let exact_entries: Vec<SnapshotEntry> = exact_cache
        .snapshot_entries()
        .into_iter()
        .map(|(key, body, epoch, ttl)| SnapshotEntry {
            key,
            response_body: body,
            created_at_epoch: epoch,
            ttl_secs: ttl,
        })
        .collect();

    let mut semantic_entries = Vec::new();
    for entry in semantic_index.iter() {
        let ctx = entry.key().clone();
        for item in entry.value().iter() {
            let elapsed = item.created_at.elapsed().as_secs();
            let epoch = now.saturating_sub(elapsed);
            semantic_entries.push(SnapshotItem {
                context_key: ctx.clone(),
                prompt: item.prompt.clone(),
                vector: item.vector.clone(),
                completion_response: item.completion_response.clone(),
                created_at_epoch: epoch,
                ttl_secs: item.ttl.as_secs(),
            });
        }
    }

    let snapshot = Snapshot {
        exact_entries,
        semantic_entries,
    };

    let tmp_path = format!("{}.tmp", path);
    let bytes = rmp_serde::to_vec(&snapshot)?;
    std::fs::write(&tmp_path, &bytes)?;
    std::fs::rename(&tmp_path, path)?;

    Ok(())
}

/// Load cache state from a disk snapshot.
pub fn load_snapshot(path: &str) -> Result<Snapshot, Box<dyn std::error::Error>> {
    let bytes = std::fs::read(path)?;
    let snapshot: Snapshot = rmp_serde::from_slice(&bytes)?;
    Ok(snapshot)
}

// ── Unit tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_evict_item_valid() {
        let index = DashMap::new();
        let bucket = vec![
            make_item(100, 3600),
            make_item(50, 3600),
            make_item(10, 3600),
        ];
        index.insert("ctx".to_string(), bucket);

        // Remove middle item (index 1)
        let result = evict_item(&index, "ctx", 1);
        assert!(result);

        let remaining = index.get("ctx").unwrap();
        assert_eq!(remaining.len(), 2);
        // The 100s-old and 10s-old items remain
        assert!(remaining[0].created_at.elapsed().as_secs() > 80);
        assert!(remaining[1].created_at.elapsed().as_secs() < 20);
    }

    #[test]
    fn test_evict_item_out_of_bounds() {
        let index = DashMap::new();
        let bucket = vec![make_item(10, 3600)];
        index.insert("ctx".to_string(), bucket);

        let result = evict_item(&index, "ctx", 5);
        assert!(!result);
        assert_eq!(index.get("ctx").unwrap().len(), 1);
    }

    #[test]
    fn test_evict_item_nonexistent_bucket() {
        let index: DashMap<String, Vec<CacheItem>> = DashMap::new();
        let result = evict_item(&index, "does-not-exist", 0);
        assert!(!result);
    }

    fn make_item(created_ago_secs: u64, ttl_secs: u64) -> CacheItem {
        CacheItem {
            prompt: String::new(),
            vector: vec![],
            completion_response: vec![],
            created_at: Instant::now() - Duration::from_secs(created_ago_secs),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    #[test]
    fn test_evict_bucket_expired() {
        let mut bucket = vec![make_item(100, 10)]; // 100s old, 10s TTL -> expired
        let evicted = evict_bucket(&mut bucket, 10);
        assert_eq!(evicted, 1);
        assert!(bucket.is_empty());
    }

    #[test]
    fn test_evict_bucket_cap_enforced() {
        let mut bucket: Vec<CacheItem> = (0..10).map(|i| make_item(i, 3600)).collect();
        let evicted = evict_bucket(&mut bucket, 5);
        assert_eq!(evicted, 5);
        assert_eq!(bucket.len(), 5);
        // The 5 newest items (smallest created_ago) remain
    }

    #[test]
    fn test_evict_bucket_under_cap_noop() {
        let mut bucket: Vec<CacheItem> = (0..3).map(|i| make_item(i, 3600)).collect();
        let evicted = evict_bucket(&mut bucket, 10);
        assert_eq!(evicted, 0);
        assert_eq!(bucket.len(), 3);
    }

    #[test]
    fn test_evict_global_counts_total_entries_not_buckets() {
        let index = DashMap::new();
        // 3 buckets, each with 2 entries = 6 total
        for ctx in ["a", "b", "c"] {
            let mut bucket = Vec::new();
            for _ in 0..2 {
                bucket.push(make_item(10, 3600));
            }
            index.insert(ctx.to_string(), bucket);
        }
        assert_eq!(index.len(), 3); // 3 buckets

        // max_items = 4 total entries -> should evict 2
        let evicted = evict_global(&index, 4);
        assert_eq!(evicted, 2);
        let total: usize = index.iter().map(|e| e.value().len()).sum();
        assert_eq!(total, 4);
    }

    #[test]
    fn test_evict_global_removes_oldest_first() {
        let index = DashMap::new();
        // One bucket with 3 items aged 100s, 50s, 10s
        let bucket = vec![
            make_item(100, 3600),
            make_item(50, 3600),
            make_item(10, 3600),
        ];
        index.insert("ctx".to_string(), bucket);

        // max_items = 1 -> evict 2 oldest (100s and 50s)
        let evicted = evict_global(&index, 1);
        assert_eq!(evicted, 2);
        let remaining: Vec<_> = index
            .get("ctx")
            .unwrap()
            .iter()
            .map(|i| i.created_at.elapsed().as_secs())
            .collect();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0] < 20); // the ~10s old item remained
    }

    #[test]
    fn test_evict_global_expired_removed_first() {
        let index = DashMap::new();
        // 2 expired, 1 fresh
        let bucket = vec![
            make_item(100, 10), // expired (100s old, 10s TTL)
            make_item(50, 10),  // expired (50s old, 10s TTL)
            make_item(5, 3600), // fresh
        ];
        index.insert("ctx".to_string(), bucket);

        // max_items = 3 (bucket count < 3) but expired items should still be removed
        let evicted = evict_global(&index, 3);
        assert_eq!(evicted, 2);
        let total: usize = index.iter().map(|e| e.value().len()).sum();
        assert_eq!(total, 1);
    }

    #[test]
    fn test_exact_cache_ttl() {
        let mut cache = ExactCache::new(100, 1); // 1 second TTL
        cache.insert("key1".to_string(), b"hello".to_vec());
        assert!(cache.get("key1").is_some());
        std::thread::sleep(Duration::from_millis(1100));
        assert!(cache.get("key1").is_none()); // expired
    }

    #[test]
    fn test_exact_cache_max_entries() {
        let mut cache = ExactCache::new(2, 3600);
        cache.insert("a".to_string(), b"1".to_vec());
        cache.insert("b".to_string(), b"2".to_vec());
        cache.insert("c".to_string(), b"3".to_vec()); // should evict oldest (a)
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_some());
        assert!(cache.get("c").is_some());
    }

    #[test]
    fn test_exact_cache_snapshot_roundtrip() {
        let mut cache = ExactCache::new(100, 3600);
        cache.insert("k1".to_string(), b"v1".to_vec());
        cache.insert("k2".to_string(), b"v2".to_vec());

        let entries = cache.snapshot_entries();
        assert_eq!(entries.len(), 2);

        let mut restored = ExactCache::new(100, 3600);
        restored.restore_from_snapshot(entries);
        assert!(restored.get("k1").is_some());
        assert!(restored.get("k2").is_some());
        assert_eq!(restored.get("k1").unwrap().response_body, b"v1");
    }

    #[test]
    fn test_exact_cache_snapshot_skips_expired() {
        let mut cache = ExactCache::new(100, 1); // 1s TTL
        cache.insert("stale".to_string(), b"old".to_vec());

        let entries = cache.snapshot_entries(); // snapshot has 1s TTL
                                                // Sleep so entry expires under its original TTL
        std::thread::sleep(Duration::from_millis(1100));

        let mut restored = ExactCache::new(100, 3600); // restore with longer TTL
        restored.restore_from_snapshot(entries);
        // When ttl (1) != snapshot_ttl (3600), the expired check applies.
        // Entry was 1.1s old with 1s TTL → expired → skipped.
        assert!(restored.get("stale").is_none());
    }
}
