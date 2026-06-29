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
  ## [0.1.1] - 2026-06-29

  ### Added
  - Linux x86_64 binary via CI
  - SHA256 checksums for all release artifacts
  - CI status badge and release badge in README
  - Automated GitHub Release workflow (tag-triggered)

  ### Changed
  - `.gitignore` now excludes `dist/` directory
  ```

## Build

### Windows
```bash
cargo build --release --target x86_64-pc-windows-msvc
strip target/x86_64-pc-windows-msvc/release/stack-intercept.exe
```

### Linux (CI — automated)
Linux binary is built automatically by the `release.yml` workflow when the tag is pushed.
See `.github/workflows/release.yml` for details.

### Windows (manual build)
```bash
cargo build --release --target x86_64-pc-windows-msvc
strip target/x86_64-pc-windows-msvc/release/stack-intercept.exe
```

## Package

- [ ] Binary + `.env.example` + `download_model.sh` → `.tar.gz` / `.zip`
- [ ] Verify binary starts: `./stack-intercept` prints "online at http://127.0.0.1:8080"
- [ ] Verify `.env.example` has accurate defaults
- [ ] Verify `README.md` quickstart works from scratch

## GitHub Release

- [ ] Tag: `git tag v0.1.1 && git push --tags`
- [ ] Create release at https://github.com/sidsri14/stack-intercept/releases
- [ ] Upload Linux binary: `stack-intercept-v0.1.1-x86_64-unknown-linux-gnu.tar.gz`
- [ ] Upload Windows binary: `stack-intercept-v0.1.1-x86_64-pc-windows-msvc.zip`
- [ ] Write release notes (copy from CHANGELOG)
