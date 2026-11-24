# Evaluation: bounded reversible intra coefficient tools — W7-gated-local-v1

2026-10-04 · study `eval-reversible-20261004-workspace` · parent `90dd06d5` ·
change `lwnxmonw` · study brief `local-scratch/zqi-evaluation-20261004/reversible.txt`
(+ `common.txt`)

Status: **evaluated prototype preserved as source patches**, outside compiled
production source. See [reproduction and adoption gates](../experiments/w7/README.md)
and the [source manifest](../experiments/w7/manifest.json). All source paths and
line references below refer to that patched baseline. The tests establish
coefficient/pixel parity on the covered cases and same-binary default/disabled
byte identity. Native performance neutrality remains unmeasured.

## Variant

`W7-gated-local-v1` — **not** the published m13/m15 numbers and not the
older runtime-f32 lab prototype. Pinned f64 constants, shared verbatim by
encoder and decoder (`zqi/src/lossy/lf.rs`):

    A01 = -0.14235657174840682   (AC01/AC10, slope term)
    B02 =  0.034850664021682236  (AC02/AC20, curvature term)
    G11 =  0.020265393519959308  (AC11, diagonal term)

- DC inputs clamped to ±2^22 (identical clamp both ends).
- Integer slope-agreement gate: predict only where the two one-sided DC
  slopes satisfy `2|s1−s2| ≤ max(|s1|,|s2|)` (exact integer form of the
  50% gate).
- Adjustment emitted only when `0 < |pq| ≤ 2^20` after f64
  round-half-away.
- Encoder emits a plain `0x81` stream if any residual would exceed
  ±2^21 (`RES_MAX`); the fallback is byte-identical to baseline.
- Historical encoder-only savings **do not transfer**: different
  constants, rounding, gates, and signaling.

## Wire format (lab-gated)

`DATA` payload tag `0x82` (`LOSSY_LF`) + mode byte:

| `lf_pred` | wire mode | predicts |
|-----------|-----------|----------|
| 13        | 1         | AC01, AC10 |
| 15        | 2         | + AC02, AC20, AC11 |

- Non-lab builds: `parse_lossy_head` rejects any tag ≠ `0x81` with
  `Corrupt("not a lossy stream")` at the tag byte, before reading the
  experimental mode byte or allocating variant state
  (`zqi/src/lossy/mod.rs:1635-1651`; the `0x82`→lossy container gate is
  `zqi/src/v1.rs:944-946`, `2689`). Verified on the full-decode path by
  `experimental_tag_rejected_nonlab`; `LossyStream::new` reaches the same
  parser directly on the payload slice. **Scope limit:** the reader
  entry `read_lossy` (`mod.rs:1814-1831`) buffers a fixed header prefix plus variable-length
  tables and nested payloads into a head Vec before calling the same parser, so it rejects
  before records/variant structures but *not* before that prefix buffer —
  an earlier reader-side tag check is recorded as a follow-up below, not
  claimed.
- Lab builds: `0x81` unchanged; `0x82` + mode ∈ {1,2}; other modes →
  `Corrupt("unknown experimental lossy mode")`.
- AQ + LF and `decoder_memory_limit` + LF → `InvalidInput` at config
  check (`mod.rs:1027-1035`).
- `lf_pred == 0` → byte-identical `0x81` stream to the same binary's
  default encode (`lf_signaling_and_rejections`). This is a same-binary
  comparison — not a cross-revision check against a pre-change
  executable; that identity is unmeasured and listed as an adoption gate.

## Inverse hook and task boundaries

One shared arithmetic source (`lf.rs`) — `apply` (encode, subtract) and
`restore` (decode, add) call the same `for_each` gate/predictor, so the
decisions match bit-exactly.

- Hook: `finish_coefs` (`mod.rs:2723`) — after entropy decode + DC fill,
  before reconstruction; every decode path (threaded `decode_lossy_impl`,
  paired `decode_stripe_coefs2`, `LossyStream`, `read_lossy`, reader)
  shares it.
