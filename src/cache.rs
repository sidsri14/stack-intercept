use serde_json::Value;
use sha2::{Sha256, Digest};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct CachedEntry {
    pub response_body: Vec<u8>,
    pub created_at: Instant,
    pub ttl: Duration,
}

pub struct ExactCache {
    entries: Vec<(String, CachedEntry)>,
    max_entries: usize,
    default_ttl: Duration,
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

    // Canonical full payload JSON (sorted keys)
    hasher.update(serde_json::to_string(payload).unwrap_or_default().as_bytes());

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
            entries: Vec::with_capacity(max_entries.min(1024)),
            max_entries,
            default_ttl: Duration::from_secs(default_ttl_secs),
        }
    }

    pub fn get(&self, key: &str) -> Option<&CachedEntry> {
        self.entries.iter().find_map(|(k, v)| {
            if k == key && v.created_at.elapsed() < v.ttl {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn insert(&mut self, key: String, body: Vec<u8>) {
        if self.entries.len() >= self.max_entries {
            // Remove oldest expired entry, or oldest overall
            if let Some(pos) = self.entries.iter().position(|(_, e)| e.created_at.elapsed() >= e.ttl) {
                self.entries.remove(pos);
            } else {
                self.entries.remove(0);
            }
        }
        self.entries.push((key, CachedEntry {
            response_body: body,
            created_at: Instant::now(),
            ttl: self.default_ttl,
        }));
    }
}
