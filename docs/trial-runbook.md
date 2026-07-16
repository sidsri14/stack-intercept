# StackIntercept Trial Runbook

Use this runbook to test StackIntercept in staging without changing application logic beyond the SDK base URL.

## Goal

Validate three things:

- Exact cache hits replay locally for deterministic repeated requests.
- Prometheus metrics expose cache behavior per tenant.
- The proxy can be removed by switching the SDK base URL back to the provider.

## Trial Defaults

Use exact-cache mode first:

```bash
docker compose -f docker-compose.trial.yml up --build
```

This trial profile intentionally disables:

- semantic cache
- model rewriting
- reactive failover
- external telemetry

## Required Environment

Set one provider key in your shell or `.env`:

```bash
OPENAI_API_KEY=sk-your-openai-key
STACK_INTERCEPT_ADMIN_KEY=replace-this
```

For OpenAI-compatible providers, set:

```bash
STACK_INTERCEPT_UPSTREAM_URL=https://api.openai.com
```

## SDK Change

Change only `base_url` / `baseURL`:

```python
import os
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key=os.environ["OPENAI_API_KEY"],
)
```

```js
import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "http://127.0.0.1:8080/v1",
  apiKey: process.env.OPENAI_API_KEY,
});
```

## Verification

Check config:

```bash
curl http://127.0.0.1:8080/admin/config \
  -H "x-admin-key: $STACK_INTERCEPT_ADMIN_KEY"
```

Check Prometheus metrics:

```bash
curl http://127.0.0.1:8080/admin/metrics/prometheus \
  -H "x-admin-key: $STACK_INTERCEPT_ADMIN_KEY"
```

Expected counters after repeated deterministic requests:

- `stack_intercept_misses` increases on the first request
- `stack_intercept_exact_hits` increases on repeated identical requests
- tenant labels appear when `x-tenant-id` is sent

## Tenant Test

Send the same request with different tenants:

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H "x-tenant-id: trial-a" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Say pong."}],"temperature":0}'
```

Repeat with `x-tenant-id: trial-b`. The first request per tenant should miss; repeated requests for the same tenant should hit.

## Rollback

Rollback is one config change:

- Python: set `base_url` back to the provider URL.
- Node: set `baseURL` back to the provider URL.
- Infrastructure: stop the trial container.

```bash
docker compose -f docker-compose.trial.yml down
```

## Trial Report

Record:

- provider
- model
- request volume
- exact cache hit count
- miss count
- latency observed at application boundary
- any non-2xx provider responses
- whether SDK behavior changed