- `eobs` is raised whenever a restored AC materializes in a coded-empty
  channel — required: the SIMD flat-fill path skips `eobs == 0` blocks
  and the scratch invariant needs them read+cleared. RunLevel uses the
  zigzag index; packed modes use `z.div_ceil(4)` group counts.
- `DcWin { all, row0, nby, bw }` carries exactly the DC rows a decode
  task can see: the current DC stripe × the current column group.
  Prediction never crosses DC-stripe or group boundaries (encoder
  `apply` uses the same `dc_rows`/`gb` bounds).
- `LossyStream` retains the current DC stripe's decoded rows (bounded
  halo — never more than one DC stripe) **only** for `0x82` streams;
  `0x81` keeps the old per-stripe clear (`base = by0`).
- No per-coefficient atomics anywhere except `#[cfg(test)]` counters
  (`APPLIED/EOB0/BLOCKED/KEPT`) — compiled out of bench/release/native
  builds. Counter-asserting tests serialize on `lf::TEST_LOCK`.

## Memory accounting

- Decoder: the only new live data is the retained DC rows in
  `LossyStream` for `0x82` streams — from the current DC stripe's top
  through one row past the stripe being decoded, at most one DC stripe's
  rows live at once (batch paths already hold the whole DC stripe).
  `0x81` streams keep the pre-LF per-stripe reset. Because the planner
  does not model the retained rows, `decoder_memory_limit` + `lf_pred`
  is **rejected** outright — production planning stays accurate.
- Encoder: `lf::encode` reads the DC image bytes the encoder already
  builds (`dc` Vec before `encode_parts`) — no extra image-sized DC
  grid. Zero additional encoder-side buffer beyond the stripe coefs it
  rewrites in place.

## Evidence (2026-10-04, this worktree)

All logs under `local-scratch/zqi-evaluation-20261004/reversible/`:

| Evidence | Result | Log |
|---|---|---|
| `cargo test -p zqi --features lab` | **75/75 lib + all integration, 0 fail** | `cargo-test-lab.log` |
| `lf::tests` (6) unit: exact apply→restore, eob-0 raise (direct, per-block), slope-gate edge, kept-block skip, residual-overflow decline, corrupt-coef bounded | all pass | same |
| `lf_tests` (8) e2e: tag/mode parse, invalid cfg, AQ+LF and memlimit rejects, `lf_pred=0` byte-identical to same-binary default, pixel parity full/simd/scalar/threads/stream/reader across 8 geometries × 4 entropies × m13/m15, exact quantized coef+eob equality incl. mid-DC-stripe rows, inter kept blocks, alpha exact, 400-case corrupt no-panic, DC-split×threads, eob0 counter on real content | all pass | same |
| Non-lab decode of `0x82` | `Corrupt("not a lossy stream")` | `nonlab-reject.log` |
| `zqi-bench` release build (`lab` → `zqi/lab`) | ok | `zqi-bench-build.log` |
| Corpus probe `lfpred` on mixed lowq set (5 files × photo/plot/screen × q30..95 × modes 0/13/15) | **75/75 rows, rc 0**: m0 baseline printed per (image,q); all 50 LF rows `0x82`+mode, full decode OK, decoded pixels byte-identical to baseline; **0 `plain` fallbacks**, 0 skips, 0 errors (harness exits nonzero on any load/encode/decode/parity failure and prints explicit `skip`/`load_error` rows) | `lfpred-probe.tsv` |

`dbytes` is the exact complete-stream size delta vs the same-binary m0
stream (pixel-identical, so no quality normalization is needed). Raw
byte-weighted aggregates independently checked by the controller:

| Mode | All | Photos | Screenshot | Plot |
|---|---:|---:|---:|---:|
| m13 (two coefficients) | −0.2011% | −0.4395% | −0.1196% | +0.0441% |
| m15 (five coefficients) | −0.1768% | −0.4396% | −0.0557% | +0.0823% |

Negative means fewer complete-file bytes. These are totals over five qualities
per source: three photos, one screenshot and one plot. m13 individual deltas
range from −0.9690% to +0.1627%. This small exposed sample does not establish a
broad corpus gain. [All 75 rows](../experiments/w7/sizes.tsv) include baseline
sizes; quality metrics would be identical because reconstruction is exact.

