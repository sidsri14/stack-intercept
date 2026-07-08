# Benchmarks

These benchmark numbers were collected with the local `benchmark.py` script using mock upstream servers. They do not require provider API keys.

Environment:

- OS: Windows
- Rust: 1.94.0
- Cargo: 1.94.0
- Build profile: dev
- Mock upstream delay: 50 ms
- Date: 2026-07-08

## Local Results

| Scenario | Median latency | Header/result | Relative to cold miss |
|---|---:|---|---:|
| Cold miss, no cache | 53.9 ms | `x-stack-intercept: miss` | 1.00x |
| Exact cache hit | 1.3 ms | `x-stack-intercept: hit` | 0.02x |
| Streaming exact cache hit | 1.2 ms | `x-stack-intercept: hit` | 0.02x |
| Semantic startup + first request | skipped | model weights not present | n/a |
| Routed fallback request | 1.4 ms | `route: fallback` | 0.03x |

## Reproduce

```bash
cargo build
python benchmark.py
```

Semantic mode requires local BGE model weights:

```bash
./download_model.sh
STACK_INTERCEPT_CACHE_MODE=semantic cargo run
```

The benchmark script now skips semantic measurements unless all required files are present:

- `model/config.json`
- `model/tokenizer.json`
- `model/model.safetensors`
