# Why We Rejected HNSW for an AVX2/FMA Linear Scan in an In-VPC Rust LLM Proxy

*StackIntercept is an open-source, in-VPC Rust proxy for OpenAI-compatible SDKs. It intercepts chat-completion calls to add an exact SHA-256 cache, an opt-in semantic cache, and transparent single-hop failover. This post explains one architectural decision — why its semantic cache does not use a vector index — and the measurement that justified it.*

Every LLM caching layer eventually gets the question: "Why aren't you using a vector database for semantic search?" The default mental model comes from RAG systems — millions of embeddings, approximate nearest-neighbor (ANN) indexes, HNSW graphs with configurable M and efConstruction. That instinct is wrong for a proxy sitting in front of a chat API, and in this post I want to show the reasoning and the numbers, because the reasoning is *scale*, not vibes.

---

## 1. The Context: A vector database is an anti-pattern at this scale

StackIntercept's semantic cache works like this: a request arrives, we embed the user's last message with a local BGE-small model, and we look for a matching stored response. To keep hits *safe*, matches are never computed against the whole corpus. They're computed inside a **per-context bucket**: everything in the request except the last user message (system prompt, tenant, prior turns, model) forms a key, and the embedding is only compared against other embeddings that share that exact context. This is a deliberate safety decision — it prevents cross-tenant and cross-prompt cache collisions.

The practical consequence is that a single lookup scans **at most 256 embeddings of 384 dimensions** — the bucket cap. That's the entire dataset the search ever touches:

```
256 vectors × 384 dims × 4 bytes = 393,216 bytes ≈ 384 KiB
```

**384 KiB.** That fits in the L2 cache of any modern x86 core, and it is fully sequential memory — the hardware prefetcher's favorite access pattern.

Now consider what an HNSW graph would add *on top of* that 384 KiB:

- **Pointer chasing.** Graph traversal is random access by definition. Every hop is a potential TLB miss and cache miss — the exact opposite of a linear, prefetcher-friendly scan.
- **Index build tax.** Every cache write is also a graph insertion: candidate selection, pruning, neighbor updates. That's work on the *write path* of a cache that exists to make writes cheap.
- **Serialization tax.** A vector database is a server you don't need. All the data already lives in-process. Shipping it over a wire, or even into a separate index structure, costs more than the search it enables.
- **Complexity.** An ANN index is probabilistic — it can *miss* a real match and return a wrong "closest." A linear scan is exact and deterministic.

This is why we frame it as an **architectural rejection, not a benchmarked loss**. We did not build an HNSW index, measure it, and find it slower (though we'd be surprised otherwise). We rejected the premise: an index structure exists to avoid scanning data that doesn't fit in cache. At N ≤ 256 and 384 KiB, there is no data that doesn't fit in cache. The overhead of indexing is mathematically unjustifiable, so we never shipped it. (The `fast-hnsw` dependency we'd considered was removed; the design is explicitly out of scope for v0.3.0.)

---

## 2. The Micro-Benchmark: Why a sequential AVX2/FMA scan wins

The scan budget is worth writing down before measuring. Each 384-dim dot product is 384 fused multiply-adds. A full 256-vector bucket scan is:

```
256 × 384 = 98,304 element-wise FMAs
98,304 / 8 lanes = 12,288 AVX2 FMA instructions
```

12,288 instructions, all on 384 KiB of L2-resident data. At typical issue rates this is *single-digit microseconds* of vector work. The question was whether the implementation could get close to that ceiling — so we built a microbenchmark rather than guessing.

The benchmark lives in the repo at `benches/dot_product.rs` and measures the exact production kernel. The crate is split into a library (`src/lib.rs`) and a binary (`src/main.rs`), and the benchmark imports directly from the library:

```rust
use stack_intercept::simd::{
    compute_vector_dot, compute_vector_dot_avx2, compute_vector_dot_unrolled,
};
```

No copy-pasted benchmark body, no drift between "benchmark code" and "shipped code" — the bench calls the same functions the proxy serves requests with. The kernel is a runtime-dispatched dot product:

- **AVX2/FMA path** (`compute_vector_dot_avx2`): gated on `#[target_feature(enable = "avx2,fma")]`, selected only when both features are detected at runtime, and uses fused multiply-add accumulation (`_mm256_fmadd_ps` — 8 `f32` lanes, one rounding per `a·b+c`).
- **Unrolled scalar fallback** (`compute_vector_dot_unrolled`): four elements per iteration via `as_chunks::<4>()`, numerically identical to the SIMD path up to float-epsilon ordering effects.

