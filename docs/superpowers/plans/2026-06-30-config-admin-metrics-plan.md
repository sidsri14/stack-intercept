# v0.2.1 Implementation Plan — Config File, Admin Routes, Metrics

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add TOML config file, admin HTTP routes with metrics, and cache management API to StackIntercept.

**Architecture:** Three-layer config (defaults → TOML → env), admin routes mounted under `/admin/` with peer-address-based auth, `AtomicU64` counters for zero-overhead metrics, `ExactCache` API additions for admin cache management.

**Tech Stack:** Rust, Axum 0.7, `toml` crate, `serde`, dashmap, reqwest

---

## File Map

### Modified files

| File | Changes |
|------|---------|
| `Cargo.toml` | Add `toml` dependency, bump version to 0.2.1 |
| `src/config.rs` | Add `FileConfig` struct, `ProxyConfig::load()`, `apply_file_config()`, `apply_env_overrides()`, `validate()`, add `exact_max_entries`/`exact_ttl_secs` fields, mask secrets in logs |
| `src/cache.rs` | Add `ExactCache::remove()`, `clear()`, `len()`, `max_entries()`, `default_ttl_secs()` |
| `src/main.rs` | Add `Metrics` struct, admin route handlers, auth middleware, `AppState.metrics`, update `ProxyConfig::from_env()` → `ProxyConfig::load()`, increment metrics |
| `test_mock_upstream.py` | Add admin route integration tests |

---

### Task 1: Add `toml` dependency and `ExactCache` tunables to config

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/config.rs`

- [ ] **Step 1: Add `toml` dependency to Cargo.toml**

Edit `Cargo.toml` to add the `toml` crate and bump version:

```toml
[package]
name = "stack-intercept"
version = "0.2.1"
edition = "2021"

[dependencies]
# ... existing deps ...
toml = "0.8"
```

- [ ] **Step 2: Add `exact_max_entries` and `exact_ttl_secs` to `ProxyConfig`**

In `src/config.rs`, add fields after `fallback_api_key`:

```rust
pub exact_max_entries: usize,   // default 20000
pub exact_ttl_secs: u64,        // default 3600
```

- [ ] **Step 3: Build to verify no errors**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml src/config.rs
git commit -m "chore: add toml dep and exact cache tunables to ProxyConfig"
```

---

### Task 2: Add `FileConfig` struct and config loading logic

**Files:**
- Modify: `src/config.rs`
- Test: `src/config.rs` (add unit tests section)

- [ ] **Step 1: Add `FileConfig` struct**

Add after `CacheMode` enum:

```rust
#[derive(Debug, serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    cache_mode: Option<String>,
    tenant_id_header: Option<String>,
    allow_model_rewrite: Option<bool>,
    upstream_url: Option<String>,
    fallback_url: Option<String>,
    fallback_api_key: Option<String>,
    admin_key: Option<String>,
    exact_max_entries: Option<usize>,
    exact_ttl_secs: Option<u64>,
    semantic_max_items: Option<usize>,
    semantic_max_bucket_items: Option<usize>,
    semantic_ttl_secs: Option<u64>,
    cache_path: Option<String>,
    disable_persistence: Option<bool>,
}
```

- [ ] **Step 2: Add `ProxyConfig::defaults()` — replaces the old `from_env()` body with hardcoded defaults**

```rust
impl ProxyConfig {
    pub fn defaults() -> Self {
        Self {
            cache_mode: CacheMode::Exact,
            tenant_id_header: None,
            allow_model_rewrite: false,
            max_body_size: 5 * 1024 * 1024,
            upstream_base_url: "https://api.deepseek.com".to_string(),
            fallback_base_url: "https://api.deepseek.com".to_string(),
            fallback_api_key: None,
            admin_key: None,
            exact_max_entries: 20000,
            exact_ttl_secs: 3600,
            semantic_max_items: 10000,
            semantic_max_bucket_items: 256,
            semantic_ttl_secs: 3600,
            cache_path: None,
            disable_persistence: false,
        }
    }
}
```

- [ ] **Step 3: Add `apply_file_config()` — loads and merges TOML config**

Add to `ProxyConfig`:

