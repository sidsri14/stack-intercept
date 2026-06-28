# StackIntercept — CLAUDE.md

## Project Overview
A local Rust LLM proxy that intercepts OpenAI SDK calls for caching and model routing. Single binary deployable on a cheap VPS. Uses Candle (Rust ML framework) for local BGE-small-en-v1.5 embeddings on CPU.

## Architecture Priority Order
Compatibility → Safety → Exact cache → Semantic cache → HNSW → Dynamic routing → Benchmarks

Never chase "semantic 0ms" before passthrough correctness. Exact cache is default; semantic is opt-in via `STACK_INTERCEPT_CACHE_MODE=semantic`.

## File Map

### Source
- `src/main.rs` — Axum HTTP server, `/v1/chat/completions` handler, AppState, streaming passthrough, cache orchestration
- `src/embeddings.rs` — `LocalPredictor` struct: `init_from_disk()`, `encode_text(&str) -> Vec<f32>`. BGE-small-en-v1.5 model, 384-dim, mean pooling + L2 normalization
- `src/cache.rs` — `cache_key_hash()` (SHA256 of canonical full payload + provider + tenant), `is_eligible()` checks, `ExactCache` (bounded TTL-based with `Vec<u8>` body), `CachedEntry`
- `src/config.rs` — `ProxyConfig::from_env()` reads `STACK_INTERCEPT_CACHE_MODE`, `STACK_INTERCEPT_TENANT_ID_HEADER`

### Scripts & Config
- `build.cmd` — MSVC build env wrapper (VS Build Tools 2022, 14.44.35207). Run `./build.cmd build` (use Git Bash, not `.\build.cmd`)
- `download_model.sh` — Fetches bge-small-en-v1.5 (config.json, tokenizer.json, model.safetensors) from HuggingFace
- `test_proxy.py` — Two-prompt verification (cache miss, then exact/semantic hit)
- `test_semantic_safety.py` — Negative tests: different system prompt, different intent, different model — all must miss cache
- `model/` — BGE model files (config.json, tokenizer.json, 133MB model.safetensors). Gitignored safetensors.
- `.cargo/config.toml` — MSVC linker path override

### Docs
- `docs/superpowers/plans/2026-06-28-corrected-proxy-architecture.md` — Full implementation plan
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
```

## Configuration

| Env Var | Default | Description |
|---|---|---|
| `STACK_INTERCEPT_MODEL_DIR` | `./model` | Path to BGE model files |
| `STACK_INTERCEPT_CACHE_MODE` | `exact` | `off`, `exact`, or `semantic` |
| `STACK_INTERCEPT_TENANT_ID_HEADER` | (none) | Header name for tenant isolation |

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
