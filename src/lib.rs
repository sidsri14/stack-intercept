//! StackIntercept library crate.
//!
//! The binary (`src/main.rs`) and microbenchmarks (`benches/`) import the hot
//! paths from here so they are never duplicated. Keeping the SIMD dot product
//! in the library crate means the benchmark exercises exactly the code the
//! proxy ships — no copy-paste drift.

pub mod simd;
