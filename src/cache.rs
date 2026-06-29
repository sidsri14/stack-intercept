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
    to_remove.sort_by(|a, b| a.2.cmp(&b.2));

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
