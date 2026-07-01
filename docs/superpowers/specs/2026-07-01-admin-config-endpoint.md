# GET /admin/config — Runtime Config Endpoint

**Version:** v0.2.2
**Date:** 2026-07-01
**Status:** Design approved, pending implementation

## Goal

Expose the current effective runtime configuration (merged from defaults → TOML → env vars) via the admin API. The endpoint doubles as a minimal health check — a 200 response means the server is alive and processing requests.

## Why

Pilot users need a way to verify which settings are active without restarting the proxy or watching startup logs. When debugging config issues ("why isn't semantic mode working?", "what TTL is actually active?"), a single curl gives the answer. This closes an ops gap: the README and deployment guide tell users *how* to set config, but provide no way to verify the merged result at runtime.

## Response Format

All config fields serialized as JSON. Secrets are masked. Cache mode is a lowercase string.

```json
{
  "cache_mode": "exact",
  "tenant_id_header": null,
  "allow_model_rewrite": false,
  "upstream_base_url": "https://api.deepseek.com",
  "fallback_base_url": "https://api.deepseek.com",
  "fallback_api_key": "sk-s************",
  "admin_key": "********",
  "exact_max_entries": 20000,
  "exact_ttl_secs": 3600,
  "semantic_max_items": 10000,
  "semantic_max_bucket_items": 256,
  "semantic_ttl_secs": 3600,
  "cache_path": null,
  "disable_persistence": false,
  "max_body_size": 5242880
}
```

### Masking rules

| Field | Unset | Set |
|---|---|---|
| `admin_key` | `null` | `"********"` |
| `fallback_api_key` | `null` | `"<first 4 chars>*****"` |

`fallback_api_key` shows the first 4 characters so ops can verify *which* key is configured (e.g., `sk-s...` vs `sk-a...`), without exposing the full secret.

### `cache_mode` serialization

| Rust enum | JSON string |
|---|---|
| `CacheMode::Off` | `"off"` |
| `CacheMode::Exact` | `"exact"` |
| `CacheMode::Semantic` | `"semantic"` |

## Implementation

### Files to modify

1. **`src/main.rs`** — Add `ConfigResponse` struct, `admin_config` handler, and route registration
2. **`test_mock_upstream.py`** — Add integration tests for the new endpoint

### What NOT to change

- `src/config.rs` — No changes needed. `ProxyConfig` already has all the fields. Add a `to_config_response()` method or serialize from the handler.
- `README.md` — Add the new route to the admin API table
- Admin auth — Same `check_admin_auth()` pattern as existing routes

### Handler structure

Location: `src/main.rs`, grouped with other admin handlers (~L490 area).

```rust
#[derive(serde::Serialize)]
struct ConfigResponse {
    cache_mode: &'static str,
    tenant_id_header: Option<&'static str>,
    allow_model_rewrite: bool,
    upstream_base_url: &'static str,
    fallback_base_url: &'static str,
    fallback_api_key: Option<String>,  // masked
    admin_key: Option<&'static str>,    // masked
    exact_max_entries: usize,
    exact_ttl_secs: u64,
    semantic_max_items: usize,
    semantic_max_bucket_items: usize,
    semantic_ttl_secs: u64,
    cache_path: Option<&'static str>,
    disable_persistence: bool,
    max_body_size: usize,
}
```

The handler constructs a `ConfigResponse` from `state.config`, applying masking rules inline. The struct borrows strings from `state.config`, so the handler signature stays simple.

### Route

```rust
.admin_route("/config", axum::routing::get(admin_config))
```

Added to the existing `admin_router` chain at `src/main.rs:248`.

## Tests

### Python integration tests (`test_mock_upstream.py`)

1. **GET /admin/config returns 200** — Basic smoke test
2. **Response contains expected top-level keys** — At minimum: `cache_mode`, `upstream_base_url`, `exact_max_entries`, `admin_key`
3. **Secrets are masked** — `admin_key` is `null` or `"********"`, not the raw value. `fallback_api_key` is `null` or masked.
4. **Respects admin auth** — Remote address without key → 403

### Rust unit tests

Not needed — the handler is a thin passthrough. Config masking logic is exercised by the Python tests.

## Health Check

Since a 200 response from `/admin/config` proves the server is alive, no separate `/admin/health` endpoint is needed. The deployment guide can recommend:

```bash
curl -s http://127.0.0.1:8080/admin/config > /dev/null && echo "healthy"
```

## Open Questions

None. Design is approved.