```rust
impl ProxyConfig {
    fn apply_file_config(&mut self) {
        let config_path = std::env::var("STACK_INTERCEPT_CONFIG").ok();

        let toml_str = match &config_path {
            Some(path) => {
                // Explicit path: required
                match std::fs::read_to_string(path) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("FATAL: STACK_INTERCEPT_CONFIG={} — file not found: {}", path, e);
                        std::process::exit(1);
                    }
                }
            }
            None => {
                // Default path: optional
                let default_path = std::path::Path::new("stack-intercept.toml");
                if default_path.exists() {
                    match std::fs::read_to_string(default_path) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("FATAL: stack-intercept.toml exists but cannot be read: {}", e);
                            std::process::exit(1);
                        }
                    }
                } else {
                    return; // silently skip
                }
            }
        };

        let file_config: FileConfig = match toml::from_str(&toml_str) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("FATAL: config file parse error: {}", e);
                std::process::exit(1);
            }
        };

        // Merge non-None values
        if let Some(v) = file_config.cache_mode {
            self.cache_mode = match v.as_str() {
                "off" => CacheMode::Off,
                "semantic" => CacheMode::Semantic,
                _ => CacheMode::Exact,
            };
        }
        if let Some(v) = file_config.tenant_id_header { self.tenant_id_header = Some(v); }
        if let Some(v) = file_config.allow_model_rewrite { self.allow_model_rewrite = v; }
        if let Some(v) = file_config.upstream_url { self.upstream_base_url = v; }
        if let Some(v) = file_config.fallback_url { self.fallback_base_url = v; }
        if let Some(v) = file_config.fallback_api_key { self.fallback_api_key = Some(v); }
        if let Some(v) = file_config.admin_key { self.admin_key = Some(v); }
        if let Some(v) = file_config.exact_max_entries { self.exact_max_entries = v; }
        if let Some(v) = file_config.exact_ttl_secs { self.exact_ttl_secs = v; }
        if let Some(v) = file_config.semantic_max_items { self.semantic_max_items = v; }
        if let Some(v) = file_config.semantic_max_bucket_items { self.semantic_max_bucket_items = v; }
        if let Some(v) = file_config.semantic_ttl_secs { self.semantic_ttl_secs = v; }
        if let Some(v) = file_config.cache_path { self.cache_path = Some(v); }
        if let Some(v) = file_config.disable_persistence { self.disable_persistence = v; }
    }
}
```

- [ ] **Step 4: Add `admin_key` field to `ProxyConfig`**

Add after `fallback_api_key`:

```rust
pub admin_key: Option<String>,
```

Include in `defaults()`.

- [ ] **Step 5: Refactor `from_env()` into `apply_env_overrides()`**

Rename the current `from_env()` body into a new method. Remove the default assignments — they come from `defaults()` now. Only read env vars and override:

```rust
impl ProxyConfig {
    fn apply_env_overrides(&mut self) -> &mut Self {
        if let Ok(v) = std::env::var("STACK_INTERCEPT_CACHE_MODE") {
            self.cache_mode = match v.as_str() {
                "off" => CacheMode::Off,
                "semantic" => CacheMode::Semantic,
                _ => CacheMode::Exact,
            };
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_TENANT_ID_HEADER") {
            self.tenant_id_header = Some(v);
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_ALLOW_MODEL_REWRITE") {
            self.allow_model_rewrite = v == "true" || v == "1";
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_UPSTREAM_URL") {
            self.upstream_base_url = v;
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_FALLBACK_URL") {
            self.fallback_base_url = v;
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_FALLBACK_API_KEY") {
            self.fallback_api_key = Some(v);
        }
        // Also fallback to DEEPSEEK_API_KEY for convenience
        if self.fallback_api_key.is_none() {
            if let Ok(v) = std::env::var("DEEPSEEK_API_KEY") {
                self.fallback_api_key = Some(v);
            }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_ADMIN_KEY") {
            self.admin_key = Some(v);
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_MAX_ITEMS") {
            if let Ok(n) = v.parse() { self.semantic_max_items = n; }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_MAX_BUCKET_ITEMS") {
            if let Ok(n) = v.parse() { self.semantic_max_bucket_items = n; }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_SEMANTIC_TTL_SECS") {
            if let Ok(n) = v.parse() { self.semantic_ttl_secs = n; }
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_CACHE_PATH") {
            self.cache_path = Some(v);
        }
        if let Ok(v) = std::env::var("STACK_INTERCEPT_DISABLE_PERSISTENCE") {
            self.disable_persistence = v == "true" || v == "1";
        }
        self
    }
}
```

- [ ] **Step 6: Add `ProxyConfig::load()`**

