# StackIntercept

[![CI](https://github.com/sidsri14/stack-intercept/actions/workflows/ci.yml/badge.svg)](https://github.com/sidsri14/stack-intercept/actions/workflows/ci.yml)
[![GitHub Release](https://img.shields.io/github/v/release/sidsri14/stack-intercept?logo=github)](https://github.com/sidsri14/stack-intercept/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://opensource.org/licenses/MIT)

**Local OpenAI-compatible cost-control proxy.** Sits between your app and an LLM provider. Caches responses for zero-cost repeats. Optionally routes simple prompts to cheaper models. One binary, no dependencies.

```
Your App  →  StackIntercept (:8080)  →  LLM Provider (DeepSeek, OpenAI, etc.)
                    │
                    ├─ Exact cache (default) — identical requests, instant replay
                    ├─ Semantic cache (opt-in) — similar prompts, same context → hit
                    └─ Model routing (opt-in) — gpt-4o for simple Qs → deepseek-chat
```

## Why

LLM API costs add up fast. Most apps send the same prompts repeatedly — same system prompt, same instructions, same questions. StackIntercept eliminates that waste:

- **Exact cache**: Repeat a request → get the cached response. Zero latency, zero cost.
- **Semantic cache**: Ask "How do I delete a file in Python?" then "How do I remove a file?" — second hits cache if the conversation context matches.
- **Model routing**: Send `gpt-4o` for everything → simple prompts automatically go to `deepseek-chat` (~5% the cost). Opt-in, transparent, safe.

## Quickstart (1 minute)

### 1. Set your API key

```bash
export DEEPSEEK_API_KEY="sk-your-key-here"
```

### 2. Start the proxy

```bash
cargo run
```

```
StackIntercept online at http://127.0.0.1:8080
```

### 3. Point your app at it

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8080", api_key="sk-your-key")
```

### 4. See it work

```bash
# First request — cache miss, forwards to provider
# Second request — cache hit, instant response
python test_mock_upstream.py    # 24 checks, no API key needed
python test_routing.py          # 60 checks, no API key needed
```

## Download

Pre-built binaries for Linux and Windows on the [Releases page](https://github.com/sidsri14/stack-intercept/releases).

```bash
# Linux
curl -LO https://github.com/sidsri14/stack-intercept/releases/download/v0.2.1/stack-intercept-v0.2.1-x86_64-unknown-linux-gnu.tar.gz
tar xzf stack-intercept-v0.2.1-x86_64-unknown-linux-gnu.tar.gz
cd stack-intercept

# Windows — download the .zip from the Releases page and extract
```

## How it works

### Caching (always on, default: exact)

Every response is cached by its SHA256 hash of the full request payload, provider, and tenant. Repeat a request verbatim → instant response. No API call made.

```bash
# First request: cache miss → provider → stored
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $DEEPSEEK_API_KEY" \
  -d '{"model":"deepseek-chat","messages":[{"role":"user","content":"Hello"}],"temperature":0}'
# Response: x-stack-intercept: miss

# Same request again: cache hit → instant
# Response: x-stack-intercept: hit
```

Cache rules:
- Only caches when `temperature=0` (deterministic output)
- Ignores requests with `tools`, `response_format`, or `cache_control: no_store`
- Tenant-isolated via `STACK_INTERCEPT_TENANT_ID_HEADER`
- TTL: 1 hour, oldest-entry eviction up to 20,000 entries
- Non-2xx responses are never cached

### Semantic mode (opt-in)

Enable with `STACK_INTERCEPT_CACHE_MODE=semantic`. Uses local BGE-small-en-v1.5 embeddings (384-dim, CPU, ~133 MB model) to find semantically similar prompts within the same conversation context.

```bash
# Download model weights first (one-time, 133 MB)
./download_model.sh

# Start with semantic mode
STACK_INTERCEPT_CACHE_MODE=semantic cargo run
```

Safety design:
- Context key hashes everything **except** the last user message (system prompt, conversation history, model, tenant, tools schema)
- Semantic scan only runs within matching context buckets
- Similarity threshold: 0.93 cosine

### Model routing (opt-in)

Enable with `STACK_INTERCEPT_ALLOW_MODEL_REWRITE=true`. When your app sends `gpt-4o` for a simple prompt, StackIntercept can route it to `deepseek-chat` instead — saving 90-95% on that request.

```bash
export STACK_INTERCEPT_ALLOW_MODEL_REWRITE=true
export STACK_INTERCEPT_FALLBACK_URL=https://api.deepseek.com
export STACK_INTERCEPT_FALLBACK_API_KEY=sk-deepseek-fallback-key
cargo run
```

**Safety by default:**
- Routing is **opt-in** (default: off). No surprise model switches.
- Blocked for: tools, structured output, temperature > 0, multimodal content, explicit model requirements ("do not switch models", "must use gpt-4")
- High-reasoning prompts stay on the original model: cryptography, legal analysis, security review, race conditions, distributed systems, financial models, and 30+ other keyword categories
- Per-request opt-out via `x-stack-intercept-no-route: true` header
- If no fallback API key is configured, routing is forced to passthrough (no auth leakage)
- Cache keys include routing namespace — routed and passthrough responses never share a cache slot

**Transparent headers on every response:**

| Header | Example | Meaning |
|---|---|---|
| `x-stack-intercept` | `hit`, `miss`, `error` | Cache status |
| `x-stack-intercept-route` | `passthrough`, `fallback` | Where the request went |
| `x-stack-intercept-original-model` | `gpt-4o` | What the client asked for |
| `x-stack-intercept-routed-model` | `deepseek-chat` | What actually served it |

## Configuration

| Env Var | Default | Description |
|---|---|---|
| `STACK_INTERCEPT_UPSTREAM_URL` | `https://api.deepseek.com` | Primary LLM provider |
| `STACK_INTERCEPT_CACHE_MODE` | `exact` | `off`, `exact`, or `semantic` |
| `STACK_INTERCEPT_MODEL_DIR` | `./model` | Path to BGE model files (semantic mode) |
| `STACK_INTERCEPT_TENANT_ID_HEADER` | (none) | HTTP header for tenant cache isolation |
| `STACK_INTERCEPT_ALLOW_MODEL_REWRITE` | `false` | Enable model routing (opt-in) |
| `STACK_INTERCEPT_FALLBACK_URL` | `https://api.deepseek.com` | Fallback provider for routed requests |
| `STACK_INTERCEPT_FALLBACK_API_KEY` | (from `DEEPSEEK_API_KEY`) | API key for fallback provider |
| `STACK_INTERCEPT_ADMIN_KEY` | (none) | Admin API auth key (required on remote) |
| `STACK_INTERCEPT_EXACT_MAX_ENTRIES` | `20000` | Max exact cache entries |
| `STACK_INTERCEPT_EXACT_TTL_SECS` | `3600` | Exact cache TTL (seconds) |
| `STACK_INTERCEPT_SEMANTIC_MAX_ITEMS` | `10000` | Max semantic cache items |
| `STACK_INTERCEPT_SEMANTIC_TTL_SECS` | `3600` | Semantic cache TTL (seconds) |
| `STACK_INTERCEPT_CACHE_PATH` | (none) | File path for disk persistence |
| `STACK_INTERCEPT_DISABLE_PERSISTENCE` | `false` | Skip disk I/O for cache snapshots |

### TOML config file

All env vars can also be set via `stack-intercept.toml` in the working directory. The loading order is:

```
hardcoded defaults → stack-intercept.toml → env vars
```

Env vars always win. Explicit path via `STACK_INTERCEPT_CONFIG=./path/to/config.toml`.

```toml
cache_mode = "exact"
upstream_url = "https://api.deepseek.com"
exact_max_entries = 20000
exact_ttl_secs = 3600
cache_path = "cache.bin"
```

Missing config file is silent (defaults apply). Invalid TOML or unknown keys fail at startup.

## Admin API

All admin routes live under `/admin/`. Local-only by default; require `x-admin-key` header when bound to a remote address.

| Route | Method | Description |
|---|---|---|
| `/admin/metrics` | GET | Hit/miss counters, uptime, routing stats |
| `/admin/cache` | GET | Cache summary (entries, limits, TTL) |
| `/admin/cache` | DELETE | Flush all caches, write empty snapshot |
| `/admin/cache/exact/:key` | DELETE | Evict single exact cache entry |
| `/admin/cache/semantic/:context_key` | DELETE | Evict single semantic bucket |

```bash
# Metrics (loopback: no auth needed)
curl http://127.0.0.1:8080/admin/metrics

# Cache summary
curl http://127.0.0.1:8080/admin/cache

# Flush all caches
curl -X DELETE http://127.0.0.1:8080/admin/cache
```

## Architecture

```
                    ┌──────────────────────┐
                    │   Client App          │
                    │  (OpenAI SDK)         │
                    └──────┬───────────────┘
                           │ POST /v1/chat/completions
                    ┌──────▼───────────────┐
                    │  StackIntercept       │
                    │                       │
                    │  1. Evaluate routing  │  ← opt-in, runs before cache
                    │  2. Exact cache check │  ← O(1) SHA256 lookup
                    │  3. Semantic scan     │  ← if semantic mode, within context bucket
                    │  4. Forward to LLM    │  ← original or routed provider
                    │  5. Cache response    │  ← on success
                    │  6. Return + headers  │  ← transparent routing info
                    └──────┬───────────────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
     ┌────────▼───┐ ┌─────▼─────┐ ┌────▼──────┐
     │  Upstream  │ │ Fallback  │ │  Cache    │
     │  Provider  │ │ Provider  │ │ (in-mem)  │
     │(DeepSeek,  │ │(DeepSeek, │ │           │
     │ OpenAI,    │ │ Together, │ │ Exact:    │
     │ etc.)      │ │ etc.)     │ │ HashMap   │
     └────────────┘ └───────────┘ │           │
                                  │ Semantic: │
                                  │ HashMap   │
                                  │ + Cosine  │
                                  └───────────┘
```

For self-hosted production setup, see [docs/deployment.md](docs/deployment.md).

## Build & Run

```bash
# Prerequisites
# - Rust toolchain (stable)
# - For semantic mode only: 133 MB model weights
./download_model.sh

# Build
cargo build --release

# Run
cargo run

# Test (no API key, no model weights needed)
python test_mock_upstream.py    # 51 checks — exact cache, streaming, tenant isolation, admin API
python test_routing.py          # 60 checks — routing safety, headers, auth, fallback key
python test_persistence_eviction_sse.py  # 24 checks — persistence, eviction, SSE errors
```

## Demo

```bash
# 60-second demo — routing, caching, headers visible
python test_demo.py
```

## Benchmark

```bash
# Latency comparison across cache modes
python benchmark.py
```

## What it's not

- Not a load balancer. No round-robin, health checks, or failover across providers.
- Not an API gateway. No rate limiting, key management, or user auth.
- Not a single-binary deployment for semantic mode (requires model weights).
- Not a streaming-aware semantic cache (SSE responses are cached but not semantically deduplicated in streaming).

## License

MIT
