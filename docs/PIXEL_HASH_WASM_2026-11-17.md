# Pixel-integrity hash pilot, wasm32

**Result:** retain XXH3-128 as the current default. In this 64 KiB hot-buffer
pilot it has similar observed cost to XXH3-64 and substantially lower cost than
standard rapidhash v3 in baseline wasm32/SIMD128. Experimental wide arithmetic
changes rapidhash's cost markedly. This does not establish browser performance
or select a new persistent algorithm.

Frozen zenhash source **85d32a33c5afe687674ff25b1045450ef635bd7a**; unchanged
`tools/portable-bench` / `tools/wasm-workload` / builder from that source.
Native host binary XXH3-128 **866ad09be83545d55d953d819c3b9899**. Build host i265;
timing host r7900x, homogeneous Zen 4. Rust 1.99.1, thin LTO, no target-cpu=native;
embedded Wasmtime **49.0.4**, Cranelift Speed, relaxed SIMD disabled. Four modules
are validated under their declared SIMD/wide-arithmetic capabilities.

All **30 cells / 6 comparison groups / 24 rounds each** complete, with top-level
unreliable/incomplete false. CPU placement is uniform_hardware, cpuset enforced.
No individual-core or frequency verification is claimed. The existing harness
checks native/WASM batch output across 19 methods, eight lengths, four seeds and
three chunk schedules before timing. Its folded batch check is not a substitute
for rerunning the independent full-output/128-bit stability corpus; that corpus
was not rerun for this pilot.

## Observed means

Throughput is GiB/s. Each timed batch hashes the same 64 KiB input 16 times;
the table divides the known 1 MiB of batch work by its measured mean time.
The batch **includes amortized allocation/input preparation**. This is not an
allocation-free hash kernel or a complete pixel-canonicalization benchmark.
JIT compilation and instance construction are outside the timed closure.

| Runtime | XXH3-64 | XXH3-128 | Rapidhash v3, 64-bit |
|---|---:|---:|---:|
| Native | 45.724 | 47.313 | 30.147 |
| wasm32 scalar | 11.150 | 11.307 | 3.647 |
| wasm32 SIMD128 | 24.071 | 24.205 | 3.844 |
| wasm32 wide arithmetic | 11.276 | 11.330 | 17.761 |
| wasm32 SIMD128 + wide | 24.279 | 24.347 | 23.487 |

Streaming, with 4 KiB update chunks:

| Runtime | XXH3-64 | XXH3-128 | Rapidhash v3, 64-bit |
|---|---:|---:|---:|
| Native | 38.023 | 41.758 | 21.274 |
| wasm32 scalar | 9.633 | 9.572 | 3.536 |
| wasm32 SIMD128 | 20.049 | 20.041 | 3.640 |
| wasm32 wide arithmetic | 9.703 | 9.682 | 16.204 |
| wasm32 SIMD128 + wide | 20.006 | 20.186 | 20.141 |

[All 30 rows](../benchmarks/pixel_hash_wasm_2026-11-17.tsv) preserve batch means,
per-64-KiB normalized time and throughput. Runtimes are interleaved within an
algorithm's groups; the three algorithms run in separate processes. There is
no A/A or multi-process confirmation. Do not infer a small cross-algorithm speed
difference or codec throughput from these exploratory means.

Here **wide arithmetic** means WebAssembly integer operations, including
`i64.mul_wide_u`: two unsigned 64-bit inputs produce low/high halves of the
full 128-bit product. The proposal also defines signed widening multiplication
and 128-bit addition/subtraction on pairs of i64 values; it does not add an
i128 Wasm value type. See the
[WebAssembly proposal](https://github.com/WebAssembly/wide-arithmetic/blob/main/proposals/wide-arithmetic/Overview.md).
Zenhash's rapidhash mixer uses exact Rust u128 multiplication and folds its
halves. An efficient widening multiply is relevant to that implementation;
changing the lowering does not change the hash function or digest width.

The frozen builder explicitly uses `-Ctarget-feature=+wide-arithmetic`, and
the host explicitly enables Wasmtime's `wasm_wide_arithmetic(true)`. The compiler
emits the retained experimental-feature warning. This label describes the
opt-in target feature in this build, not experimental hash arithmetic or a
different persistent algorithm. Baseline and wide outputs pass the existing
native/Wasm batch parity checks described above.

Follow-up inspection with wasm-tools 1.257.1 finds **594 static
`i64.mul_wide_u` instructions** in the retained wide module, versus **594 static
`call $__multi3` instructions** in the scalar module. These are whole-module
counts across the full workload, not operations per hash or a new speed test.
Both original modules remain covered by the unchanged evidence seal.
Reproduce inspection with `just pixel-hash-disassemble MODULE FRESH_OUTPUT`;
this reads the existing module and does not rebuild it or rerun timings.

Browser support/performance was not qualified. Actual preallocated
group/tile sizes, stride/canonicalization, Chrome/Firefox/WebKit and cold input
remain unmeasured. None of these rows is a reason to enable an unfrozen hash ID.

## Failures, resource scope and reproduction

The i265 placement attempts (plain, taskset, explicit CPU set, stock matching
zenbench exec) all reject before timing. They are retained, not accepted samples.
The pilot moves the identical binary/modules to the homogeneous host with all
gates intact. No unrestricted override or resource threshold change is used.

The original portable host lock required two archmage packages to resolve from
0.9.31-beta.2 to 0.9.31, as demanded by the frozen source manifest. Original and
resolved locks are retained. The first --locked and offline failures remain.
No zenhash source or production IMI dependency pin changed.

Run-heavy receipts: guest build **6 s / peak RSS 0.28 GiB**; native host build
**132 s / 1.49 GiB**; matching launcher **25 s / 0.45 GiB**. Timing processes:
XXH3-64 **6 s / 0.12 GiB**, XXH3-128 **5 s / 0.12 GiB**, rapidhash **5 s /
0.12 GiB**; minimum available 16,786–16,801 MiB. These are wrapper/process
observations, not hash-state memory estimates.

Export the frozen source, preserve the supplied resolved locks, then use
`just pixel-hash-portable-build EXPORT` on an eligible reserved host. Run
`just pixel-hash-portable-run EXPORT ALGORITHM FRESH_OUTPUT` for xxh3_64,
xxh3_128 and rapid_v3. Modules live at the compile-time source path; rebuild or
preserve that exact path when moving the native host executable. The
[sealed evidence pointer](../benchmarks/pixel_hash_wasm_2026-11-17.pointer.md)
records raw outputs, warnings/failures, locks and exact used binaries/modules.

The separate producer fix **81400bec** passes all three executable integration
tests and strict bin/test Clippy. Final tests **12 s / 0.41 GiB**, Clippy **0 s /
0.14 GiB**. Its first build/test **48 s / 1.28 GiB** also passed; the first strict
Clippy failed on two existing test lints. Parentheses and equivalent as_chunks
iteration fix those without changing fixtures or pixel expectations.

Both holders exited **09:58:56 UTC**, independently verified absent/all three
standard locks unheld on each host at **09:58:57 UTC**. Used binaries/modules
are preserved. Four inactive i265 Cargo targets totaling **1,991,024,537 bytes**
were removed. No maps, NAS writes, upload, deployment or release occurred.
