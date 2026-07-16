# Security Notes

StackIntercept is designed for self-hosted deployment inside your own network path.

## Data Flow

```
Client app -> StackIntercept -> LLM provider
```

StackIntercept does not send prompts, responses, or metrics to any external telemetry service. Admin metrics are exposed only through the local `/admin/*` HTTP routes.

## Secrets

- Provider API keys are accepted through request `Authorization` headers or environment variables.
- Admin API access should use `STACK_INTERCEPT_ADMIN_KEY` outside local loopback development.
- Do not commit `.env`, provider keys, cache snapshots, or model provider credentials.
- Prefer a secret manager for production deployments.

## Network Boundary

For production or shared staging:

- keep StackIntercept behind private networking or TLS termination
- do not expose port `8080` directly to the public internet
- set `STACK_INTERCEPT_ADMIN_KEY`
- restrict admin routes at the ingress/firewall layer when possible

## Caching Boundary

Exact cache keys include provider, tenant, routing namespace, and canonical request payload.

Semantic cache is opt-in. It hashes stable conversation context first and only scans within the matching context bucket. Use this header when semantic reuse is not acceptable for a request:

```text
x-stack-intercept-no-semantic-cache: true
```

Use this header when model routing is not acceptable:

```text
x-stack-intercept-no-route: true
```

## Persistence

If `STACK_INTERCEPT_CACHE_PATH` is set, exact and semantic cache snapshots are written to disk using MessagePack. Treat these files as sensitive because they may contain cached provider responses.

Recommended:

- store snapshots on encrypted disks
- restrict file permissions to the service user
- disable persistence for highly sensitive trials:

```bash
STACK_INTERCEPT_DISABLE_PERSISTENCE=true
```

## Non-Goals

StackIntercept is not an identity provider, WAF, billing engine, or full API gateway. It currently does not enforce per-user auth, hard token budgets, request signing, or tenant rate limits.

