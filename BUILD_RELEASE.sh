# Build for Linux

# --- Option A: Native Linux build (CI, WSL, or Linux host) ---

# Install cross-compilation target (one-time)
rustup target add x86_64-unknown-linux-gnu

# Build release binary
cargo build --release --target x86_64-unknown-linux-gnu

# Binary is at: target/x86_64-unknown-linux-gnu/release/stack-intercept
# Strip debug symbols for smaller binary
strip target/x86_64-unknown-linux-gnu/release/stack-intercept

# Package with model download script
mkdir -p dist/stack-intercept
cp target/x86_64-unknown-linux-gnu/release/stack-intercept dist/stack-intercept/
cp download_model.sh .env.example README.md dist/stack-intercept/
tar czf stack-intercept-linux-x86_64.tar.gz -C dist stack-intercept

echo "Created: stack-intercept-linux-x86_64.tar.gz"


# --- Option B: Cross-compile from macOS ---

# Install cross tool
# cargo install cross

# Build Linux binary
# cross build --release --target x86_64-unknown-linux-gnu


# --- Option C: Docker build ---

# docker build -t stack-intercept-builder .
# docker run --rm -v $(pwd)/dist:/dist stack-intercept-builder
