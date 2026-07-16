# Stage 1: Build dependency cache and compile binary
FROM rust:slim-bookworm AS builder
WORKDIR /app

# System deps for compilation
RUN apt-get update && apt-get install -y pkg-config libssl-dev g++ && rm -rf /var/lib/apt/lists/*

# Cache dependencies by building a dummy project first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -f target/release/deps/stack_intercept*

# Real source
COPY src ./src

# Full release build
RUN cargo build --release

# Stage 2: Minimal runtime
FROM debian:bookworm-slim
WORKDIR /app

RUN apt-get update && apt-get install -y libssl3 ca-certificates curl && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/stack-intercept /app/stack-intercept
COPY download_model.sh ./

# Download BGE model for semantic caching
RUN mkdir -p /app/model && bash download_model.sh /app/model && rm download_model.sh

EXPOSE 8080
ENV STACK_INTERCEPT_CACHE_MODE=exact
ENV STACK_INTERCEPT_UPSTREAM_URL=https://api.openai.com

CMD ["./stack-intercept"]