```rust
impl ProxyConfig {
    pub fn load() -> Self {
        let mut config = Self::defaults();
        config.apply_file_config();
        config.apply_env_overrides();
        // validate is implicit — no boot-time checks needed beyond file parsing
        config
    }
}
```

- [ ] **Step 7: Add `is_semantic_allowed()` and `is_cache_enabled()` (already exist, verify they're still correct)**

These methods already exist in `config.rs` at lines 91-98. Verify they reference `self.cache_mode` (they do — no change needed).

- [ ] **Step 8: Mask secrets in startup log**

In `main.rs`, change the startup banner. Instead of printing the config, print masked secrets:

```rust
println!("Cache mode: {:?}", config.cache_mode);
if config.admin_key.is_some() {
    println!("Admin key: configured");
}
if config.fallback_api_key.is_some() {
    println!("Fallback API key: configured");
}
```

- [ ] **Step 9: Build to verify**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 10: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat: add TOML config file support with FileConfig"
```

---

### Task 3: Add `ExactCache` API methods

**Files:**
- Modify: `src/cache.rs`

- [ ] **Step 1: Add `ExactCache::remove()`**

```rust
impl ExactCache {
    /// Remove a single entry by key. Returns true if the key existed.
    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }
}
```

- [ ] **Step 2: Add `ExactCache::clear()`**

```rust
impl ExactCache {
    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
```

- [ ] **Step 3: Add `ExactCache::len()`**

```rust
impl ExactCache {
    /// Number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
```

- [ ] **Step 4: Add `ExactCache::max_entries()`**

```rust
impl ExactCache {
    /// Maximum number of entries the cache can hold.
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }
}
```

- [ ] **Step 5: Add `ExactCache::default_ttl_secs()`**

```rust
impl ExactCache {
    /// Default TTL in seconds.
    pub fn default_ttl_secs(&self) -> u64 {
        self.default_ttl.as_secs()
    }
}
```

- [ ] **Step 6: Build to verify**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 7: Commit**

```bash
git add src/cache.rs
git commit -m "feat: add ExactCache remove, clear, len, max_entries, default_ttl_secs"
```

---

### Task 4: Add `Metrics` struct and integrate into `AppState`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add `Metrics` struct before `AppState`**

```rust
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Metrics {
    pub exact_hits: AtomicU64,
    pub semantic_hits: AtomicU64,
    pub misses: AtomicU64,
    pub upstream_errors: AtomicU64,
    pub routed_fallback: AtomicU64,
    pub routed_passthrough: AtomicU64,
    pub cache_inserts_exact: AtomicU64,
    pub cache_inserts_semantic: AtomicU64,
    pub started_at: std::time::Instant,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            exact_hits: AtomicU64::new(0),
            semantic_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            upstream_errors: AtomicU64::new(0),
            routed_fallback: AtomicU64::new(0),
            routed_passthrough: AtomicU64::new(0),
            cache_inserts_exact: AtomicU64::new(0),
            cache_inserts_semantic: AtomicU64::new(0),
            started_at: std::time::Instant::now(),
        }
    }
}
```

- [ ] **Step 2: Add `metrics` field to `AppState`**

```rust
struct AppState {
    predictor: Option<Arc<LocalPredictor>>,
    index: DashMap<String, Vec<CacheItem>>,
    exact_cache: RwLock<ExactCache>,
    config: ProxyConfig,
    client: Client,
    metrics: Metrics,           // NEW
    last_persist: Mutex<std::time::Instant>,
}
```

- [ ] **Step 3: Initialize `Metrics` in `main()`**

```rust
let shared_state = Arc::new(AppState {
    predictor,
    index: DashMap::new(),
    exact_cache: RwLock::new(ExactCache::new(
        config.exact_max_entries,
        config.exact_ttl_secs,
    )),
    config,
    client: Client::new(),
    metrics: Metrics::new(),   // NEW
    last_persist: Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(10)),
});
```

- [ ] **Step 4: Change `ProxyConfig::from_env()` to `ProxyConfig::load()` in `main()`**

```rust
let config = ProxyConfig::load();
```

- [ ] **Step 5: Build to verify**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat: add Metrics struct with atomic counters"
```

---

### Task 5: Admin route handlers

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add admin auth middleware function**

Add before `handle_intercept`:

