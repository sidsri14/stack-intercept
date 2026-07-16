# Changelog

## Unreleased

### Changed
- Optimized semantic-cache vector dot product with a runtime AVX path on x86_64 and an unrolled scalar fallback.
- Removed the unused `fast-hnsw` dependency and marked the old HNSW v0.3.0 spec as a historical/deferred draft.
- Updated README wording to avoid implying zero-latency behavior or shipped HNSW support.

## [0.3.0] - 2026-07-16

### Added
- **Resilience Auto-Failover Engine (Reactive Failover)**:
  - Automatically retries and failovers request to a fallback provider when primary upstream fails.
  - Failover is triggered on connection/transport errors or configurable status codes (default `500, 502, 503, 504`).
  - Configurable via TOML or env vars (`STACK_INTERCEPT_REACTIVE_FAILOVER`, `STACK_INTERCEPT_FAILOVER_MODEL`, `STACK_INTERCEPT_FAILOVER_STATUS_CODES`).
  - Supports model name rewriting on failover via `STACK_INTERCEPT_FAILOVER_MODEL`.
  - Increments a new `reactive_failovers` metric visible on `/admin/metrics`.
- Integration test suite (`test_failover.py`) checking connection refuse, 5xx errors, model rewriting, and disabled modes.
- **Per-tenant metrics breakdown**: cache hit/miss/routing/error counters isolated per tenant ID.
- **Prometheus endpoint**: `GET /admin/metrics/prometheus` exposes all counters in Prometheus text format.
- **Semantic cache eviction by item ID**: `DELETE /admin/cache/semantic/:key?item_id=N` removes a single entry from a context bucket.
- **Per-request semantic cache opt-out**: `x-stack-intercept-no-semantic-cache: true` header bypasses semantic cache for individual requests.
- **Docker support**: multi-stage Dockerfile with Rust compilation + BGE model download + Compose configuration.

## [0.2.2] - 2026-07-04


### Added
- `GET /admin/config` endpoint — runtime config introspection with secrets masked
  - Returns all config fields (cache_mode, URLs, limits, etc.)
  - Secrets (admin_key, fallback_api_key) masked in output
  - Doubles as a health check endpoint
- Windows x86_64 binary via CI release workflow
  - `stack-intercept.exe` packaged in `.zip` with docs and config template
  - CI pipeline builds, packages, and generates SHA256 checksums for both Linux and Windows

### Changed
- CI pipeline now builds and tests on both Linux and Windows
- Package step generates SHA256 checksums on both platforms
- `.gitignore` excludes `.gstack/` directory

## [0.2.1] - 2026-06-30

### Added
- TOML config file support (`stack-intercept.toml`)
  - Config file path via `STACK_INTERCEPT_CONFIG` env var
  - Precedence: defaults < TOML < env vars
  - `#[serde(deny_unknown_fields)]` catches typos in config
  - Explicit path missing → fatal error; default path missing → silent skip
- Configurable exact cache limits via config file or env vars
  - `exact_max_entries` (default 20000)
  - `exact_ttl_secs` (default 3600)
- Admin HTTP routes under `/admin/`:
  - `GET /admin/metrics` — cache hit/miss/routing counters and uptime
  - `GET /admin/cache` — cache summary (entry counts, limits, TTL)
  - `DELETE /admin/cache` — flush all caches and persist empty snapshot
  - `DELETE /admin/cache/exact/:key` — evict a single exact cache entry
  - `DELETE /admin/cache/semantic/:context_key` — evict a semantic context bucket
- Admin route auth: loopback-open by default, key-required for remote access
  - Configurable via `STACK_INTERCEPT_ADMIN_KEY` env var or `admin_key` in TOML
  - `ConnectInfo<SocketAddr>` peer address check (no X-Forwarded-For trust)
- Request-level metrics via atomic counters (`AtomicU64`, `Ordering::Relaxed`):
  - exact_hits, semantic_hits, misses, upstream_errors
  - routed_fallback, routed_passthrough
  - cache_inserts_exact, cache_inserts_semantic

### Changed
- `ProxyConfig` refactored into layered config pipeline:
  `defaults() → apply_file_config() → apply_env_overrides()`
- `ExactCache::new()` now reads limits from config instead of hardcoded values
- Secrets (admin_key, fallback_api_key) masked in startup logs

### Tests
- 9 unit tests for config defaults, FileConfig merge, cache mode parsing
- 6 integration tests for admin routes (metrics, cache, eviction, auth)

## [0.2.0] - 2026-06-29

### Added
- Disk persistence for both exact and semantic caches (rmp-serde MessagePack)
  - Configurable via `STACK_INTERCEPT_CACHE_PATH`
  - Opt-out via `STACK_INTERCEPT_DISABLE_PERSISTENCE`
  - Debounced to at most one write per second
  - Graceful shutdown (Ctrl+C) flushes remaining data
- SSE-formatted error frames for upstream stream failures
  - `content-type: text/event-stream` set on streaming error responses
  - Properly terminated `[DONE]\n\n` per SSE spec

### Changed
- Semantic cache index upgraded from `RwLock<HashMap>` to `DashMap` for
  concurrent bucket-level access without global lock contention
- Cache items now carry per-entry TTL (configurable via
  `STACK_INTERCEPT_SEMANTIC_TTL_SECS`, default 3600s)

### Fixed
- Global semantic eviction now counts total entries across all context
  buckets, not the number of buckets (`STACK_INTERCEPT_SEMANTIC_MAX_ITEMS`
  now correctly caps at 10,000 total entries)
- Bucket eviction off-by-one: push now happens before eviction, preventing
  perpetual max+1 bucket occupancy

### Config
- `STACK_INTERCEPT_CACHE_PATH` — path to cache snapshot file
- `STACK_INTERCEPT_DISABLE_PERSISTENCE` — opt-out of disk writes
- `STACK_INTERCEPT_SEMANTIC_MAX_ITEMS` — total semantic entry cap (default 10000)
- `STACK_INTERCEPT_SEMANTIC_MAX_BUCKET_ITEMS` — per-bucket cap (default 256)
- `STACK_INTERCEPT_SEMANTIC_TTL_SECS` — per-entry TTL (default 3600)

## [0.1.1] - 2026-06-29

### Added
- Linux x86_64 binary via CI (`package` job in CI workflow)
- SHA256 checksums for all release artifacts
- CI status badge and release badge in README
- `rust-cache` across all CI jobs for faster builds
- Automated GitHub Release workflow (triggered by `v*` tags)

### Changed
- `.gitignore` now excludes `dist/` directory

## [0.1.0] - 2026-06-28

### Added
- Exact cache (SHA256-based, in-memory)
- Semantic cache (BGE-small-en-v1.5 embeddings, opt-in)
- Dynamic model routing (opt-in, safe-by-default)
- Transparent route headers on all responses
- Tenant-level cache isolation
- Mock-server integration tests (84 checks)
- 60-second demo script
- Latency benchmark script

### Safety
- Routing is opt-in (default: off)
- Unsafe features block routing (tools, structured output, temp > 0, multimodal)
- High-reasoning keyword classifier (30+ patterns)
- Explicit model requirement detection
- Cache namespace isolation for routed vs passthrough responses
- Fallback API key leakage prevention
- Bearer prefix normalization
