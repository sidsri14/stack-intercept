# StackIntercept v0.2.1 — Config File, Admin Routes, Metrics

## Overview

Add three operational features without touching the cache engine:

1. **TOML config file** — `stack-intercept.toml` as an alternative/addition to env vars
2. **Admin HTTP routes** — `/admin/metrics`, `/admin/cache`, cache eviction under `/admin/`
3. **Request-level metrics** — hit/miss/routing counters exposed via `/admin/metrics`

HNSW indexing remains deferred future work. It did not ship in v0.3.0.

## 1. Config File

### File format (`stack-intercept.toml`)

```toml
cache_mode = "exact"              # "off" | "exact" | "semantic"
tenant_id_header = "X-Tenant"
allow_model_rewrite = false
upstream_url = "https://api.deepseek.com"
fallback_url = "https://api.deepseek.com"
fallback_api_key = "sk-..."       # prefer env var for secrets
admin_key = "..."                 # prefer env var for secrets

# Cache sizing
exact_max_entries = 20000
exact_ttl_secs = 3600
semantic_max_items = 10000
semantic_max_bucket_items = 256
semantic_ttl_secs = 3600

# Persistence
cache_path = "cache.bin"
disable_persistence = false
```

### Loading order

```
hardcoded defaults → stack-intercept.toml → env vars
```

Env vars always win. The file fills in gaps.

### `FileConfig` struct (separate from `ProxyConfig`)

```rust
#[derive(Debug, Deserialize, Default)]
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

`deny_unknown_fields` catches typos at startup.

### Missing file behavior

| Scenario | Behavior |
|---|---|
| `STACK_INTERCEPT_CONFIG` set, file missing | **Fail startup** with clear error |
| `STACK_INTERCEPT_CONFIG` unset, `./stack-intercept.toml` missing | **Silent skip** — env-only works fine |
| File exists but invalid TOML | **Fail startup** |
| File has unknown keys | **Fail startup** (`deny_unknown_fields`) |

### `ProxyConfig::from_env()` changes

Replace the current `from_env()` with a three-phase builder:

1. `ProxyConfig::defaults()` — hardcoded defaults (same values as today)
2. `apply_file_config()` — load TOML, merge non-None values into `ProxyConfig`
3. `apply_env_overrides()` — current env-var logic, run last so env always wins
4. `validate()` — check invariants (e.g., no admin key on non-loopback)

```rust
impl ProxyConfig {
    pub fn load() -> Self {
        let mut config = Self::defaults();
        config.apply_file_config();
        config.apply_env_overrides();
        config.validate();
        config
    }
}
```

### `ExactCache::new()` takes config

Replace the hardcoded `ExactCache::new(20000, 3600)` in `main.rs`:

```rust
exact_cache: RwLock::new(ExactCache::new(
    config.exact_max_entries,
    config.exact_ttl_secs,
)),
```

Add to `ProxyConfig`:

```rust
pub exact_max_entries: usize,  // default 20000
pub exact_ttl_secs: u64,       // default 3600
```

### Secrets in logs

`fallback_api_key` and `admin_key` are never printed in startup banners. Mask them:

```
Admin key: configured
Fallback API key: configured
```

## 2. Admin Routes

All admin routes live under `/admin/`. They are registered as a separate Axum router and nested:

```rust
let admin_router = Router::new()
    .route("/metrics", get(admin_metrics))
    .route("/cache", get(admin_cache_summary))
    .route("/cache", delete(admin_cache_flush))
    .route("/cache/exact/:key", delete(admin_cache_exact_delete))
    .route("/cache/semantic/:context_key", delete(admin_cache_semantic_delete));
```

Mounted via:

```rust
app.nest("/admin", admin_router);
```

### Auth rule (implemented as an Axum middleware/layer)

```
IF bound to loopback (127.0.0.1 / ::1):
    admin routes open (no auth required)
ELSE:
    admin routes require x-admin-key header matching config.admin_key
    IF header missing or wrong: 403 Forbidden
