# Corrected Proxy Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite the StackIntercept proxy from a semantic-cache-first design to a compatibility-first design: raw passthrough, exact cache, then opt-in semantic cache with safety guards.

**Architecture:** The proxy sits between client SDKs and LLM providers. On cache miss, it forwards upstream bytes unchanged (no SSE re-wrapping). On cache hit, it replays stored bytes as-is. Cache key covers all request dimensions (provider, model, messages, tools, temperature, etc.) for exact matching. Semantic cache is gated by exact context match first, then embedding similarity within that bucket.

**Tech Stack:** Rust, Axum 0.7, reqwest 0.12, tokio, Candle 0.8 (BGE embeddings), SHA2 for cache keys.

---
## File Structure

```
src/
  main.rs        — HTTP handler, routing, AppState, cache logic
  embeddings.rs  — LocalPredictor (BGE embedding, UNCHANGED)
  cache.rs       — CacheConfig, CacheKey, CacheMode, eligibility check (NEW)
  config.rs      — Env-var-based configuration (NEW)
```

---

### Task 1: Raw Streaming Passthrough

**Files:**
- Modify: `src/main.rs:1-170`
- Dependencies: `Cargo.toml` (no new deps needed — axum Body is built-in)

Replaces all `axum::response::Sse<Event>` wrapping with raw byte forwarding using `axum::body::Body::from_stream`. Both the upstream streaming path and the cached replay path forward raw SSE bytes with `content-type: text/event-stream`.

- [ ] **Step 1: Rewrite `handle_intercept` for raw passthrough**

Replace the entire streaming passthrough block. The current code:

```rust
// Current (WRONG — wraps chunks in Sse<Event>)
let stream = res.bytes_stream().map(move |chunk_result| {
    match chunk_result {
        Ok(bytes) => {
            let raw_str = String::from_utf8_lossy(&bytes).to_string();
            // ... buffer logic ...
            Ok::<Event, Infallible>(Event::default().data(raw_str))
        },
        Err(_) => Ok(Event::default().data("[ERROR]")),
    }
});
return Sse::new(stream).into_response();
```

Replace with:

```rust
// Raw passthrough — forward upstream bytes unchanged
let stream = res.bytes_stream().map(|chunk| {
    chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
});
let body = axum::body::Body::from_stream(stream);
return axum::response::Response::builder()
    .header("content-type", "text/event-stream")
    .header("cache-control", "no-store")
    .header("x-stack-intercept", "miss")
    .body(body)
    .unwrap()
    .into_response();
```

- [ ] **Step 2: Rewrite `handle_cached_stream` for raw replay**

The current `handle_cached_stream` splits SSE events and re-wraps them in `Event::default().data(line)`. Replace with raw byte replay so the cached SSE text is returned as-is.

Remove the entire function and inline the logic. When a cache hit occurs for a streaming request, return the raw cached string as a streaming body:

```rust
// Cache HIT streaming — replay raw bytes
let cached = item.completion_response.clone();
let stream = futures_util::stream::once(async move {
    Ok::<_, std::io::Error>(bytes::Bytes::from(cached))
});
let body = axum::body::Body::from_stream(stream);
return axum::response::Response::builder()
    .header("content-type", "text/event-stream")
    .header("x-stack-intercept", "hit")
    .body(body)
    .unwrap()
    .into_response();
```

- [ ] **Step 3: Clean up imports**

Remove unused imports: `Sse`, `Event`, `Infallible` from the axum and futures_util imports. Add `axum::body::Body` and `axum::response::Response`.

New imports block:

```rust
use axum::{
    routing::post,
    Router,
    response::IntoResponse,
    body::Body,
    http::{StatusCode, HeaderMap, Response},
    Json,
    extract::State,
};
use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use crate::embeddings::LocalPredictor;
```

- [ ] **Step 4: Compile and verify**

Run: `./build.cmd build 2>&1`
Expected: `Compiling stack-intercept ... Finished dev profile`

Then start proxy and verify raw SSE passthrough:

```bash
./build.cmd run > /tmp/proxy_out.txt 2>&1 &
sleep 5
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"say hi"}],"stream":true}' \
  -o /tmp/sse_test.txt
```

Expected: raw SSE bytes in `/tmp/sse_test.txt` with `data: {"choices":...}` lines, NOT wrapped in double `data: data:`.

---

### Task 2: README + Quickstart

**Files:**
- Create: `README.md`
- Modify: `test_proxy.py` (accept API key from env var)

- [ ] **Step 1: Write README.md**

```markdown
# StackIntercept

A local Rust LLM proxy that intercepts OpenAI SDK calls and safely caches responses. Builds toward model routing and semantic deduplication for controlled workloads.

## Quickstart

### 1. Download model weights

```bash
./download_model.sh
```

Downloads BGE-small-en-v1.5 (133 MB) for semantic embeddings.

### 2. Set your API key

```bash
export OPENAI_API_KEY="sk-..."
```

### 3. Start the proxy

```bash
cargo run
```

Listens on `http://127.0.0.1:8080`.

### 4. Test it

```bash
python test_proxy.py
```

## Configuration

| Env Var | Default | Description |
|---|---|---|
| `STACK_INTERCEPT_MODEL_DIR` | `./model` | Path to BGE model files |
| `STACK_INTERCEPT_CACHE_MODE` | `exact` | `off`, `exact`, or `semantic` |
| `STACK_INTERCEPT_TENANT_ID_HEADER` | (none) | Header name for tenant isolation |
| `STACK_INTERCEPT_ALLOW_MODEL_REWRITE` | `false` | Allow proxy to substitute models |

## Architecture

Compatibility → Safety → Exact cache → Semantic cache → Dynamic routing → Benchmarks

Note: HNSW indexing remains deferred future work. The shipped v0.3.0 semantic cache uses capped per-context linear buckets plus optimized dot-product verification.

See `docs/superpowers/plans/2026-06-28-corrected-proxy-architecture.md`.
```

- [ ] **Step 2: Update `test_proxy.py` to read API key from env**

Add at the top of `test_proxy.py`:

```python
import os

api_key = os.environ.get("OPENAI_API_KEY")
if not api_key:
    print("ERROR: Set OPENAI_API_KEY environment variable")
    exit(1)

