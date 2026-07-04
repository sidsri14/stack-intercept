# Release Checklist

## Pre-release

- [x] `cargo build --release` compiles with zero warnings
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo fmt --check` passes
- [x] All 32 Rust unit tests pass
- [ ] All integration tests pass without API keys or model weights:
  ```bash
  python test_mock_upstream.py    # ~59 checks
  python test_routing.py          # ~60 checks
  python test_persistence_eviction_sse.py  # ~24 checks
  ```
- [ ] Demo script runs clean:
  ```bash
  python test_demo.py
  ```
- [ ] Benchmark script runs without errors:
  ```bash
  python benchmark.py
  ```

## Version bump (done before tagging)

- [x] Update `version` in `Cargo.toml` (semver) — 0.2.1 → 0.2.2
- [x] Update `CHANGELOG.md` with release notes

## Build

### CI — automated (Linux x86_64 + Windows x86_64)
Both Linux and Windows binaries are built automatically by the `release.yml` workflow when the tag is pushed.
See `.github/workflows/release.yml` for details.

### Local development builds
```bash
# Linux / macOS
cargo build --release

# Windows (requires MSVC Build Tools)
build.cmd build --release
```

## Package

- [x] Binary + `.env.example` + `download_model.sh` + `docs/` → `.tar.gz` / `.zip` (CI handles this)
- [ ] Verify binary starts: `./stack-intercept` prints "online at http://127.0.0.1:8080"
- [ ] Verify `.env.example` has accurate defaults
- [ ] Verify `README.md` quickstart works from scratch

## GitHub Release

- [ ] Tag: `git tag v0.2.2 && git push --tags`
- [ ] Create release at https://github.com/sidsri14/stack-intercept/releases
- [ ] Upload Linux binary: `stack-intercept-v0.2.2-x86_64-unknown-linux-gnu.tar.gz`
- [ ] Upload Windows binary: `stack-intercept-v0.2.2-x86_64-pc-windows-msvc.zip`
- [ ] Write release notes (copy from CHANGELOG)
