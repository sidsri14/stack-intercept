# StackIntercept Engineering Case Study

## Problem

OpenAI-compatible applications commonly need three small pieces of middleware:
deterministic response reuse, a bounded reaction to an upstream failure, and a
way to observe those behaviors. StackIntercept keeps those concerns in a local
Rust reverse proxy rather than asking application code to implement them per
client.

## What is implemented

- Exact cache keys are SHA-256 hashes of canonical request JSON plus provider,
  tenant, and routing namespace. The cache only accepts deterministic requests
  and never stores non-2xx responses.
- The optional semantic mode uses a local BGE embedding model, an exact context
  gate, and capped per-context linear buckets. It is off by default.
- Reactive failover retries once on configured upstream failures when a fallback
  is configured. It is not a load balancer or a health-checking provider pool.
- Prometheus metrics expose cache and route counters, including tenant labels.

## How to verify it locally

The repository includes mock-upstream integration tests and a short interactive
demo. Neither requires a provider API key:

```bash
cargo build
python test_demo.py
python test_mock_upstream.py
python test_routing.py
python test_failover.py
```

For containerized staging, use `docker-compose.trial.yml`. It starts in
exact-cache mode, leaves semantic caching and model routing disabled, and makes
rollback a base-URL change.

## Measured behavior

The benchmark document records local mock-upstream measurements, not a promise
about a customer's network or provider latency. Exact cache hits measured 2.3
ms and streaming exact cache hits 1.8 ms in the recorded environment. Semantic
mode includes local embedding work and measured 50.1 ms in that same benchmark.
See [benchmarks.md](benchmarks.md) for the reproduction command and conditions.

## Explicit limits

- No rate limiting or spend-cap enforcement.
- No circuit breaker, load balancing, or provider health checking.
- Semantic caching is not enabled in the default image and requires local model
  weights.
- The project has no claim of customer traffic, cost savings, or production SLA.
