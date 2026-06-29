# Changelog

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
