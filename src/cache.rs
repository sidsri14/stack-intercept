use serde_json::Value;
use sha2::{Sha256, Digest};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct CachedEntry {
    pub response_body: String,
    pub created_at: Instant,
    pub ttl: Duration,
}

pub struct ExactCache {
    entries: Vec<(String, CachedEntry)>,
    max_entries: usize,
    default_ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct CacheKey {
    pub provider: String,
    pub model: String,
    pub messages_json: String,
    pub tools_json: Option<String>,
    pub response_format_json: Option<String>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
    pub tenant_id: Option<String>,
    pub stream: bool,
}

impl CacheKey {
    /// Build from a parsed chat completions payload.
    /// Returns None if the payload is not cache-eligible.
    pub fn from_payload(payload: &Value, tenant_id: Option<String>) -> Option<Self> {
        // Only cache when temperature is 0 or absent
        let temp = payload["temperature"].as_f64();
        if temp.is_some_and(|t| t != 0.0) {
            return None;
        }

        // Don't cache if tools are present
        if let Some(tools) = payload["tools"].as_array() {
            if !tools.is_empty() {
                return None;
            }
        }

        let stream = payload["stream"].as_bool().unwrap_or(false);

        Some(Self {
            provider: "openai".to_string(),
            model: payload["model"].as_str().unwrap_or("unknown").to_string(),
            messages_json: serde_json::to_string(&payload["messages"]).unwrap_or_default(),
            tools_json: payload["tools"].get(0).map(|_| serde_json::to_string(&payload["tools"]).unwrap_or_default()),
            response_format_json: payload["response_format"].as_object().map(|_| serde_json::to_string(&payload["response_format"]).unwrap_or_default()),
            temperature: temp,
            top_p: payload["top_p"].as_f64(),
            max_tokens: payload["max_tokens"].as_u64().map(|v| v as u32),
            tenant_id,
            stream,
        })
    }

    /// Deterministic hex hash for use as a lookup key
    pub fn hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.provider);
        hasher.update(&self.model);
        hasher.update(&self.messages_json);
        if let Some(t) = &self.tools_json { hasher.update(t); }
        if let Some(f) = &self.response_format_json { hasher.update(f); }
        if let Some(t) = self.temperature { hasher.update(&t.to_le_bytes()); }
        if let Some(t) = self.top_p { hasher.update(&t.to_le_bytes()); }
        if let Some(m) = self.max_tokens { hasher.update(&m.to_le_bytes()); }
        if let Some(t) = &self.tenant_id { hasher.update(t); }
        hasher.update(&[self.stream as u8]);
        format!("{:x}", hasher.finalize())
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

    pub fn insert(&mut self, key: String, body: String) {
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
