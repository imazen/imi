# YCoCg-R as a lossless residual decorrelation mode (2026-10-06)

Result: **not adopted.** A lab-only mode 4 that applies YCoCg-R lifting to the
prediction residuals (modulo 2^N, so no extra bit) saves 0.09% pooled. That is
too little for a new wire mode that every SIMD decode kernel would have to
implement.

## What was tested

The [2026-10-04 study](EVAL_YCOCGR_LOSSLESS_2026-10-04.md) transformed pixels
before coding (Co/Cg need N+1 bits). This experiment instead added YCoCg-R as
a fifth per-stripe choice beside the existing residual modes 1-3 (R-G, B-G
and so on), on the residuals of the G, R, B slots:

```
forward:  co = r - b;  t = b + (co >> 1);  cg = g - t;  y = t + (cg >> 1)   -> slots (y, co, cg)
inverse:  t = y - (cg >> 1);  g = cg + t;  b = t - (co >> 1);  r = b + co
```

with sign-extended halving and every step modulo 2^N. Unlike modes 1-3 it
rewrites slot 0, which the SIMD and inter-row decode kernels assume is
unchanged, so lab decoding routed mode-4 stripes to the scalar path.

Work in an exported tree (`main` after the IMIJ
rename); frozen under r5900xt `local-scratch/imij-ycocgr-20261006/` (source tarball,
binaries, hashes, 312-row measurement, gate logs).

## Exactness

Every encoded stream (312 rows, including all mode-4 emissions) decodes to
the exact source. With mode 4 disabled, the 945-cell encode parity harness is
byte-identical to an untouched export. `cargo test -p imij --lib` and strict
clippy pass with and without `lab`.

## Bytes

Same 52 sources as the 2026-10-04 study (36 SDR: photos, illustrations,
plots, screens; 4 HDR photos at 8/10/12/16-bit grids), efforts 7.5 and 10,
complete files, mode 4 admitted (mask `0b11111`) vs not (`0b01111`):

| Stratum | Without | With mode 4 | Change |
|---|---:|---:|---:|
| Photos (27 SDR + 16 HDR grids) | 75,079,667 | 75,056,840 | -0.030% |
| Non-photos (9 SDR) | 11,792,303 | 11,736,785 | -0.471% |
| All | 86,871,970 | 86,793,625 | -0.090% |

Mode 4 was picked for 219 of 1,510 stripes (14.5%). Best files: an HDR photo
at the 8-bit grid -2.02%, two illustrations -1.43% and -1.00%. Worst: a plot
+0.90%, a photo +0.32%, an HDR photo at 16 bits +0.29%. These regressions
happen only where mode 4 was selected: the per-stripe cost estimate
occasionally prefers it over a better mode.

## Why not adopt

0.09% pooled (0.47% on non-photos) does not pay for a fifth decorrelation mode
that rewrites slot 0 in the scalar, AVX2, AVX-512, NEON and wasm128 decoders
and in the inter-frame reconstruction. Revisit only if a better per-stripe
cost estimate turns the occasional regressions into wins and the non-photo
gain grows well beyond half a percent.