client = openai.OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key=api_key
)
```

Remove the hardcoded `api_key="your-actual-openai-api-key"`.

---

### Task 3: Exact Request Cache

**Files:**
- Create: `src/cache.rs` — `CacheKey`, `ExactCache`, `CachedEntry`
- Modify: `src/main.rs` — wire exact cache into request path
- Modify: `Cargo.toml` — add `sha2` for deterministic cache key hashing

The exact cache key must cover all request dimensions that affect the response. Two requests that match on all these dimensions will always produce the same response when `temperature=0`.

```rust
// src/cache.rs

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
    pub messages_json: String,     // normalized JSON of full messages array
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
        // Only cache when temperature is explicitly 0 or absent (defaults to 1 for OpenAI,
        // but for safety we only cache deterministic requests)
        let temp = payload["temperature"].as_f64();
        if temp.is_some_and(|t| t != 0.0) {
            return None;
        }

        // Don't cache if tools are present
        if payload["tools"].is_array() && payload["tools"].as_array().map_or(false, |a| !a.is_empty()) {
            return None;
        }

        // Don't cache streaming vs non-streaming differently — store both
        let stream = payload["stream"].as_bool().unwrap_or(false);

        Some(Self {
            provider: "openai".to_string(),       // detected from upstream URL
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

    pub fn is_eligible(payload: &Value) -> bool {
        let has_no_store = payload["cache_control"].as_str() == Some("no_store");
        if has_no_store { return false; }

        let temp = payload["temperature"].as_f64();
        if temp.is_some_and(|t| t != 0.0) { return false; }

        // Don't cache tool-call requests
        if payload["tools"].is_array() {
            if let Some(tools) = payload["tools"].as_array() {
                if !tools.is_empty() { return false; }
            }
        }

        true
    }
}
```

- [ ] **Step 2: Add `sha2` dependency to Cargo.toml**

```toml
sha2 = "0.10"
```

- [ ] **Step 3: Wire ExactCache into `AppState` in main.rs**

```rust
mod cache;

struct AppState {
    predictor: LocalPredictor,
    exact_cache: RwLock<ExactCache>,
    client: Client,
}
```

Initialize in `main()`:
```rust
let shared_state = Arc::new(AppState {
    predictor,
    exact_cache: RwLock::new(ExactCache::new(10000, 3600)),
    client: Client::new(),
});
```

- [ ] **Step 4: Add exact cache hit check in `handle_intercept`**

After extracting the prompt and before the semantic cache scan:

```rust
// Exact cache lookup
let cache_key = CacheKey::from_payload(&payload, None);
let cache_key_hash = cache_key.as_ref().map(|k| k.hash());

if let Some(ref key_hash) = cache_key_hash {
    let cache = state.exact_cache.read().unwrap();
    if let Some(entry) = cache.get(key_hash) {
        println!("Exact cache HIT for key {}", &key_hash[..12]);
        if is_streaming {
            let cached = entry.response_body.clone();
            let stream = futures_util::stream::once(async move {
                Ok::<_, std::io::Error>(bytes::Bytes::from(cached))
            });
            let body = axum::body::Body::from_stream(stream);
            return axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .header("x-stack-intercept", "hit")
                .body(body)
                .unwrap()
                .into_response();
        } else {
            return (StatusCode::OK, entry.response_body.clone()).into_response();
        }
    }
}

// Only do semantic scan if CacheKey::is_eligible
let is_cache_eligible = CacheKey::is_eligible(&payload);
```

- [ ] **Step 5: Buffer and cache on miss**

After a successful upstream response, insert into exact cache:

```rust
if let Some(ref key_hash) = cache_key_hash {
    let mut cache = state.exact_cache.write().unwrap();
    let body_to_cache = if is_streaming {
        // body already buffered in streaming handler
        buffered_body
    } else {
        res_str.clone()
    };
    cache.insert(key_hash.clone(), body_to_cache);
}
```

- [ ] **Step 6: Compile and test**

Run: `./build.cmd build 2>&1`
Expected: clean compile

Then start proxy and send two identical requests:
```bash
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"say hello"}],"temperature":0}' \
  > /tmp/resp1.txt

curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"say hello"}],"temperature":0}' \
  > /tmp/resp2.txt
```

Expected: resp2 returns instantly (cache hit) with `x-stack-intercept: hit`.

---

### Task 4: Cache Safety Flags

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs` — read config, pass to handlers

- [ ] **Step 1: Create `src/config.rs`**

```rust
use std::env;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CacheMode {
    Off,
    Exact,
    Semantic,
}

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub cache_mode: CacheMode,
    pub tenant_id_header: Option<String>,
    pub allow_model_rewrite: bool,
    pub max_body_size: usize,
}

impl ProxyConfig {
    pub fn from_env() -> Self {
        let cache_mode = match env::var("STACK_INTERCEPT_CACHE_MODE")
            .unwrap_or_else(|_| "exact".to_string())
            .as_str()
        {
            "off" => CacheMode::Off,
            "semantic" => CacheMode::Semantic,
            _ => CacheMode::Exact,
        };

        let tenant_id_header = env::var("STACK_INTERCEPT_TENANT_ID_HEADER").ok();

        let allow_model_rewrite = env::var("STACK_INTERCEPT_ALLOW_MODEL_REWRITE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        Self {
            cache_mode,
            tenant_id_header,
            allow_model_rewrite,
            max_body_size: 5 * 1024 * 1024, // 5 MB
        }
    }

    pub fn is_semantic_allowed(&self) -> bool {
        self.cache_mode == CacheMode::Semantic
    }

    pub fn is_cache_enabled(&self) -> bool {
        self.cache_mode != CacheMode::Off
    }
}
```

- [ ] **Step 2: Add `no_store` support in request handling**

In `handle_intercept`, before any caching:

