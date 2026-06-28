use serde_json::Value;
use sha2::{Sha256, Digest};
use std::time::{Duration, Instant};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct CachedEntry {
    pub response_body: Vec<u8>,
    pub created_at: Instant,
    pub ttl: Duration,
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
pub fn cache_key_hash(payload: &Value, tenant_id: Option<String>, upstream_base_url: &str) -> Option<String> {
    if !is_eligible(payload) { return None; }

    let mut hasher = Sha256::new();

    // Provider hostname
    hasher.update(extract_hostname(upstream_base_url).as_bytes());

    // Tenant
    if let Some(t) = tenant_id { hasher.update(t.as_bytes()); }

    // Canonical full payload JSON (recursively sorted keys for determinism)
    hasher.update(canonical_json(payload).as_bytes());

    Some(format!("{:x}", hasher.finalize()))
}

/// Check whether a request is eligible for caching at all.
pub fn is_eligible(payload: &Value) -> bool {
    let has_no_store = payload["cache_control"].as_str() == Some("no_store");
    if has_no_store { return false; }

    let temp = payload["temperature"].as_f64();
    if temp.is_some_and(|t| t != 0.0) { return false; }

    if let Some(tools) = payload["tools"].as_array() {
        if !tools.is_empty() { return false; }
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
            let expired_key = self.entries.iter()
                .find(|(_, e)| e.created_at.elapsed() >= e.ttl)
                .map(|(k, _)| k.clone());

            if let Some(k) = expired_key {
                self.entries.remove(&k);
            } else {
                // Evict the entry with the oldest created_at (FIFO eviction)
                let oldest = self.entries.iter()
                    .min_by(|(_, a), (_, b)| a.created_at.cmp(&b.created_at))
                    .map(|(k, _)| k.clone());
                if let Some(k) = oldest {
                    self.entries.remove(&k);
                }
            }
        }
        self.entries.insert(key, CachedEntry {
            response_body: body,
            created_at: Instant::now(),
            ttl: self.default_ttl,
        });
    }
}
