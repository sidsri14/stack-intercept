# StackIntercept

A local Rust LLM proxy that intercepts OpenAI SDK calls and safely caches responses. Routes to DeepSeek by default for 90-95% cost savings vs GPT-4o. Builds toward model routing and semantic deduplication for controlled workloads.

## Quickstart

### 1. Download model weights

```bash
./download_model.sh
```

Downloads BGE-small-en-v1.5 (133 MB) for semantic embeddings.

### 2. Set your API key

```bash
export OPENAI_API_KEY="sk-deepseek-your-key-here"
```

StackIntercept uses your DeepSeek key to route all requests through `api.deepseek.com` by default.

### 3. Start the proxy

```bash
cargo run
```

Listens on `http://127.0.0.1:8080`.

### 4. Test it

```bash
# Exact cache test (default mode)
python test_proxy.py

# Semantic cache safety test
export STACK_INTERCEPT_CACHE_MODE=semantic
python test_semantic_safety.py

# Semantic cache hit verification
python test_semantic_hit.py
```

## Configuration

| Env Var | Default | Description |
|---|---|---|
| `STACK_INTERCEPT_UPSTREAM_URL` | `https://api.deepseek.com` | Upstream LLM provider base URL |
| `STACK_INTERCEPT_MODEL_DIR` | `./model` | Path to BGE model files |
| `STACK_INTERCEPT_CACHE_MODE` | `exact` | `off`, `exact`, or `semantic` |
| `STACK_INTERCEPT_TENANT_ID_HEADER` | (none) | Header name for tenant isolation |
| `STACK_INTERCEPT_ALLOW_MODEL_REWRITE` | `false` | Allow proxy to substitute models |

Use `STACK_INTERCEPT_UPSTREAM_URL=https://api.openai.com` to switch back to OpenAI.

## Architecture

Compatibility -> Safety -> Exact cache -> Semantic cache -> HNSW -> Dynamic routing -> Benchmarks

See `docs/superpowers/plans/2026-06-28-corrected-proxy-architecture.md`.
