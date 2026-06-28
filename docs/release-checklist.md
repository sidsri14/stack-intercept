# Release Checklist

## Pre-release

- [ ] `cargo build --release` compiles with zero warnings
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] All integration tests pass without API keys or model weights:
  ```bash
  python test_mock_upstream.py    # 24 tests
  python test_routing.py          # 60 tests
  ```
- [ ] Demo script runs clean:
  ```bash
  python test_demo.py
  ```
- [ ] Benchmark script runs without errors:
  ```bash
  python benchmark.py
  ```

## Version bump

- [ ] Update `version` in `Cargo.toml` (semver)
- [ ] Update `CHANGELOG.md` with release notes
  ```markdown
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
  ```

## Build

### Windows
```bash
cargo build --release --target x86_64-pc-windows-msvc
strip target/x86_64-pc-windows-msvc/release/stack-intercept.exe
```

### Linux (CI or cross-compile)
```bash
cargo build --release --target x86_64-unknown-linux-gnu
strip target/x86_64-unknown-linux-gnu/release/stack-intercept
```

## Package

- [ ] Binary + `.env.example` + `download_model.sh` → `.tar.gz` / `.zip`
- [ ] Verify binary starts: `./stack-intercept` prints "online at http://127.0.0.1:8080"
- [ ] Verify `.env.example` has accurate defaults
- [ ] Verify `README.md` quickstart works from scratch

## GitHub Release

- [ ] Tag: `git tag v0.1.0 && git push --tags`
- [ ] Create release at https://github.com/sidsri14/stack-intercept/releases
- [ ] Upload Linux binary: `stack-intercept-v0.1.0-x86_64-unknown-linux-gnu.tar.gz`
- [ ] Upload Windows binary: `stack-intercept-v0.1.0-x86_64-pc-windows-msvc.zip`
- [ ] Write release notes (copy from CHANGELOG)