```

Implementation uses the Axum `ConnectInfo<SocketAddr>` extractor to read the actual peer address. No `X-Forwarded-For` trust.

**Important**: if `admin_key` is empty and the socket is non-loopback, admin routes return 403. This prevents accidental public exposure.

If `admin_key` is set, it's always required regardless of bind address (belt-and-suspenders).

### Route: `GET /admin/metrics`

Returns JSON:

```json
{
  "uptime_secs": 12345,
  "exact_hits": 42,
  "semantic_hits": 7,
  "misses": 150,
  "upstream_errors": 2,
  "routed_fallback": 30,
  "routed_passthrough": 120,
  "cache_inserts_exact": 80,
  "cache_inserts_semantic": 15
}
```

### Route: `GET /admin/cache`

Returns JSON:

```json
{
  "exact": {
    "entries": 123,
    "max_entries": 20000,
    "ttl_secs": 3600
  },
  "semantic": {
    "buckets": 18,
    "entries": 442,
    "max_items": 10000,
    "max_bucket_items": 256,
    "ttl_secs": 3600
  }
}
```

### Route: `DELETE /admin/cache`

Flush both caches, write empty snapshot to disk:

1. `state.exact_cache.write().unwrap().clear()`
2. `state.index.clear()`
3. Force `flush_persistence(&state)` (writes empty snapshot)
4. Return updated summary JSON (all zeros)

### Route: `DELETE /admin/cache/exact/:key`

```rust
state.exact_cache.write().unwrap().remove(&key);
// Notify: true if key existed, false otherwise
```

Returns `{ "removed": true }` or `{ "removed": false }`.

### Route: `DELETE /admin/cache/semantic/:context_key`

```rust
let existed = state.index.remove(&context_key).is_some();
```

Returns `{ "removed": true }` or `{ "removed": false }`.

## 3. Metrics

### `Metrics` struct

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
    pub started_at: Instant,
}
```

All counters use `Ordering::Relaxed`. Instantiated once in `AppState`, populated at each cache/route decision point.

### Points to increment

| Location | Counter |
|---|---|
| Exact cache hit | `exact_hits` |
| Semantic cache hit | `semantic_hits` |
| Cache miss (exact + semantic both miss) | `misses` |
| Upstream request error | `upstream_errors` |
| Route decision = fallback | `routed_fallback` |
| Route decision = passthrough | `routed_passthrough` |
| After exact cache insert | `cache_inserts_exact` |
| After semantic cache insert | `cache_inserts_semantic` |

## 4. `ExactCache` API additions

```rust
impl ExactCache {
    pub fn remove(&mut self, key: &str) -> bool;
    pub fn clear(&mut self);
    pub fn len(&self) -> usize;
    pub fn max_entries(&self) -> usize;
    pub fn default_ttl_secs(&self) -> u64;
}
```

## 5. `AppState` changes

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

## 6. Test Plan

### Config file tests (unit tests in `config.rs`)

- Loading valid `stack-intercept.toml` applies values correctly
- Env vars override TOML values
- Explicit `STACK_INTERCEPT_CONFIG` + missing file → error
- Default path missing → silent skip
- Invalid TOML → error
- Unknown keys → error
- `FileConfig` values merge correctly (None vs Some)

### Admin route tests (integration in mock upstream test)

- `GET /admin/metrics` returns valid JSON with zero counters before any requests
- `GET /admin/cache` returns valid cache summary
- After cache hit/miss, metrics reflect actual usage
- `DELETE /admin/cache` clears both caches
- `DELETE /admin/cache/exact/:key` removes single entry
- `DELETE /admin/cache/semantic/:context_key` removes semantic bucket
- Admin routes without key on loopback → 200
- Admin routes without key on non-loopback → 403
- Admin routes with correct key on non-loopback → 200
- Admin routes with wrong key → 403

### Persistence test

- `DELETE /admin/cache` followed by restart → empty cache (snapshot was empty)

## 7. Non-goals (v0.2.1)

- HNSW semantic indexing
- Semantic bucket eviction by individual item ID
- Per-tenant metrics breakdown
- Prometheus endpoint format
- Rate limiting / API key management