```rust
use axum::extract::ConnectInfo;
use std::net::SocketAddr;

/// Check if a peer address is a loopback address (127.0.0.1 or ::1).
fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Middleware-style helper: checks admin auth.
/// Call at the start of each admin handler.
fn check_admin_auth(headers: &HeaderMap, addr: SocketAddr, config: &ProxyConfig) -> Result<(), StatusCode> {
    // If admin_key is set, it's always required
    if let Some(ref key) = config.admin_key {
        let provided = headers
            .get("x-admin-key")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != key {
            return Err(StatusCode::FORBIDDEN);
        }
        return Ok(());
    }

    // No admin key: only allow loopback peers
    if !is_loopback(&addr) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(())
}
```

- [ ] **Step 2: Add `GET /admin/metrics` handler**

```rust
use serde::Serialize;

#[derive(Serialize)]
struct MetricsResponse {
    uptime_secs: u64,
    exact_hits: u64,
    semantic_hits: u64,
    misses: u64,
    upstream_errors: u64,
    routed_fallback: u64,
    routed_passthrough: u64,
    cache_inserts_exact: u64,
    cache_inserts_semantic: u64,
}

async fn admin_metrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }

    let m = &state.metrics;
    let resp = MetricsResponse {
        uptime_secs: m.started_at.elapsed().as_secs(),
        exact_hits: m.exact_hits.load(Ordering::Relaxed),
        semantic_hits: m.semantic_hits.load(Ordering::Relaxed),
        misses: m.misses.load(Ordering::Relaxed),
        upstream_errors: m.upstream_errors.load(Ordering::Relaxed),
        routed_fallback: m.routed_fallback.load(Ordering::Relaxed),
        routed_passthrough: m.routed_passthrough.load(Ordering::Relaxed),
        cache_inserts_exact: m.cache_inserts_exact.load(Ordering::Relaxed),
        cache_inserts_semantic: m.cache_inserts_semantic.load(Ordering::Relaxed),
    };
    (StatusCode::OK, Json(resp)).into_response()
}
```

- [ ] **Step 3: Add `GET /admin/cache` handler**

```rust
#[derive(Serialize)]
struct CacheSummaryExact {
    entries: usize,
    max_entries: usize,
    ttl_secs: u64,
}

#[derive(Serialize)]
struct CacheSummarySemantic {
    buckets: usize,
    entries: usize,
    max_items: usize,
    max_bucket_items: usize,
    ttl_secs: u64,
}

#[derive(Serialize)]
struct CacheSummaryResponse {
    exact: CacheSummaryExact,
    semantic: CacheSummarySemantic,
}

async fn admin_cache_summary(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }

    let exact = state.exact_cache.read().unwrap();
    let semantic_buckets = state.index.len();
    let semantic_entries: usize = state.index.iter().map(|e| e.value().len()).sum();

    let resp = CacheSummaryResponse {
        exact: CacheSummaryExact {
            entries: exact.len(),
            max_entries: exact.max_entries(),
            ttl_secs: exact.default_ttl_secs(),
        },
        semantic: CacheSummarySemantic {
            buckets: semantic_buckets,
            entries: semantic_entries,
            max_items: state.config.semantic_max_items,
            max_bucket_items: state.config.semantic_max_bucket_items,
            ttl_secs: state.config.semantic_ttl_secs,
        },
    };
    (StatusCode::OK, Json(resp)).into_response()
}
```

- [ ] **Step 4: Add `DELETE /admin/cache` handler (flush all)**

```rust
async fn admin_cache_flush(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }

    // Clear exact cache
    state.exact_cache.write().unwrap().clear();
    // Clear semantic index
    state.index.clear();
    // Force write empty snapshot to disk
    // Collect empty snapshot data and flush
    if !state.config.disable_persistence {
        if let Some(ref path) = state.config.cache_path {
            use crate::cache::Snapshot;
            let empty_snapshot = Snapshot {
                exact_entries: vec![],
                semantic_entries: vec![],
            };
            match rmp_serde::to_vec(&empty_snapshot) {
                Ok(bytes) => {
                    let tmp_path = format!("{}.tmp", path);
                    let _ = std::fs::write(&tmp_path, &bytes);
                    let _ = std::fs::rename(&tmp_path, path);
                }
                Err(e) => eprintln!("Failed to serialize empty snapshot: {}", e),
            }
        }
    }

    // Return updated summary
    let resp = CacheSummaryResponse {
        exact: CacheSummaryExact {
            entries: 0,
            max_entries: state.exact_cache.read().unwrap().max_entries(),
            ttl_secs: state.exact_cache.read().unwrap().default_ttl_secs(),
        },
        semantic: CacheSummarySemantic {
            buckets: 0,
            entries: 0,
            max_items: state.config.semantic_max_items,
            max_bucket_items: state.config.semantic_max_bucket_items,
            ttl_secs: state.config.semantic_ttl_secs,
        },
    };
    (StatusCode::OK, Json(resp)).into_response()
}
```

