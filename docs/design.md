# StackIntercept Design Notes

StackIntercept is a local OpenAI-compatible proxy. It is designed to reduce repeated LLM calls while keeping behavior explicit and inspectable.

## Request Flow

```text
Client
  |
  | POST /v1/chat/completions
  v
StackIntercept
  |
  |-- load runtime config
  |-- validate request cache/routing eligibility
  |-- evaluate opt-in model routing
  |-- check exact cache
  |-- check semantic cache, if enabled
  |-- forward to upstream or fallback provider
  |-- cache successful deterministic response
  |-- return provider response with transparent headers
```

## Exact Cache Safety

Exact caching is only used for deterministic and replay-safe requests.

Key rules:

- Requests with `temperature` greater than `0` are not cached.
- Requests with tools are not cached.
- Requests with structured output are not cached.
- Requests with `cache_control: no_store` are not cached.
- Non-2xx upstream responses are not cached.
- Cache keys include provider identity and routing namespace.
- Tenant isolation is available through `STACK_INTERCEPT_TENANT_ID_HEADER`.

The exact cache key is built from the canonical request payload plus provider, tenant, and routing context. This prevents routed and passthrough responses from sharing a cache entry.

## Semantic Cache Safety

Semantic caching is opt-in through `STACK_INTERCEPT_CACHE_MODE=semantic`.

The semantic cache does not scan every stored prompt globally. Instead, it first computes a context key from stable conversation context:

- system prompt
- earlier conversation messages
- model
- tenant
- tools schema, when relevant to eligibility

The final user message is excluded from the context key and embedded separately. Similarity search only happens inside the matching context bucket. This limits semantic reuse to requests that have equivalent surrounding context.

Current safety controls:

- Local embeddings are used.
- Similarity threshold is high by default.
- Items expire through TTL.
- Semantic mode requires local model files.
- Streaming responses are not semantically deduplicated.

## Model Routing Safety

Model routing is disabled by default. It is enabled only when `STACK_INTERCEPT_ALLOW_MODEL_REWRITE=true`.

Routing is blocked for cases where changing models can materially change correctness:

- tools/function calling
- structured outputs
- multimodal requests
- non-deterministic requests
- prompts that explicitly require a specific model
- security, legal, financial, cryptography, race-condition, and distributed-systems style prompts

When routing is used, StackIntercept returns transparent headers:

- `x-stack-intercept-route`
- `x-stack-intercept-original-model`
- `x-stack-intercept-routed-model`

Routing also uses a separate cache namespace, so routed and original-provider responses do not collide.

## Admin Surface

Admin routes are under `/admin`.

Loopback access is allowed without an admin key for local development. Remote bindings require `STACK_INTERCEPT_ADMIN_KEY`.

Admin endpoints expose:

- metrics
- cache summary
- cache flush
- exact-entry eviction
- semantic-bucket eviction
- masked runtime config

Secrets are never returned in full from config endpoints.

## Non-Goals

StackIntercept is intentionally not:

- a full API gateway
- a load balancer
- a user-auth system
- a provider failover service
- a replacement for provider-side evaluation or safety systems

The current design focuses on local cost control, transparent behavior, and conservative caching/routing defaults.
