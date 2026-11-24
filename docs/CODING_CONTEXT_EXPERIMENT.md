# Coding-context wire experiment

This page preserves the original private-container experiment. The subsequently
authorized [native local ANS implementation](LOCAL_ANS_CONTEXTS.md) uses local AC
models with shared DC, and allocates feature 27/head record 39 in PR91. Its
performance must be measured independently; the C-arm numbers below do not
describe the native extension.

User-authorized 2026-11-10. Compare complete-file size at identical pixels,
decode cost and random-access dependencies. No transport, muxing, capture,
metrics/maps, new public APIs or identifier allocations are part of this study.

## Measured decision

The original 40 cells are complete. Keep image-wide contexts as the default.
Pursue optional group-local coding contexts for ANS with 64-row groups; do not
make them mandatory or introduce packet framing or another quantization field.
This experiment allocates no native record ids and changes no shipping defaults.

First improve the compatible encoder: B reduces complete size in 40/40 cells,
by 3.82–41.19% against A, without new syntax. C improves on B in 37/40 cells.
Every tested ANS context case is smaller; the three size losses are Huffman:
HDR Q20/64 rows +4.84%, RGB8 Q5/64 rows +1.71%, RGB8 Q20/64 rows +0.18%.
A context extension therefore needs an explicit global/default path.

The table below combines ten settings per row: two retained images and five
qualities. Byte ranges are per-cell C/B; summed bytes weight by encoded size.
Decode ratios are geometric means of complete-frame time; smaller is faster.

| Rows | Coder | C/B byte range | C/B summed bytes | Time 1t | 2t | 4t | 8t |
|---:|---|---|---:|---:|---:|---:|---:|
| 8 | ANS | 0.7632–0.9700 | 0.9320 | 1.5099 | 1.3825 | 1.2618 | 1.1298 |
| 8 | Huffman | 0.9349–0.9884 | 0.9721 | 1.3659 | 1.2692 | 1.1807 | 1.0956 |
| 64 | ANS | 0.8265–0.9715 | 0.9560 | 1.0898 | 1.0358 | 0.9633 | 0.9082 |
| 64 | Huffman | 0.9781–1.0484 | 0.9971 | 1.0593 | 1.0258 | 0.9602 | 0.8908 |

Eight-row contexts impose substantial decoding cost. Sixty-four-row contexts
trade slower one-worker decoding for better measured parallel decoding.
Huffman's aggregate compression benefit is only 0.29% at 64 rows and includes
larger files; these results do not justify a compression default for it.

### Which coding state matters

For HDR Q40, ANS, 64 rows, the byte accounting is:

| Arm | DC payload | AC payload | Numeric heads/record framing | Outer declaration/index/framing | Complete |
|---|---:|---:|---:|---:|---:|
| A | 241487 | 119256 | 656 | 192 | 361591 |
| B | 119441 | 119256 | 1007 | 192 | 239896 |
| C | 121051 | 84144 | 7175 | 799 | 213169 |

B recovers 122046 DC payload bytes using the current wire. C's additional
26727-byte saving is driven by local AC models: DC payload actually grows by
1610 bytes against B despite all 34 local DC contexts fitting U16. This does
not support a new DC-width switch by itself. A useful context proposal should
express local coding maps/models, retain explicit unit/context association in
the index, and share unchanged image declarations. The compact inheritance
syntax itself has not been measured; C counts full repeated numeric heads.

### Sparse reads are an independence check

Withheld-payload controls pass: a selected C group decodes without any other
group's numeric payload. This preserves independence rather than establishing
a new random-access capability over native IMI.

For the HDR Q40/64-row ANS center pixel, B requests 139427 bytes in 48 reads;
C requests 7517 bytes in 211 reads. **These are reader behaviors, not wire
minimums.** The native reader's `Window::ensure` uses 64 KiB read-ahead windows;
C uses exact ranges and scans outer context headers. Their buffering policies
are different. Do not attribute this byte reduction to the wire or use it to
justify a format change. No read-ahead redesign is part of this experiment.

### Evidence and limits