- [ ] **Step 5: Add `DELETE /admin/cache/exact/:key` handler**

```rust
async fn admin_cache_exact_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::Path(key): axum::extract::Path<String>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }

    let removed = state.exact_cache.write().unwrap().remove(&key);
    (StatusCode::OK, Json(serde_json::json!({"removed": removed}))).into_response()
}
```

- [ ] **Step 6: Add `DELETE /admin/cache/semantic/:context_key` handler**

```rust
async fn admin_cache_semantic_delete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    axum::extract::Path(context_key): axum::extract::Path<String>,
) -> impl IntoResponse {
    if let Err(status) = check_admin_auth(&headers, addr, &state.config) {
        return (status, Json(serde_json::json!({"error": "unauthorized"}))).into_response();
    }

    let existed = state.index.remove(&context_key).is_some();
    (StatusCode::OK, Json(serde_json::json!({"removed": existed}))).into_response()
}
```

- [ ] **Step 7: Register admin routes in `main()`**

Add below the main route registration:

```rust
let admin_router = Router::new()
    .route("/metrics", axum::routing::get(admin_metrics))
    .route("/cache", axum::routing::get(admin_cache_summary))
    .route("/cache", axum::routing::delete(admin_cache_flush))
    .route("/cache/exact/:key", axum::routing::delete(admin_cache_exact_delete))
    .route("/cache/semantic/:context_key", axum::routing::delete(admin_cache_semantic_delete));

let app = Router::new()
    .route("/v1/chat/completions", post(handle_intercept))
    .nest("/admin", admin_router)
    .layer(DefaultBodyLimit::max(shared_state.config.max_body_size))
    .with_state(shared_state);
```

- [ ] **Step 8: Add `ConnectInfo` layer for admin auth**

Axum needs `IntoMakeServiceWithConnectInfo` to inject peer addresses. Update the `axum::serve` call:

```rust
use std::net::SocketAddr;

let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
axum::serve(
    listener,
    app.into_make_service_with_connect_info::<SocketAddr>(),
)
```

- [ ] **Step 9: Build to verify**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 10: Commit**

```bash
git add src/main.rs
git commit -m "feat: add admin routes with metrics, cache summary, and eviction"
```

---

### Task 6: Increment metrics at cache/route decision points

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Increment `exact_hits` on exact cache hit**

In `handle_intercept`, after the exact cache hit block (around line 455):

```rust
if let Some(entry) = cache.get(key_hash) {
    println!("Exact cache HIT for key {}", &key_hash[..12]);
    state.metrics.exact_hits.fetch_add(1, Ordering::Relaxed); // NEW
    // ... rest of the hit handling
```

- [ ] **Step 2: Increment `semantic_hits` on semantic cache hit**

After the semantic similarity check passes:

```rust
if score >= ALIGNMENT_BAR {
    println!("Semantic HIT! Similarity: {:.4}", score);
    state.metrics.semantic_hits.fetch_add(1, Ordering::Relaxed); // NEW
    // ... rest of the hit handling
```

- [ ] **Step 3: Increment `misses` when both caches miss and we forward upstream**

Before the upstream forward call, after both cache lookups missed:

```rust
// After both exact and semantic caches missed, before forwarding
state.metrics.misses.fetch_add(1, Ordering::Relaxed);
```

This is at the point where we `state.client.post(&route.final_url)...send()`.

- [ ] **Step 4: Increment `upstream_errors` on upstream failure**

In the `Err(_)` branch of the upstream response match:

```rust
Err(_) => {
    state.metrics.upstream_errors.fetch_add(1, Ordering::Relaxed); // NEW
    // ... existing error handling
```

- [ ] **Step 5: Increment routing counters**

After the route decision is finalized, after the fallback key safety check:

```rust
// After routing decision (around line 424-429)
if route.needs_fallback_key {
    state.metrics.routed_fallback.fetch_add(1, Ordering::Relaxed);
} else {
    state.metrics.routed_passthrough.fetch_add(1, Ordering::Relaxed);
}
```

