# Benchmarks

These benchmark numbers were collected with the local `benchmark.py` script using mock upstream servers. They do not require provider API keys.

Environment:

- OS: Windows
- Rust: 1.94.0
- Cargo: 1.94.0
- Build profile: dev
- Mock upstream delay: 50 ms
- Date: 2026-07-15

## Local Results

| Scenario | Median latency | Header/result | Relative to cold miss |
|---|---:|---|---:|
| Cold miss, no cache | 53.6 ms | `x-stack-intercept: miss` | 1.00x |
| Exact cache hit | 2.3 ms | `x-stack-intercept: hit` | 0.04x |
| Streaming exact cache hit | 1.8 ms | `x-stack-intercept: hit` | 0.03x |
| Semantic cache hit | 50.1 ms | `x-stack-intercept: hit` | 0.93x |
| Routed fallback request | 53.6 ms | `route: fallback` | 1.00x |

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

The benchmark script skips semantic measurements unless all required files are present:

- `model/config.json`
- `model/tokenizer.json`
- `model/model.safetensors`
