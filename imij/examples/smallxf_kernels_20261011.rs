//! Small-transform study (2026-10-11): decoder kernel cost per block plane
//! (dequantization + inverse, random dense coefficients, no flat blocks).
//!
//! `smallxf_kernels_20261011 BLOCKS ITERS ROUNDS KERNEL...` prints one line
//! per kernel: `kernel<TAB>ns per block plane` (median of the rounds). The
//! SIMD tier follows `IMIJ_SCALAR=1` / `IMIJ_NO_AVX512=1`; run one process
//! per tier. Kernels: `aan8` (8x8 decoder kernel), `f4` (normative 4x4
//! kernel), `int4` (integer candidate), `ref4`, `ref8` (scalar references),
//! `dense4` (record-19 4x4 layout), `copy` (the per-pass coefficient copy).

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: smallxf_kernels_20261011 BLOCKS ITERS ROUNDS KERNEL...");
        std::process::exit(2);
    }
    let n = |i: usize| a[i].parse::<usize>().expect("number");
    let (blocks, iters, rounds) = (n(1), n(2), n(3));
    for k in &a[4..] {
        let ns = imij::lab::small_kernel_bench(k, blocks, iters, rounds).expect("kernel");
        println!("{k}\t{ns:.3}");
    }
}