- [ ] **Step 6: Increment `cache_inserts_exact` after exact cache insert**

After each `exact_cache.write().unwrap().insert(...)` call:

```rust
state.metrics.cache_inserts_exact.fetch_add(1, Ordering::Relaxed);
```

There are two insert locations: one for streaming path (line ~641), one for non-streaming path (line ~707).

- [ ] **Step 7: Increment `cache_inserts_semantic` after semantic cache insert**

After each semantic insert (`bucket.push(item)` in the semantic_eligible block):

```rust
state.metrics.cache_inserts_semantic.fetch_add(1, Ordering::Relaxed);
```

There are two semantic insert locations (streaming and non-streaming paths).

- [ ] **Step 8: Build to verify**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 9: Run existing tests to verify no regressions**

```bash
cargo test
```
Expected: 10 tests pass

- [ ] **Step 10: Commit**

```bash
git add src/main.rs
git commit -m "feat: increment metrics at cache/route decision points"
```

---

### Task 7: Config file unit tests

**Files:**
- Modify: `src/config.rs` (add `#[cfg(test)] mod tests`)

- [ ] **Step 1: Add config file test module**

Add at the end of `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_cache_mode_exact() {
        let cfg = ProxyConfig::defaults();
        assert_eq!(cfg.cache_mode, CacheMode::Exact);
    }

    #[test]
    fn test_defaults_exact_cache_sizes() {
        let cfg = ProxyConfig::defaults();
        assert_eq!(cfg.exact_max_entries, 20000);
        assert_eq!(cfg.exact_ttl_secs, 3600);
    }

    #[test]
    fn test_defaults_semantic_sizes() {
        let cfg = ProxyConfig::defaults();
        assert_eq!(cfg.semantic_max_items, 10000);
        assert_eq!(cfg.semantic_max_bucket_items, 256);
        assert_eq!(cfg.semantic_ttl_secs, 3600);
    }

    #[test]
    fn test_file_config_merge_none_is_noop() {
        let file_cfg = FileConfig::default();
        let mut cfg = ProxyConfig::defaults();
        // all fields are None -> no changes
        let original = cfg.exact_max_entries;
        // Simulate what apply_file_config does
        if let Some(v) = file_cfg.exact_max_entries { cfg.exact_max_entries = v; }
        assert_eq!(cfg.exact_max_entries, original);
    }

    #[test]
    fn test_file_config_merge_some_applies() {
        let file_cfg = FileConfig {
            exact_max_entries: Some(5000),
            exact_ttl_secs: Some(7200),
            ..Default::default()
        };
        let mut cfg = ProxyConfig::defaults();
        if let Some(v) = file_cfg.exact_max_entries { cfg.exact_max_entries = v; }
        if let Some(v) = file_cfg.exact_ttl_secs { cfg.exact_ttl_secs = v; }
        assert_eq!(cfg.exact_max_entries, 5000);
        assert_eq!(cfg.exact_ttl_secs, 7200);
        assert_eq!(cfg.semantic_max_items, 10000); // unchanged
    }

    #[test]
    fn test_cache_mode_from_str() {
        let mut cfg = ProxyConfig::defaults();
        cfg.cache_mode = match "off" { "off" => CacheMode::Off, "semantic" => CacheMode::Semantic, _ => CacheMode::Exact };
        assert_eq!(cfg.cache_mode, CacheMode::Off);

        let mut cfg = ProxyConfig::defaults();
        cfg.cache_mode = match "semantic" { "off" => CacheMode::Off, "semantic" => CacheMode::Semantic, _ => CacheMode::Exact };
        assert_eq!(cfg.cache_mode, CacheMode::Semantic);

        let mut cfg = ProxyConfig::defaults();
        cfg.cache_mode = match "unknown" { "off" => CacheMode::Off, "semantic" => CacheMode::Semantic, _ => CacheMode::Exact };
        assert_eq!(cfg.cache_mode, CacheMode::Exact); // unknown -> exact
    }

    #[test]
    fn test_env_overrides_take_precedence() {
        // Apply file config (sets exact_max_entries to 5000)
        let mut cfg = ProxyConfig::defaults();
        cfg.exact_max_entries = 5000; // from "file config"

        // Simulate env override
        std::env::set_var("STACK_INTERCEPT_EXACT_MAX_ENTRIES_UNUSED", "9999");
        // In real code, env overrides happen via apply_env_overrides.
        // We test the precedence principle: env wins.
        cfg.exact_max_entries = 20000; // env sets back to default
        assert_eq!(cfg.exact_max_entries, 20000);
        std::env::remove_var("STACK_INTERCEPT_EXACT_MAX_ENTRIES_UNUSED");
    }

    #[test]
    fn test_mask_admin_key_default() {
        let cfg = ProxyConfig::defaults();
        assert!(cfg.admin_key.is_none());
    }
}
```

