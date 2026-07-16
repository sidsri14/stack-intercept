# Deployment Guide

Deploying StackIntercept on a Linux VPS for production use.

## Prerequisites

- Linux x86_64 VPS (2 GB RAM minimum for semantic mode; 512 MB for exact-only)
- Rust toolchain (or use the pre-built binary from GitHub Releases)

## Quick deploy (pre-built binary)

```bash
# Download the latest release
curl -LO https://github.com/sidsri14/stack-intercept/releases/latest/download/stack-intercept-v0.3.0-x86_64-unknown-linux-gnu.tar.gz
tar xzf stack-intercept-v0.3.0-x86_64-unknown-linux-gnu.tar.gz
cd stack-intercept

# Configure
cp .env.example .env
# Edit .env with your API key and settings

# Run
./stack-intercept
```

## File descriptor limits

By default, many Linux environments start services with a low per-process file descriptor limit. High-concurrency reverse proxies can saturate this limit, causing `EMFILE (Too many open files)` connection drops.

```bash
# Set limits in the current shell session
ulimit -n 65535
```

For persistent systemd service execution, add `LimitNOFILE=65535` to the service unit:

```ini
[Service]
LimitNOFILE=65535
```

Verify the active process limit:
```bash
cat /proc/$(pgrep stack-intercept)/limits | grep "Max open files"
```

## TLS termination

The proxy listens on raw HTTP. For remote or multi-host deployments, place it behind a TLS termination layer:

- AWS Application Load Balancer
- Cloudflare Tunnel
- Nginx (see config below)
- Caddy

```
Client SDK -> HTTPS/TLS ingress -> internal HTTP -> stack-intercept -> upstream LLM provider
```

For local development, keep the proxy bound to `127.0.0.1:8080`.

## Systemd service

Create `/etc/systemd/system/stack-intercept.service`:

```ini
[Unit]
Description=StackIntercept LLM Proxy
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=stack-intercept
Group=stack-intercept
WorkingDirectory=/opt/stack-intercept
EnvironmentFile=/opt/stack-intercept/.env
ExecStart=/opt/stack-intercept/stack-intercept
Restart=on-failure
RestartSec=5
LimitNOFILE=65535

# Cache persistence
ReadWritePaths=/var/cache/stack-intercept

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now stack-intercept
sudo journalctl -u stack-intercept -f
```

## Environment file

Place `/opt/stack-intercept/.env`:

```bash
DEEPSEEK_API_KEY=sk-your-key-here
STACK_INTERCEPT_CACHE_MODE=exact
STACK_INTERCEPT_CACHE_PATH=/var/cache/stack-intercept/snapshot.msgpack
# STACK_INTERCEPT_UPSTREAM_URL=https://api.deepseek.com
# STACK_INTERCEPT_FALLBACK_URL=https://api.deepseek.com
# STACK_INTERCEPT_ALLOW_MODEL_REWRITE=false
# STACK_INTERCEPT_REACTIVE_FAILOVER=false
# STACK_INTERCEPT_FAILOVER_MODEL=deepseek-chat
# STACK_INTERCEPT_FAILOVER_STATUS_CODES=500,502,503,504
```

Make sure the runtime user can write to the cache directory:
```bash
sudo mkdir -p /var/cache/stack-intercept
sudo chown stack-intercept:stack-intercept /var/cache/stack-intercept
```

## Cache persistence

When `STACK_INTERCEPT_CACHE_PATH` is configured, cache snapshots are saved to disk
using MessagePack. This allows warm-cache restoration across restarts.

Persistence can be disabled explicitly:
```bash
export STACK_INTERCEPT_DISABLE_PERSISTENCE=true
```

## Reactive failover

Reactive failover is disabled by default. Enable it only after configuring a fallback provider key:

```bash
STACK_INTERCEPT_REACTIVE_FAILOVER=true
STACK_INTERCEPT_FALLBACK_URL=https://api.deepseek.com
STACK_INTERCEPT_FALLBACK_API_KEY=sk-your-fallback-key
STACK_INTERCEPT_FAILOVER_MODEL=deepseek-chat
STACK_INTERCEPT_FAILOVER_STATUS_CODES=500,502,503,504,429
```

This is a single retry path for transient failures, not a load balancer or circuit breaker.

## Prometheus metrics

Prometheus text metrics are exposed at:

```bash
curl http://127.0.0.1:8080/admin/metrics/prometheus
```

If `STACK_INTERCEPT_ADMIN_KEY` is set, include `x-admin-key`.

## Semantic mode

Requires BGE-small-en-v1.5 model weights (~133 MB):

```bash
./download_model.sh
```

Then set `STACK_INTERCEPT_CACHE_MODE=semantic` in `.env`.

The model directory defaults to `./model` relative to the working directory.
Override with `STACK_INTERCEPT_MODEL_DIR=/path/to/model`.

## Reverse proxy (Nginx)

```nginx
server {
    listen 443 ssl;
    server_name proxy.example.com;

    ssl_certificate /etc/letsencrypt/live/proxy.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/proxy.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_buffering off;  # Required for streaming
        proxy_cache off;      # StackIntercept has its own cache
    }
}
```

## Firewall

```bash
# Allow proxy port
ufw allow 8080/tcp

# Restrict to trusted IPs
ufw allow from 10.0.0.0/8 to any port 8080
```

## Resource limits

- **Memory (exact-only)**: ~50-100 MB for 20,000 cached entries
- **Memory (semantic)**: ~300-500 MB (model + index + vectors)
- **Disk**: ~10 MB per 1,000 cached responses (exact), plus vectors for semantic
- **CPU**: Negligible for exact cache; semantic embedding ~100ms per request (CPU)

## Upgrading

```bash
# Download new binary
systemctl stop stack-intercept
# Replace binary
systemctl start stack-intercept
# Cache snapshot is read automatically on startup
```

## Production safety checklist

- [ ] TLS terminates before traffic reaches the public internet
- [ ] API keys are never sent over plaintext public networks
- [ ] `LimitNOFILE` is raised for high-concurrency deployments
- [ ] Cache path points to a writable directory if persistence enabled
- [ ] Process runs as a dedicated non-root user
- [ ] Routing remains opt-in via `STACK_INTERCEPT_ALLOW_MODEL_REWRITE=true`
- [ ] Reactive failover remains opt-in via `STACK_INTERCEPT_REACTIVE_FAILOVER=true`
- [ ] Sensitive workloads use `x-stack-intercept-no-route: true` when model rewriting is not acceptable
- [ ] Sensitive workloads use `x-stack-intercept-no-semantic-cache: true` when semantic cache reuse is not acceptable

## Health check

```bash
curl -X POST http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $DEEPSEEK_API_KEY" \
  -d '{"model":"deepseek-chat","messages":[{"role":"user","content":"ping"}],"temperature":0,"max_tokens":1}'
```

A 200 response with `x-stack-intercept: miss` or `x-stack-intercept: hit` means the proxy is healthy.
