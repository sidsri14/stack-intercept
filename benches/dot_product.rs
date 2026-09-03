//! Dot-product microbenchmark for the semantic cache similarity scan.
//!
//! Measures the production workload: one query vector against a full 256-item
//! bucket of 384-dim BGE embeddings (`256 × 384 × 4 B ≈ 384 KiB`), plus a
//! single-vector baseline. Compares four paths, all imported directly from the
//! library crate (`stack_intercept::simd`) so the numbers reflect exactly the
//! code the proxy ships:
//!
//!   1. `dispatcher` — production entry point (runtime AVX2+FMA dispatch)
//!   2. `avx2`       — raw AVX2+FMA kernel (measured only when the CPU has it)
//!   3. `unrolled`   — unrolled scalar fallback
//!   4. `naive`      — plain iterator baseline
//!
//! Run with `cargo bench` (or `cargo bench --bench dot_product`).
//! No Criterion dependency — a simple timer is plenty for a straight-line loop.

use std::hint::black_box;
use std::time::Instant;

use stack_intercept::simd::{
    compute_vector_dot, compute_vector_dot_avx2, compute_vector_dot_unrolled,
};

const DIMS: usize = 384;
const BUCKET: usize = 256;

/// 384-dim cosine workloads: one query vector plus a full 256-vector bucket.
fn build_vectors() -> (Vec<f32>, Vec<Vec<f32>>) {
    let query: Vec<f32> = (0..DIMS)
        .map(|i| ((i as f32) * 0.017).sin() * 0.5)
        .collect();
    let bucket: Vec<Vec<f32>> = (0..BUCKET)
        .map(|r| {
            (0..DIMS)
                .map(|i| (((i + r) as f32) * 0.013).cos() * 0.25)
                .collect()
        })
        .collect();
    (query, bucket)
}

/// Run `f` repeatedly for ~300 ms and report median-equivalent ns/call.
fn bench_ns<F: FnMut() -> f32>(label: &str, mut f: F) -> f64 {
    // Warm up so the first call's cold misses are not in the measurement.
    for _ in 0..100 {
        black_box(f());
    }
    let start = Instant::now();
    let mut iters: u64 = 0;
    let mut sink = 0.0f32;
    while start.elapsed().as_secs_f64() < 0.3 {
        sink += black_box(f());
        iters += 1;
    }
    black_box(sink);
    let ns_per = start.elapsed().as_secs_f64() * 1e9 / iters as f64;
    println!("  {label:<40} {ns_per:>10.1} ns/call");
    ns_per
}

fn main() {
    let (query, bucket) = build_vectors();
    let query = black_box(query);
    let bucket = black_box(bucket);
    let vecs: Vec<&[f32]> = bucket.iter().map(|v| v.as_slice()).collect();
    let query_slice = query.as_slice();

    let scan_flops = (BUCKET * DIMS * 2) as f64; // 1 mul + 1 add per element

    println!("=== dot-product microbenchmark ===");
    println!(
        "  workload: {BUCKET} vectors x {DIMS} dims = {} B/bucket scan (~{} KiB, L2-resident)",
        BUCKET * DIMS * 4,
        (BUCKET * DIMS * 4) / 1024
    );
    #[cfg(target_arch = "x86_64")]
    println!(
        "  cpu: avx2={} fma={} avx512f={}",
        std::arch::is_x86_feature_detected!("avx2"),
        std::arch::is_x86_feature_detected!("fma"),
        std::arch::is_x86_feature_detected!("avx512f"),
    );
    println!();

    // --- single dot product (384 dims) ---
    let single = bucket[0].as_slice();
    println!("single dot (384 dims):");
    bench_ns("dispatcher", || compute_vector_dot(query_slice, single));
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma") {
        // SAFETY: guarded by runtime AVX2+FMA feature detection above.
        bench_ns("avx2+fma", || unsafe {
            compute_vector_dot_avx2(query_slice, single)
        });
    }
    bench_ns("unrolled", || {
        compute_vector_dot_unrolled(query_slice, single)
    });
    bench_ns("naive", || {
        query_slice
            .iter()
            .zip(single.iter())
            .map(|(a, b)| a * b)
            .sum::<f32>()
    });
    println!();

    // --- full bucket scan (256 x 384) ---
    println!("full bucket scan (256 x 384):");
    let scan_dispatcher = bench_ns("dispatcher", || {
        vecs.iter()
            .map(|v| compute_vector_dot(query_slice, v))
            .sum()
    });
    #[cfg(target_arch = "x86_64")]
    let scan_avx2 = if std::arch::is_x86_feature_detected!("avx2")
        && std::arch::is_x86_feature_detected!("fma")
    {
        // SAFETY: guarded by runtime AVX2+FMA feature detection above.
        Some(bench_ns("avx2+fma", || {
            vecs.iter()
                .map(|v| unsafe { compute_vector_dot_avx2(query_slice, v) })
                .sum()
        }))
    } else {
        None
    };
    let scan_unrolled = bench_ns("unrolled", || {
        vecs.iter()
            .map(|v| compute_vector_dot_unrolled(query_slice, v))
            .sum()
    });
    let scan_naive = bench_ns("naive", || {
        vecs.iter()
            .map(|v| {
                query_slice
                    .iter()
                    .zip(v.iter())
                    .map(|(a, b)| a * b)
                    .sum::<f32>()
            })
            .sum()
    });
    println!();

    println!("=== results ===");
    let report = |label: &str, ns: f64| {
        if ns > 0.0 {
            println!(
                "  {label:<40} {ns:>8.2} ns/scan   {:>7.2} us   {:>7.2} GFLOPS   {:>6.1} ns/item",
                ns / 1000.0,
                scan_flops / ns,
                ns / BUCKET as f64
            );
        }
    };
    report("dispatcher", scan_dispatcher);
    #[cfg(target_arch = "x86_64")]
    if let Some(ns) = scan_avx2 {
        report("avx2+fma", ns);
    }
    report("unrolled", scan_unrolled);
    report("naive", scan_naive);
}
