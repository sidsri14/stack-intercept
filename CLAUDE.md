# StackIntercept — CLAUDE.md

## Project Overview
A local Rust LLM proxy that intercepts OpenAI SDK calls for caching and model routing. Single binary deployable on a cheap VPS. Uses Candle (Rust ML framework) for local BGE-small-en-v1.5 embeddings on CPU.

## Architecture Priority Order
Compatibility → Safety → Exact cache → Semantic cache → HNSW → Dynamic routing → Benchmarks

Routing is opt-in (`STACK_INTERCEPT_ALLOW_MODEL_REWRITE=true`). Cache keys include routing namespace to prevent cross-contamination. Route headers are added to all responses for transparency.

## File Map

### Source
- `src/main.rs` — Axum HTTP server, `/v1/chat/completions` handler, AppState, streaming passthrough, cache orchestration, `Metrics` struct (8 AtomicU64 counters), 5 admin route handlers, `check_admin_auth()`, `is_loopback()`
- `src/embeddings.rs` — `LocalPredictor` struct: `init_from_disk()`, `encode_text(&str) -> Vec<f32>`. BGE-small-en-v1.5 model, 384-dim, mean pooling + L2 normalization
- `src/cache.rs` — `cache_key_hash()` (SHA256 of canonical full payload + provider + tenant + routing namespace), `is_eligible()` checks, `ExactCache` (bounded TTL-based with `Vec<u8>` body, +`remove`/`clear`/`len`/`is_empty`/`max_entries`/`default_ttl_secs`), `CachedEntry`, `CacheItem`, `evict_bucket()`/`evict_global()`, Snapshot types (rmp-serde MessagePack persistence)
- `src/router.rs` — `evaluate_routing()` inspects payload, classifies prompt complexity, decides whether to downgrade premium models to cheap models. Opt-in via `STACK_INTERCEPT_ALLOW_MODEL_REWRITE=true`. `RouteDecision` with `cache_namespace()`.
- `src/config.rs` — `FileConfig` (TOML deserialization, `#[serde(deny_unknown_fields)]`), `ProxyConfig` (`defaults()`/`load()`/`from_env()`/`apply_file_config()`/`apply_env_overrides()`), `CacheMode` enum. All 14 settings plus env-override-warnings for parse failures.

### Scripts & Config
- `build.cmd` — MSVC build env wrapper (VS Build Tools 2022, 14.44.35207). Run `./build.cmd build` (use Git Bash, not `.\build.cmd`)
- `download_model.sh` — Fetches bge-small-en-v1.5 (config.json, tokenizer.json, model.safetensors) from HuggingFace
- `test_proxy.py` — Two-prompt verification (cache miss, then exact/semantic hit)
- `test_semantic_safety.py` — Negative tests: different system prompt, different intent, different model — all must miss cache
- `test_mock_upstream.py` — 59 integration checks (admin routes, exact key deletion, cache hit/miss, streaming, tenant isolation)
- `test_routing.py` — 60 checks (routing safety, headers, auth, fallback key)
- `test_persistence_eviction_sse.py` — 24 checks (disk persistence, eviction, SSE error handling)
- `test_demo.py` — 60-second demo
- `benchmark.py` — Latency comparison across cache modes
- `model/` — BGE model files (config.json, tokenizer.json, 133MB model.safetensors). Gitignored safetensors.
- `.cargo/config.toml` — MSVC linker path override
- `.env.example` — Template for config file

### Docs
- `docs/superpowers/specs/2026-06-30-config-admin-metrics-design.md` — v0.2.1 design spec (TOML config, admin routes, metrics)
- `docs/superpowers/plans/2026-06-30-config-admin-metrics-plan.md` — v0.2.1 implementation plan
- `docs/superpowers/plans/2026-06-28-corrected-proxy-architecture.md` — Original architecture plan
- `docs/deployment.md` — Production deployment guide
- `docs/release-checklist.md` — Pre-release verification steps
- `landing/` — Static landing page for stackintercept.com (index.html + vercel.json)

## Key Architecture Decisions

### Raw SSE passthrough
Upstream provider SSE bytes are forwarded as-is via `axum::body::Body::from_stream`. Do NOT use `axum::Sse<Event>` — that wraps bytes in additional `data:` framing, corrupting streaming semantics.

### Two-layer cache
1. **Exact cache** (default): SHA256(provider, tenant, canonical_full_payload). Only caches when temperature=0, no tools, no `cache_control: no_store`.
2. **Semantic cache** (opt-in via `semantic` mode): BGE embedding + cosine dot product at `ALIGNMENT_BAR=0.93`. Gated by context key (everything except the last user message) first, then embedding similarity within that bucket.

