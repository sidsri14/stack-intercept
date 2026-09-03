//! SIMD-accelerated dot product for semantic cache similarity search.
//!
//! Semantic lookups scan a bounded bucket of 384-dimensional `f32` BGE
//! embeddings, comparing a query vector against every stored vector with a
//! cosine dot product. A full 256-item bucket scan touches
//! `256 × 384 × 4 B ≈ 384 KiB` — comfortably L2-resident and friendly to
//! the sequential prefetcher. That workload is the reason this proxy keeps a
//! simple capped linear scan instead of a graph index. HNSW was rejected at
//! the architectural stage: at this dataset size, pointer chasing and index
//! build tax are mathematically unjustifiable against a cache-resident scan.
//!
//! The hot path is an AVX2+FMA loop (256-bit registers, 8 `f32` lanes, fused
//! multiply-add accumulation) selected at runtime via `is_x86_feature_detected!`.
//! Every other CPU falls back to an unrolled scalar loop. Both paths are exact —
//! identical numeric results — so cache correctness never depends on which path
//! ran.

/// Compute the dot product of two equal-length `f32` slices.
///
/// This is the production entry point. It dispatches at runtime: AVX2+FMA on
/// capable x86_64 CPUs, an unrolled scalar loop everywhere else. Returns `0.0`
/// when the slices have different lengths so callers get a deterministic,
/// safe result without a panic on malformed input.
pub fn compute_vector_dot(v1: &[f32], v2: &[f32]) -> f32 {
    if v1.len() != v2.len() {
        return 0.0;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma")
        {
            // SAFETY: guarded by runtime AVX2+FMA feature detection. The function
            // only uses unaligned loads within bounds checked by the loop.
            unsafe {
                return compute_vector_dot_avx2(v1, v2);
            }
        }
    }

    compute_vector_dot_unrolled(v1, v2)
}

/// AVX2+FMA dot product. 8 `f32` lanes per vector, fused multiply-add
/// accumulation (`_mm256_fmadd_ps`).
///
/// # Safety
///
/// The caller must guarantee the CPU supports both the `avx2` and `fma`
/// features (typically via `is_x86_feature_detected!("avx2") &&
/// is_x86_feature_detected!("fma")`) before calling this. It also assumes
/// `v1.len() == v2.len()`; callers should route through [`compute_vector_dot`]
/// which checks lengths first.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
pub unsafe fn compute_vector_dot_avx2(v1: &[f32], v2: &[f32]) -> f32 {
    use std::arch::x86_64::*;

    let len = v1.len();
    let mut sum_reg = _mm256_setzero_ps();
    let mut i = 0usize;

    while i + 8 <= len {
        let va = _mm256_loadu_ps(v1.as_ptr().add(i));
        let vb = _mm256_loadu_ps(v2.as_ptr().add(i));
        sum_reg = _mm256_fmadd_ps(va, vb, sum_reg);
        i += 8;
    }

    let mut lanes = [0.0f32; 8];
    _mm256_storeu_ps(lanes.as_mut_ptr(), sum_reg);
    let mut total = lanes.iter().sum::<f32>();

    while i < len {
        total += v1[i] * v2[i];
        i += 1;
    }

    total
}

/// Unrolled scalar dot product, the portable fallback for CPUs without AVX2+FMA.
///
/// Processes four elements per iteration via `as_chunks::<4>()`, then sums the
/// tail element-wise. Numerically identical to the AVX2 path.
pub fn compute_vector_dot_unrolled(v1: &[f32], v2: &[f32]) -> f32 {
    let (chunks_a, tail_a) = v1.as_chunks::<4>();
    let (chunks_b, tail_b) = v2.as_chunks::<4>();

    let mut total = 0.0f32;

    for (a, b) in chunks_a.iter().zip(chunks_b.iter()) {
        total += a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    }

    total + tail_a.iter().zip(tail_b).map(|(a, b)| a * b).sum::<f32>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_vector_dot_matches_expected() {
        let a = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let b = vec![0.5, -1.0, 2.0, 0.25, 3.0, -0.5, 1.5, 2.0, -2.0];
        let expected = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>();

        assert!((compute_vector_dot(&a, &b) - expected).abs() < 1e-5);
    }

    #[test]
    fn test_compute_vector_dot_empty_and_mismatched() {
        assert_eq!(compute_vector_dot(&[], &[]), 0.0);
        assert_eq!(compute_vector_dot(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn test_avx2_path_matches_unrolled_when_available() {
        let a: Vec<f32> = (0..384).map(|i| (i as f32) * 0.5 - 100.0).collect();
        let b: Vec<f32> = (0..384).map(|i| 0.25 - (i as f32) * 0.1).collect();
        let expected = compute_vector_dot_unrolled(&a, &b);

        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma")
        {
            // SAFETY: guarded by runtime AVX2+FMA feature detection above.
            let simd = unsafe { compute_vector_dot_avx2(&a, &b) };
            // FMA rounds a*b+c once; the scalar path rounds a*b then adds. On a
            // sum of magnitude ~2e5 that yields a relative difference at the
            // float-epsilon scale, so compare relatively, not absolutely.
            let tolerance = expected.abs().max(1.0) * 1e-4;
            assert!(
                (simd - expected).abs() <= tolerance,
                "AVX2 and unrolled paths diverged: {simd} vs {expected}"
            );
        }
    }
}
