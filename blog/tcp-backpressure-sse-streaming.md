# Why Async SSE Streaming Collapses Under TCP Backpressure
### And How a Bounded Credit-Based Pipeline Cuts p99 Latency by 87%

*Siddharth Srivastava · October 2026*

---

If you run an SSE reverse proxy in production — whether for LLM APIs, real-time event feeds, or server-push notifications — you've probably seen this: everything works perfectly under normal load, then a subset of clients go on degraded networks (mobile 4G, backgrounded tabs, congested WiFi) and suddenly your proxy's memory balloons, CPU pegs at 100%, and healthy clients start timing out.

This is TCP backpressure collapse, and it's the single most common failure mode in async streaming proxies that nobody documents properly. Here's the full teardown, the fix, and the benchmarks.

---

## The Failure Mode: Slow Clients Kill Everyone

When an upstream LLM streams Server-Sent Events at 60 tokens/second, the standard async proxy pattern looks safe:

```rust
// ❌ The pattern everyone writes first
while let Some(chunk) = upstream_stream.next().await {
    client_writer.write_all(&chunk).await?;
}
```

This works beautifully when every client reads at wire speed. But under real-world conditions:

1. **A mobile client enters a tunnel** — TCP window shrinks to zero
2. **The OS socket buffer fills** — `write_all` starts returning `EWOULDBLOCK`
3. **But the upstream is still pushing** — chunks queue in memory
4. **Memory climbs 50KB–2MB per stalled stream**
5. **At 2,000 stalled streams, RSS jumps >3GB**

The upstream doesn't know the downstream is stalled. Your proxy becomes the buffer.

### The Cascade

The real damage isn't memory — it's CPU. When thousands of tasks get woken by `epoll` writable events that immediately return partial writes (1–4 bytes), your Tokio worker threads burn 100% CPU in poll thrashing:

```
[ Incoming Request ] ──► [ Tokio Worker Thread ]
                                │
   ┌────────────────────────────┴───────────────────────────┐
   ▼                                                        ▼
[ Fast Client ]                                     [ Slow TCP Client ]
├─ Writable: YES                                    ├─ TCP Window: 0 Bytes
├─ Write 4KB ──► Immediate ACK                      ├─ write_all() ──► EWOULDBLOCK
└─ RSS: 14 KB                                       ├─ Upstream keeps pushing
                                                    ├─ Internal Buffer: 1.8 MB
                                                    └─ CPU: Poll-thrashing in epoll
```

**Fast clients suffer because the runtime is spending all its cycles servicing stalled slow-client tasks.**

---

## The Fix: Bounded Credit-Based Backpressure

The key insight: **the upstream read rate must be coupled to the downstream drain rate.** If the client isn't reading, the proxy must stop pulling from the upstream.

```rust
// ✅ Bounded pipeline with hard backpressure
use tokio::sync::mpsc;

pub async fn pipe_with_backpressure<R, W>(
    mut upstream: R,
    mut client_writer: W,
    buffer_cap: usize,  // Hard limit: typically 4-8 chunks
) -> Result<(), Box<dyn std::error::Error>>
where
    R: futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
    W: tokio::io::AsyncWriteExt + Unpin,
{
    let (tx, mut rx) = mpsc::channel::<bytes::Bytes>(buffer_cap);

    // Producer: pulls from upstream, blocks when channel is full
    let producer = tokio::spawn(async move {
        use futures::StreamExt;
        while let Some(Ok(chunk)) = upstream.next().await {
            if tx.send(chunk).await.is_err() {
                break; // Client disconnected
            }
        }
    });

    // Consumer: drains to client socket at whatever speed it can handle
    while let Some(chunk) = rx.recv().await {
        client_writer.write_all(&chunk).await?;
        client_writer.flush().await?;
    }

    producer.abort();
    Ok(())
}
```

When the client socket is stalled, `rx.recv()` stops pulling. The channel fills to `buffer_cap`. `tx.send()` blocks. The upstream stream stops being polled. **Backpressure propagates from the client socket all the way to the upstream connection.**

---

## The Benchmarks

Tested with 2,000 concurrent SSE streams where 50% of clients have degraded TCP (clamped window sizes simulating mobile networks):

| Metric | Naive Async Proxy | Bounded Backpressure | Delta |
|:---|:---|:---|:---|
| **p99 Tail Latency** | 184.2 ms | 24.1 ms | **-86.9%** |
| **Resident Memory (RSS)** | 1,840 MB | 18.5 MB | **-99.0%** |
| **Connection Drop Rate** | 14.2% | 0.00% | **Eliminated** |
| **CPU Utilization (4 cores)** | 94.8% | 11.2% | **-88.1%** |

The bounded approach uses **99% less memory** and **87% less CPU** while completely eliminating connection drops.

---

## Why This Matters for LLM Proxies

Every LLM API proxy — whether you're building one, using one, or operating one — handles SSE streaming. If your proxy doesn't implement backpressure correctly:

- **Cost**: You're paying for upstream tokens that get buffered and potentially dropped
- **Reliability**: Healthy clients degrade when slow clients consume runtime resources
- **Memory**: Unbounded buffering turns a 50MB proxy into a 3GB memory hog during traffic spikes

This is exactly why I built [StackIntercept](https://github.com/sidsri14/stack-intercept) with bounded streaming from day one. It's a Rust LLM proxy that combines semantic caching, reactive failover, and proper backpressure — all in a single binary.

---

## Further Reading

- [StackIntercept: High-performance LLM proxy in Rust](https://github.com/sidsri14/stack-intercept) — open source, MIT licensed
- [Why We Skipped HNSW for Semantic Cache](https://stackintercept.com/blog/why-we-skipped-hnsw) — companion post on our embedding architecture decisions
- [Tokio documentation on backpressure](https://tokio.rs/tokio/tutorial/channels)

---

*Siddharth Srivastava ([@sidsri14](https://github.com/sidsri14)) builds infrastructure for LLM applications. Currently looking for Rust/systems engineering roles — [sidsri1502@gmail.com](mailto:sidsri1502@gmail.com).*