Five source patches are checked in under `experiments/w7/patches/`
(`jj diff --git` vs parent `90dd06d5`; each <30 KB), including tests and the
bench harness. The controller applied them to a fresh baseline archive and verified all
five reconstructed source hashes, then verified the five input hashes.
Hashes: `input-sha256.txt` (corpus PNGs), `source-sha256.txt`,
`patch-sha256.txt`, `binary-sha256.txt`, `output-sha256.txt`
(probe TSV). `lfpred-probe-v1.log` keeps the superseded first probe run.

### Bug found and fixed (logged per request)

`LossyStream::new` moved the nested DC `Parsed` out via
`p.dc.take().unwrap()`, but `next_row` still computed the DC-stripe
height via `p.dc_k()` — which returns 1 once `p.dc` is `None`. Effective
`hs` collapsed from `k·sb` to `sb`, so every lossy stripe became its own
"DC stripe" and the rolling halo was never filled: the down-neighbour at
each stripe's last row was missing and the stream path under-predicted.
Caught by `lf_pixel_parity_all_paths` (75×70 sb3, diff at pixel row 16 =
stripe-0 last row) and `lf_alpha_exact` (diff at row 8, block (1,1)).
Fixed reading the real height from `self.dc.p.hs` (`mod.rs:2063-2065`).
Regression coverage: sb=3/k=2 in the parity matrix, alpha, and
`lf_dc_split_threads`.

## Explicitly unmeasured / deferred (adoption blockers)

- **Normal-decode performance neutrality: UNMEASURED.** The `DcWin`
  plumbing and `LossyStream` field touch shared decode paths. `0x81`
  streams take the pre-LF buffer behaviour by construction (`halo=0`,
  `base=by0`), but no native timing evidence exists. Gate any adoption
  on a native decode comparison.
- **Cross-revision default-byte identity: UNMEASURED.** `lf_pred = 0`
  byte identity is proven only against the same patched binary's default
  encode, not against a pre-change baseline executable. Same adoption
  gate as native neutrality.
- **Native speed of the experiment: UNMEASURED.** All evidence above is
  correctness/parity; counter instrumentation is test-only and nothing
  was timed. No speed claim is made.
- **Reader-path early rejection: follow-up.** `read_lossy` buffers the
  header and variable-length data before `parse_lossy_head` rejects `0x82`
  (non-lab) or a bad mode (lab). Add a tag check at reader entry before
  the prefix read if early rejection is required there too; not claimed
  in this report.
- **Broad compression benefit: UNMEASURED.** The exact complete-file
  comparison above covers only five exposed sources and five qualities.
- **Secondary reversible cross-channel/lifting transform audit:
  DEFERRED** (per priority instruction — bounded W7 first). Blockers to
  unblock it: (a) needs its own bit-exact inverse hook (likely also
  `finish_coefs`) and a second wire mode or flag; (b) channel-pair
  ordering constraints vs per-channel `eobs` bookkeeping must be
  designed (the eob-raise contract solved here is a prerequisite, not a
  solution); (c) same task-boundary, kept-block and corrupt-input
  evidence matrix would be required.
- Known scope limits: LF+AQ undefined (rejected); kept-block semantics
  tested; U16/F16/F32 paths not in this probe's corpus (RGB/U8 only —
  unit parity covers the coefficient layer only).

## Reproduce

Apply the saved patches to the pinned baseline first. Run heavy commands under
the shared heavy-job lock and resource wrapper described in the coordination
report. Then, in the reconstructed worktree:

    cargo test -p zqi --features lab
    cargo test -p zqi experimental_tag_rejected_nonlab
    cd zqi-bench && cargo build --release --bin zqi-bench
    ./target/release/zqi-bench lfpred \
        local-scratch/zqi-lowq-2026-10-03/corpus.tsv   # TSV on stdout
    # ZQI_LFMODES overrides the default mode list "0,13,15"