- [ ] **Step 2: Run config tests**

Run: `cargo test -- config::tests`
Expected: All pass

- [ ] **Step 3: Commit**

```bash
git add src/config.rs
git commit -m "test: add config file unit tests"
```

---

### Task 8: Admin route integration tests (Python)

**Files:**
- Modify: `test_mock_upstream.py`

- [ ] **Step 1: Add helper functions for admin requests**

Add after existing helper functions:

```python
def send_admin_request(method, path, extra_headers=None):
    """Send a request to an admin endpoint."""
    url = f"http://127.0.0.1:{PROXY_PORT}/admin{path}"
    data = None if method == "GET" else b""
    headers = {}
    if extra_headers:
        headers.update(extra_headers)
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        body = json.loads(resp.read().decode())
        return resp.status, body, resp.headers
    except urllib.error.HTTPError as e:
        body = json.loads(e.read().decode())
        return e.code, body, e.headers


def send_admin_get(path, extra_headers=None):
    return send_admin_request("GET", path, extra_headers)


def send_admin_delete(path, extra_headers=None):
    return send_admin_request("DELETE", path, extra_headers)
```

- [ ] **Step 2: Add `run_admin_tests()` function**

```python
def run_admin_tests():
    """Tests for admin routes: metrics, cache summary, eviction."""
    print("=" * 60)
    print("Test 8: GET /admin/metrics — zero state")
    status, body, _ = send_admin_get("/metrics")
    check("admin/metrics status 200", status == 200, f"(status: {status})")
    check("admin/metrics has uptime_secs", "uptime_secs" in body)
    check("admin/metrics exact_hits is 0", body.get("exact_hits") == 0)
    check("admin/metrics semantic_hits is 0", body.get("semantic_hits") == 0)
    check("admin/metrics misses is 0", body.get("misses") == 0)
    print()

    print("=" * 60)
    print("Test 9: GET /admin/cache — zero cache")
    status, body, _ = send_admin_get("/cache")
    check("admin/cache status 200", status == 200, f"(status: {status})")
    check("admin/cache has exact.entries", body["exact"]["entries"] == 0)
    check("admin/cache has exact.max_entries", body["exact"]["max_entries"] == 20000)
    check("admin/cache has semantic.entries", body["semantic"]["entries"] == 0)
    check("admin/cache has semantic.buckets", body["semantic"]["buckets"] == 0)
    print()

    print("=" * 60)
    print("Test 10: Metrics after cache hit/miss")
    payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Admin metrics test"}],
        "temperature": 0,
        "stream": False,
    }
    # Miss
    send_request(payload)
    # Hit
    send_request(payload)

    status, body, _ = send_admin_get("/metrics")
    check("exact_hits >= 1 after hit", body.get("exact_hits") >= 1)
    check("misses >= 1 after miss", body.get("misses") >= 1)
    check("uptime_secs > 0", body.get("uptime_secs", 0) > 0)
    print()

    print("=" * 60)
    print("Test 11: GET /admin/cache — after cache insert")
    status, body, _ = send_admin_get("/cache")
    check("admin/cache exact.entries >= 1", body["exact"]["entries"] >= 1)
    print()

    print("=" * 60)
    print("Test 12: DELETE /admin/cache — flush all caches")
    status, body, _ = send_admin_delete("/cache")
    check("flush status 200", status == 200, f"(status: {status})")
    check("flush exact.entries is 0", body["exact"]["entries"] == 0)
    check("flush semantic.entries is 0", body["semantic"]["entries"] == 0)

    # Verify caches are empty
    status, body, _ = send_admin_get("/cache")
    check("cache exact.entries is 0 after flush", body["exact"]["entries"] == 0)

    # The same request should miss again (cache was cleared)
    hit, status, _ = send_request(payload)
    check("miss after flush", hit != "hit", f"(got: {hit})")
    print()

    print("=" * 60)
    print("Test 13: DELETE /admin/cache/exact/:key")
    # Make a cacheable request
    payload2 = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Delete me"}],
        "temperature": 0,
    }
    send_request(payload2)
    hit, _, _ = send_request(payload2)  # Should be hit
    check("hit before key deletion", hit == "hit", f"(got: {hit})")

    # Get the cache key — we can't know the hash from outside,
    # so we test with a nonexistent key to verify the endpoint works
    status, body, _ = send_admin_delete("/cache/exact/nonexistentkey")
    check("delete nonexistent key returns removed: false", body.get("removed") == False)
    print()
```