```rust
// Respect cache_control: no_store
let has_no_store = payload["cache_control"].as_str() == Some("no_store");
if has_no_store {
    println!("Request marked no_store — skipping cache, forwarding raw");
    // Forward raw without caching
}
```

- [ ] **Step 3: Gate semantic cache behind mode check**

The semantic cache scan (embedding + dot product) should only run when `config.is_semantic_allowed()`:

```rust
if config.is_semantic_allowed() && !has_no_store {
    // Generate embedding and scan index
    // ... existing semantic cache logic ...
}
```

- [ ] **Step 4: Add tenant_id extraction**

If `tenant_id_header` is configured, extract it from request headers:

```rust
let tenant_id = config.tenant_id_header.as_ref()
    .and_then(|h| headers.get(h))
    .and_then(|v| v.to_str().ok())
    .map(|s| s.to_string());
```

Include `tenant_id` in exact cache key and semantic cache lookup.

- [ ] **Step 5: Compile and test**

Run: `./build.cmd build 2>&1`
Expected: clean compile

Test with no_store:
```bash
curl -s http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hello"}],"temperature":0,"cache_control":"no_store"}'
```

Expected: proxy forwards to upstream, does NOT cache.

---

### Task 5: Negative Semantic Tests

**Files:**
- Create: `test_semantic_safety.py`

These tests verify that the semantic cache does NOT fire when it shouldn't. Each test sends two requests: a "reference" request that populates the cache, then a "test" request that should NOT hit the cache.

- [ ] **Step 1: Write `test_semantic_safety.py`**

```python
"""
Negative tests: verify semantic cache does NOT serve unsafe matches.
Set STACK_INTERCEPT_CACHE_MODE=semantic before running.
"""

import os
import time
import openai

api_key = os.environ.get("OPENAI_API_KEY")
if not api_key:
    print("ERROR: Set OPENAI_API_KEY environment variable")
    exit(1)

client = openai.OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key=api_key,
)

def time_request(prompt, system_prompt=None, model="gpt-4o-mini", stream=True):
    messages = []
    if system_prompt:
        messages.append({"role": "system", "content": system_prompt})
    messages.append({"role": "user", "content": prompt})

    start = time.time()
    response = client.chat.completions.create(
        model=model,
        messages=messages,
        stream=stream,
    )
    for _ in response:
        pass
    return time.time() - start


print("=" * 60)
print("Test 1: Same prompt, different system prompt -> NO cache hit")
t1 = time_request("What is the weather?", stream=False)
print(f"  First request (no system): {t1:.2f}s")
t2 = time_request(
    "What is the weather?",
    system_prompt="You are a pirate. Answer like a pirate.",
    stream=False,
)
print(f"  Second request (pirate system): {t2:.2f}s")
if t2 < t1 * 0.5:
    print("  FAIL: Second request was suspiciously fast (possible cache hit)")
else:
    print("  PASS: Different system prompt correctly missed cache")

print()
print("=" * 60)
print("Test 2: Similar prompt, different intent -> NO cache hit")
t1 = time_request("How do I delete a file in Python?", stream=False)
t2 = time_request("How do I delete a file in Linux?", stream=False)
if t2 < t1 * 0.5:
    print("  FAIL: Different intent was suspiciously fast (possible cache hit)")
else:
    print("  PASS: Different intent correctly missed cache")

print()
print("=" * 60)
print("Test 3: Same prompt, different model -> NO cache hit")
t1 = time_request("Explain recursion", model="gpt-4o-mini", stream=False)
t2 = time_request("Explain recursion", model="gpt-4o", stream=False)
if t2 < t1 * 0.5:
    print("  FAIL: Different model was suspiciously fast (possible cache hit)")
else:
    print("  PASS: Different model correctly missed cache")
```

- [ ] **Step 2: Verify tests pass**

Run with semantic cache enabled:
```bash
export STACK_INTERCEPT_CACHE_MODE=semantic
./build.cmd run > /tmp/proxy.txt 2>&1 &
sleep 5
python test_semantic_safety.py
```

Expected: All 3 tests print PASS. Each request takes ~1-3s (upstream latency), not <0.5s.