### Semantic safety
Semantic search is never done on the last-user-message alone. It requires matching exact context key (everything except the last message) first, then embedding similarity within that bucket. This prevents unsafe cache hits across different tenants, system prompts, or models.

### HNSW not needed in prototype
0–10k entries: `Vec<CacheItem>` + linear cosine scan. fast-hnsw is in Cargo.toml but unused — stays as placeholder for >10k entries.

### No streaming request body parsing
`/v1/chat/completions` request bodies are normal JSON. Buffer with 5 MB max body size, JSON parse normally. Only the response is streaming.

## Build & Run

```bash
# Download model (133 MB)
./download_model.sh

# Set API key
export OPENAI_API_KEY="sk-..."

# Build
./build.cmd build

# Run
./build.cmd run

# Test
python test_proxy.py
python test_semantic_safety.py
python test_mock_upstream.py
python test_routing.py     # requires STACK_INTERCEPT_ALLOW_MODEL_REWRITE tests
```

## Configuration

| Env Var | Default | Description |
|---|---|---|
| `STACK_INTERCEPT_MODEL_DIR` | `./model` | Path to BGE model files |
| `STACK_INTERCEPT_CACHE_MODE` | `exact` | `off`, `exact`, or `semantic` |
| `STACK_INTERCEPT_TENANT_ID_HEADER` | (none) | Header name for tenant isolation |
| `STACK_INTERCEPT_ALLOW_MODEL_REWRITE` | `false` | Enable dynamic model routing (opt-in) |
| `STACK_INTERCEPT_UPSTREAM_URL` | `https://api.deepseek.com` | Primary LLM provider base URL |
| `STACK_INTERCEPT_FALLBACK_URL` | `https://api.deepseek.com` | Fallback (cheap) provider base URL for routed requests |
| `STACK_INTERCEPT_FALLBACK_API_KEY` | (from `DEEPSEEK_API_KEY`) | API key for fallback provider |
| `STACK_INTERCEPT_ADMIN_KEY` | (none) | Admin API auth key (required on remote) |
| `STACK_INTERCEPT_EXACT_MAX_ENTRIES` | `20000` | Max exact cache entries |
| `STACK_INTERCEPT_EXACT_TTL_SECS` | `3600` | Exact cache TTL (seconds) |
| `STACK_INTERCEPT_SEMANTIC_MAX_ITEMS` | `10000` | Max semantic cache items |
| `STACK_INTERCEPT_SEMANTIC_MAX_BUCKET_ITEMS` | `256` | Max items per semantic bucket |
| `STACK_INTERCEPT_SEMANTIC_TTL_SECS` | `3600` | Semantic cache TTL (seconds) |
| `STACK_INTERCEPT_CACHE_PATH` | (none) | File path for disk persistence |
| `STACK_INTERCEPT_DISABLE_PERSISTENCE` | `false` | Skip disk I/O for cache snapshots |

Config file (`stack-intercept.toml`) is also supported. Loading order: defaults → TOML → env vars. Env vars always win.

### Route headers (all responses)
- `x-stack-intercept-route`: `passthrough` or `fallback`
- `x-stack-intercept-original-model`: the model the client requested
- `x-stack-intercept-routed-model`: the model actually used to serve the request

### Per-request routing opt-out
Set `x-stack-intercept-no-route: true` on any request to bypass routing entirely.

## Candle-specific Notes
- `Device::Cpu` with SIMD — no AVX-512 guarantee. CUDA feature exists but untested.
- bge-small-en-v1.5: 384-dim, 12-layer BERT, 30522 vocab
- API: `VarBuilder::from_mmaped_safetensors`, `BertModel::load(vb, config)`, `model.forward(&ids, &token_types, None)`
- Mean pooling + L2 normalization for cosine via dot product

## Cargo Dependencies
- `axum 0.7` — HTTP framework (json, macros features)
- `reqwest 0.12` — Upstream proxy calls (json, stream features)
- `candle-core 0.8`, `candle-nn 0.8`, `candle-transformers 0.8` — Local ML inference
- `tokenizers 0.19` — BGE tokenizer
- `sha2 0.10` — Deterministic cache key hashing
- `rmp-serde` — MessagePack serialization for disk snapshot persistence
- `toml 0.8` — Config file parsing
- `fast-hnsw 1.0` — Unused, placeholder for HNSW index
- `serde/serde_json`, `tokio`, `futures-util`, `tracing`, `anyhow`

## Cross-compilation Target
x86-64 Linux for production deployment. Current dev environment: Windows (MSVC toolchain).

# CLAUDE.md

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.
