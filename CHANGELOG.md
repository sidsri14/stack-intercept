# Changelog

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
