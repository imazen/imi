# Value-table profiling

The standalone manifest keeps Coz 0.1.3 out of the codec dependencies.
Run its justfile commands under the usual resource wrapper, with a host lock
around profiling. `build TARGET` and `check TARGET` build and check the tool;
`profile BINARY IMAGE RAW ITERATIONS OUTPUT` records causal experiments.
Every progress point includes clearing the output, decoding and checking exact
pixels. Confirm predictions with the paired timing runner; virtual speedups
are not measured decoder speedups. Source-line attribution can be coarse.

`matrix DIRECTORY OUTPUT HOST_LOCK WORK_LOCK ARM=BINARY ...` invokes
[value_table_profile.py](../value_table_profile.py). Build every source arm
and linker layout in a separate fresh Cargo target. A shared target can reuse
an artifact from a different copied source tree. `decoder-build TARGET FLAGS`
builds the timing harness; `decoder-layout TARGET SEED` uses matching lld
section-shuffle seeds. Use lld without shuffling for the mainref arm.
`sizes TARGET DIRECTORY SIDE` regenerates the synthetic fixtures.
`callgrind TARGET HOME` runs the instruction suite under AVX2; wrap it with
the host lock and set TMPDIR before invoking it. The runner records hashes,
checks load before each round, locks each process, verifies every warmup and
timed decode, and refuses to overwrite output. Its `--synthetic`, `--off-only`
and `--rounds 24` options select the other study grids.

`summarize MATRIX` expects three complete rounds with base/base2/adopted arms.
It takes the worst process median per file and thread count, sums times within
each set, and retains all per-cell time ratios and A/A exceptions in JSON.
`summarize-layouts MATRIX` expects 24 rounds with mainref plus matching
main/adopted seeds 11, 29 and 47. It reports median and worst-round aggregates,
within-cell rate spreads, consistent regressions and the A/A flagged counts.
Both summaries require the runner's DONE marker and refuse to replace JSON.
`summarize-off MATRIX` reads an `--off-only` study with any arms plus `base`
and `base2`: aggregate time per set and thread count, per-cell geomeans and
cells slower than their per-cell A/A difference. `summarize-arm MATRIX ARM`
prints one arm's table/off ratios per set, its worst single file and the
regular-map MPx/s, the figures in docs/VALUE_TABLES.md.
`instruction-gate MAIN_LOG CHANGE_LOG` compares the 21 cg_decode JSON records
against the evaluation standard, printing every result and failing on any miss.

Synthetic filenames describe the requested source set, not necessarily the
encoded index width. Inspect the generator's map declarations before labelling
a size result: at side 1024, k49152 declares 15-bit table indices. Exhaustive
16-bit writer correctness does not establish 16-bit-domain performance.
Vector-bank source code also does not establish spill-free execution; inspect
the executable's assembly before claiming hardware register residency.
