# Self-Hosted Production Deployment Notes

These notes are for teams running `stack-intercept` as a self-hosted service outside a single-machine localhost workflow.

## 1. Operating System Open File Limits (FDs)

By default, many Linux environments start services with a low per-process file descriptor limit. High-concurrency reverse proxies can saturate this limit, causing `EMFILE (Too many open files)` connection drops when the process can no longer accept client or upstream sockets.

Before launching the binary under production concurrency, increase the descriptor limit.

```bash
# Set limits in the current shell session
ulimit -n 65535
```

For persistent `systemd` service execution, add `LimitNOFILE=65535` to the service unit under the `[Service]` block:

```ini
[Service]
ExecStart=/usr/local/bin/stack-intercept
LimitNOFILE=65535
```

After starting the service, verify the active process limit:

```bash
cat /proc/$(pgrep stack-intercept)/limits | grep "Max open files"
```

## 2. TLS Termination & Upstream Security

The `stack-intercept` binary listens on raw HTTP sockets. This keeps the proxy simple and fast for local or internal-network deployments, but it means the binary should not be exposed directly over the public internet.

Do not pass API keys or corporate traffic over an unencrypted public connection. For remote or multi-host deployments, place the proxy behind a dedicated TLS termination layer such as:

- AWS Application Load Balancer
- Cloudflare Tunnel
- Nginx
- Caddy
- Envoy / internal service mesh ingress

Recommended deployment shape:

```text
Client SDK -> HTTPS/TLS ingress -> internal HTTP -> stack-intercept -> upstream LLM provider
```

For local development or single-machine usage, keep the proxy bound to `127.0.0.1` and point SDK clients at `http://127.0.0.1:8080/v1`.

## 3. State Persistence Paths

When `STACK_INTERCEPT_CACHE_PATH` is configured, `stack-intercept` persists exact and semantic cache snapshots to disk using MessagePack. This allows warm-cache restoration across restarts.

Example:

```bash
export STACK_INTERCEPT_CACHE_PATH=/var/cache/stack-intercept/snapshot.msgpack
```

Make sure the runtime user can write to the target directory:

```bash
sudo mkdir -p /var/cache/stack-intercept
sudo chown stack-intercept:stack-intercept /var/cache/stack-intercept
```

Persistence can be disabled explicitly:

```bash
export STACK_INTERCEPT_DISABLE_PERSISTENCE=true
```

For `systemd`, include the same values in the service unit:

```ini
[Service]
Environment=STACK_INTERCEPT_CACHE_PATH=/var/cache/stack-intercept/snapshot.msgpack
Environment=STACK_INTERCEPT_DISABLE_PERSISTENCE=false
```

## 4. Minimal systemd Service Example

```ini
[Unit]
Description=StackIntercept local LLM cost-control proxy
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=stack-intercept
Group=stack-intercept
ExecStart=/usr/local/bin/stack-intercept
Restart=on-failure
RestartSec=2
LimitNOFILE=65535
Environment=STACK_INTERCEPT_CACHE_MODE=exact
Environment=STACK_INTERCEPT_UPSTREAM_URL=https://api.deepseek.com
Environment=STACK_INTERCEPT_CACHE_PATH=/var/cache/stack-intercept/snapshot.msgpack

[Install]
WantedBy=multi-user.target
```

## 5. Production Safety Checklist

Before exposing the proxy beyond localhost, verify:

- TLS terminates before traffic reaches the public internet.
- API keys are never sent over plaintext public networks.
- `LimitNOFILE` is raised for high-concurrency deployments.
- `STACK_INTERCEPT_CACHE_PATH` points to a writable directory if persistence is enabled.
- The process runs as a dedicated non-root user.
- Routing remains opt-in via `STACK_INTERCEPT_ALLOW_MODEL_REWRITE=true`.
- Sensitive workloads use `x-stack-intercept-no-route: true` when model rewriting is not acceptable.
