# Benchmarks

Historical per-study raw benchmark tables and experimental study logs were trimmed when
creating the public `imazen/imi` repository. Summary results and methodology live under
[docs/EXPERIMENTS.md](../docs/EXPERIMENTS.md), [docs/METRIC_DECISIONS.md](../docs/METRIC_DECISIONS.md),
and [docs/BENCH_MATRIX.md](../docs/BENCH_MATRIX.md).

## Running benchmarks locally

- Decode/encode microbenchmarks and callgrind instruction counts:
  ```sh
  cargo bench -p imij
  ```
- Cross-codec benchmark suite (`imij-bench/` is a standalone workspace):
  ```sh
  cargo run --manifest-path imij-bench/Cargo.toml --release -- --help
  ```
- Browser / WebAssembly decode measurements: see [tools/imij-wasm/web/README.md](../tools/imij-wasm/web/README.md).