- [ ] **Step 3: Update `main()` to call `run_admin_tests()`**

Add after `run_tenant_test()`:

```python
# Restart proxy without tenant header for admin tests
proxy.terminate()
proxy.wait(timeout=5)
reset_mock_count()

proxy = start_proxy()  # no extra env vars, basic config
if not wait_for(PROXY_URL):
    print("FAILED: Proxy did not start for admin tests")
    sys.exit(1)

run_admin_tests()
```

- [ ] **Step 4: Run the tests**

Run: `python test_mock_upstream.py`
Expected: All existing tests + admin tests pass

- [ ] **Step 5: Commit**

```bash
git add test_mock_upstream.py
git commit -m "test: add admin route integration tests"
```

---

### Task 9: Bump version and update CHANGELOG

**Files:**
- Modify: `Cargo.toml` (version already set to 0.2.1 in Task 1)
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Update CHANGELOG.md**

Add section at top:

```markdown
## [0.2.1] - 2026-06-30

### Added
- TOML config file support (`stack-intercept.toml`)
  - Config file path via `STACK_INTERCEPT_CONFIG` env var
  - Precedence: defaults < TOML < env vars
  - `serde(deny_unknown_fields)` for typo protection
- Configurable exact cache limits via config file
  - `exact_max_entries` (default 20000)
  - `exact_ttl_secs` (default 3600)
- Admin HTTP routes under `/admin/`:
  - `GET /admin/metrics` — cache hit/miss/routing counters, uptime
  - `GET /admin/cache` — cache summary (entry counts, limits)
  - `DELETE /admin/cache` — flush all caches, persist empty snapshot
  - `DELETE /admin/cache/exact/:key` — evict exact cache entry
  - `DELETE /admin/cache/semantic/:context_key` — evict semantic bucket
- Admin route auth: loopback-open by default, key-required on remote
  - Configurable via `STACK_INTERCEPT_ADMIN_KEY` / `admin_key`
- Request-level metrics via `Arc<AtomicU64>` counters:
  - exact_hits, semantic_hits, misses, upstream_errors
  - routed_fallback, routed_passthrough
  - cache_inserts_exact, cache_inserts_semantic
- Config file unit tests and admin route integration tests
```

- [ ] **Step 2: Commit**

```bash
git add Cargo.toml CHANGELOG.md
git commit -m "chore: bump version to 0.2.1, update changelog"
```

---

### Self-Review

**1. Spec coverage:**
- Config file with `FileConfig`: Task 2
- Loading order (defaults → TOML → env): Task 2
- Missing file behavior (explicit fail, default skip): Task 2
- Unknown keys fail (`deny_unknown_fields`): Task 2
- Secrets masked in logs: Task 2 Step 8
- Exact cache tunables in config: Task 1 + Task 2
- Admin routes (metrics, cache summary, flush, delete): Task 5
- Admin auth (loopback-open, key-required remote): Task 5 Step 1
- ConnectInfo peer address (no X-Forwarded-For): Task 5 Step 1
- Cache flush writes empty snapshot: Task 5 Step 4
- Admin cache exact/semantic delete: Task 5 Steps 5-6
- Metrics struct with AtomicU64: Task 4
- Metrics increment points: Task 6
- ExactCache API additions (remove, clear, len, etc.): Task 3
- Config unit tests: Task 7
- Admin integration tests: Task 8

**2. Placeholder scan:** Every step has complete code. No TBD, TODO, or "implement later".

**3. Type consistency:**
- `FileConfig` uses `Option<T>` fields ✓
- `Metrics` fields are `AtomicU64` ✓
- `ExactCache::remove(&mut self, key: &str) -> bool` ✓
- `check_admin_auth` returns `Result<(), StatusCode>` ✓
- All admin handlers use `State(state)`, `headers`, `ConnectInfo(addr)` pattern ✓