Measured on one dev machine (a Windows x86-64 desktop; `avx2=true fma=true avx512f=false`), single-threaded, warm cache, `cargo bench`:

| Path | Single dot (384 dims) | Full bucket scan (256 × 384) | GFLOPS |
|---|---|---|---|
| **AVX2+FMA** | 95 ns | **9.50 µs** | 20.7 |
| **dispatcher** (production) | 93 ns | **9.88 µs** | 19.9 |
| **unrolled** scalar | 151 ns | **27.0 µs** | 7.3 |
| **naive** iterator | 377 ns | **87.3 µs** | 2.3 |

Reading the numbers:

- The **full bucket scan is ~9.5 µs** on the AVX2/FMA path — comfortably single-digit microseconds, matching the 12,288-instruction budget analysis. That's **37 ns per candidate vector**.
- AVX2/FMA is **2.8× faster than the unrolled scalar fallback** and **9.2× faster than a naive iterator**. The naive version isn't a strawman — `Iterator::zip().map().sum()` is the first implementation everyone writes, and it gets 2.25 GFLOPS because the compiler doesn't auto-vectorize the horizontal reduction reliably.
- The runtime dispatcher costs ~0.4 µs per scan (9.88 vs 9.50 µs) — the price of `is_x86_feature_detected!` on every call. For a per-request lookup that runs at most a handful of times, that's noise; we keep the production entry point simple and safe.

To be clear about what these numbers are *not*: they are warm-cache, single-threaded, single-core measurements on one machine. They are not a cross-platform benchmark, and we make no latency-percentile claims. But for the claim that matters — *"a full semantic lookup is bounded single-digit microseconds in L2"* — the margin over the alternatives (unrolled 27 µs, naive 87 µs) is wide enough that the conclusion is robust: **at this dataset size, a sequential AVX2/FMA scan is the right structure, and no index could pay for itself.**

---

## 3. The Resilience Angle: Single-hop failover beats a better index

Once your semantic scan is bounded to single-digit microseconds in L2, the proxy's real bottleneck isn't vector math — **it's upstream API volatility**. This is why we stopped over-optimizing vector indexes and focused on the part of the system that actually determines whether a request succeeds: the connection to the model provider.

An LLM proxy lives or dies by what happens when the upstream is unhappy. Providers return `429` rate-limits and `5xx` outages constantly, in-VPC or not. A cache miss is not rare — most traffic is genuinely new. So the failure mode that matters most is: *we missed the cache, the upstream errored, and the user gets an error instead of a completion.*

StackIntercept's answer is **reactive single-hop failover**: when the primary upstream fails with a transport error or a configured status code (`429, 500, 502, 503, 504` by default), the proxy transparently re-dispatches the request to a fallback provider — optionally rewriting the model name (e.g. `gpt-4o` → `deepseek-chat`) so the retry is cost-effective. It's a single hop, not a retry storm: exactly one failover attempt, then the error surfaces. It is off by default *in effect* — it stays a no-op until a fallback API key is configured — and it never retries on the path that would violate a caller's assumptions. Each failover increments a `reactive_failovers` counter on the admin metrics endpoint, so the behavior is observable, not magic.

Why does this matter more than a fancier index? A semantic cache improves the *happy path* (cache hit latency). Failover decides whether the *unhappy path* produces a served request or a hard failure. In production, the unhappy path is the one users remember. A 429 that gets transparently served by the fallback is invisible; a 429 that surfaces to the client is a ticket. Optimizing an index that the happy path doesn't need is optimizing the wrong side of the SLO.

---

## 4. The Code

Everything in this post is reproducible. The repo is MIT-licensed and open source:

**https://github.com/sidsri14/stack-intercept**

The files that back the claims above:

- `src/simd.rs` — the AVX2/FMA kernel, the unrolled fallback, and the runtime dispatcher (with tests asserting the two paths agree within float-epsilon).
- `benches/dot_product.rs` — the microbenchmark; run it yourself with `cargo bench` (no API keys or model weights required).
- `src/main.rs` — the reactive failover dispatch and the `reactive_failovers` metric.
- `test_failover.py` — integration tests for connection-refused failover, 5xx handling, model rewriting, and disabled mode.

If you're building a caching layer in front of LLM APIs, steal the decision procedure rather than the code: **measure your actual lookup dataset before reaching for an index, and spend your resilience budget on the upstream — not the vector math.**