[All 40 cells](../benchmarks/coding_context_2026-11-10.md) include complete sizes,
absolute MP/s at 1/2/4/8 workers, A/A controls and actual requested read bytes.
[Reproduction and retained artifacts](../benchmarks/coding_context_2026-11-10.pointer.md)
identify the measured source, binary, manifests and interrupted-run selection.
All coefficients and pixels agree exactly in every cell and worker budget.
Each accepted timing group has 24 pre-round-gated paired rounds. Same-binary
A/A mean-time ratios range from 0.9592 to 1.0340 across the study; these are
exploratory observations, not a layout-randomized no-regression certificate.

Missing for format adoption: broader images/geometries, adopted-feature
combinations, other architectures, encoder throughput and a compact native
context syntax. Two UHD controls do not establish a general rate-distortion
or speed result. No quality metric, diagnostic map or new storage mirror was
used. The scoped experiment ends with this recommendation; it does not start
another implementation or qualification campaign.

## Frozen arms

- A: unchanged one-pass encoding, signed32 DC, universal DC tables, AC tables
  trained on the first admitted band with positive later-symbol support.
- B: the same representation and existing wire, with DC tables learned only
  from the retained first band. Observations have weight 64; every legal token
  has smoothing weight one. Tokens beyond RUN_CAP remain impossible. No global
  statistics pass is substituted for this compatible arm.
- C: one native image declaration plus indexed local numeric contexts. Each
  context uses the existing numeric coder trained on only the admitted group.
  Its statistics and coding passes reread that group, never future source rows.
  Local DC representation/maps/models and AC models can differ; quantized
  coefficients and reconstructed pixels must match A/B exactly. The
  ordinary pixel encoder supplies its complete local DC maps/models and AC
  models. It sees only the admitted group, never the full source raster.

Code is private to lab test binaries under
`imij/src/lossy/bounded/coding_context_experiment`. The measurement entry needs
an explicit `IMI_CODING_CONTEXT_MANIFEST`; without it the test executes real
small odd-size/flat-first-band regressions rather than skipping for absent
scientific files. Ordinary defaults and public APIs remain unchanged.

## Candidate wire

C uses private IMICTX01 magic, not an ordinary IMI signature. It contains one
native image declaration, group row count, ordinal/length/features plus the
existing numeric DATA payload per group, an explicit offset/length directory
and a fixed locator/footer. There are no nested complete image files or child
indexes. The decoder rejects any numeric context whose check-tail reservation still
requires a removed child trailer. Full numeric heads and repeated quantization parameters count toward
size, as do outer framing and the directory. This is a conservative full-context
layout, not a minimal override design. Encoded integrity is off for all arms;
artifact identities use XXH3-128.

## Acceptance

- Equal entropy-decoded quantized coefficients and complete decoded samples.
- Exact native output at 1/2/4/8 decoding workers.
- Legal unseen later symbols remain encodable in B after a flat first band.
- Context index/ordinal/truncation failures remain failures.
- Selected-group pixels remain exact with every other numeric payload withheld;
  no previous-group model history is permitted.
- Report complete size and DC/AC/numeric-head/outer overhead separately.
- Measure parsing and fresh output allocation with A/B/C plus identical A/A,
  24 clean paired rounds at 1/2/4/8 workers on the reserved measurement host.
  Gate: at most 5% aggregate foreign CPU load, at most four processes above
  10% of one core, at least 4096 MiB available RAM and CPU at most 85 C.
  The initial zero-process gate rejected UI/agent activity and kernel
  compaction despite observed aggregate foreign load below 3.58%; those
  diagnostic timings remain rejected. Every accepted round must meet the
  declared gate. This is an exploratory cost comparison, not release proof.

Sources are the retained UHD HDR control and UHD RGB8 DCT control. Both entropy
coders, low/high quantization and two group heights are declared in the frozen
manifest. Exact pixels replace quality rescoring; no maps or decoded-image
sweep is requested. Measured identities and results are linked above.

## Interpretation

C beating B supports this design only for the measured settings; it does not
prove that no better compatible encoder exists. C losing includes the cost of
full repeated numeric heads and does not rule out a compact override layout.
B tests one declared first-band learning rule, not every compatible policy.
