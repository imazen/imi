# Scoped defaults, seed bank IDs and single-pass feature boundaries

The proposed shipping grammar is now [V4](../SPEC_V4.md), with
[explicit PR89/90 scope and migration](../SPEC_V4.md#9-pr89-and-pr90-scope).
This document records existing V3 code and measured compatibility; the V4
proposal does not turn unimplemented writer paths into supported paths.

Owner direction, 2026-11-11: bank IDs use varints; compatibility can change before
release. Identify single-pass compatibility/performance and the experiments that
header-selected defaults cannot replace. This is a bounded code/audit change,
not a restart of the paused transport/display campaign.

## Implemented bank selection

New seeded lossy files use parameter-presence bit 9 and feature 28
`seed_bank_ids` (Lossy 0.6). After the ordinary optional parameters, the field is:

```
bank_id      uvarint    1..=u64::MAX, immutable bank identity
scope        uvarint    bit 0 AC, bit 1 DC, bit 2 alpha; nonzero
```

Both use the existing canonical unsigned LEB128 reader. A large ID is a key,
not an allocation count or array index. Bank 0 is the encoder's "disable seeds"
setting and is invalid in a present selection. Unknown banks/scopes are
Unsupported; zero selection, overflow, truncation, nonminimal encoding and
simultaneous legacy/new fields are Corrupt. Scope interpretation is independent
of the ID. There is one general capability, not one feature bit per future bank.
Unknown readers refuse the required capability before pixels.

The old bit-0 packed bank/scope field and bank-1 capability remain readable.
New encoders emit the varint form, so seeded bytes/profile requirements change;
this does not redefine the old field. Bank 1's frozen table asset is unchanged.

Logical IDs and lab inspection/configuration/registration are u64 end to end.
The old lab high-bit policy overload is removed: `seed_exact` separately controls
whether explicit candidates are coded for the final size comparison. The lab
registry retains its previous maximum of 14 registered banks, independently of
ID magnitude. A version-2 lab asset envelope carries a varint logical ID and
length-delimited unchanged version-1 table asset. Legacy asset registration keeps
its old ID/header checks. Published bank meanings must never depend on decoder
software version; newer implementations may optimize/cache expansion, not change
the probability tables behind an existing ID.

The bank-ID increment above did not implement scoped parameter sets. The
subsequent [PR92 implementation](archive/WIRE_DEFAULTS.md) adds Lossy 0.7 defaults,
typed DC/AC definitions/references and consecutive DC coverage. It remains
separate from the bank migration and from the paused transport campaign.

The compatibility table below records the bank-migration baseline. Current
PR92 additions: bounded/one-pass writers emit inline current-unit defaults and
causal ANS/root model selections; whole-image writers also emit nonuniform DC
coverage. The bounded writer explicitly refuses nonuniform DC output.
[Current writer matrix and qualification](archive/WIRE_DEFAULTS.md#encoder-policy-and-compatibility).

## Current lossy single-pass compatibility

"Single-pass" below means `BoundedLossySession::one_pass` or its budgeted form,
not the ordinary encoder or the multipass bounded encoder. Checked source:
`lossy/bounded.rs::prepare_plan`, `bounded/one_pass.rs::configure/prepare`,
`bounded/one_pass/rate.rs`, `lossy/local_ans.rs::validate` and the existing native
component/common-adapter qualification receipts.

| Feature | Current one-pass status | What defaults/model references change |
|---|---|---|
| Explicit ANS and split-Huffman AC | Supported; conservative positive-support models prepared from the retained first band. | Model declaration/reference cost; does not remove training or entropy work. |
| Native local ANS contexts | Opt-in supported for explicit ANS; automatic tiers and Huffman with this option are refused. First-band DC learning accompanies it. | Could generalize selection to both coders and reusable definitions; the current public option remains ANS-only. |
| Seeded tables | The ordinary encoder supports AC/DC/alpha seeds. One-pass preparation passes no bank to its AC learner and uses explicit nested heads. Seed-backed one-pass selection is not implemented by this varint migration. | Built-in defaults can avoid transmitting/training models, but need a causal, valid choice and an explicit fallback for uncovered symbols. |
| XYB / YCoCg, Gray/RGB with optional exact alpha | Native paths exist. Constructed native component evidence covers 8/10/12/16-bit 444/422/420; that is a separate component route. | Interpretation still must be admitted before pixels. Defaults do not implement color conversion or sampling. |
| Precision / shared adapter | The native plan admits U8/U16/U32/F16/F32 subject to its color/range/feature checks. Dedicated one-pass evidence is narrower. Common live adapter is restricted to full-storage sRGB U8/U16. | A general declaration cannot turn admitted-but-unqualified cases into qualified paths. |
| Fixed default 8×8 transforms | Supported. | A transform default could remove repeated selection for other supported shapes. |
| 16×16 / 16×8 / 8×16 merges, small/adaptive transforms and nondefault channel layouts | Deliberately excluded by the current one-pass profile; the ordinary/multipass codec already has these grammars. | Signaling gets cheaper; transform kernels, group preparation, edge handling and the one-pass writer still need integration. |
| Image-computed AQ / caller AQ fields | Rejected by bounded preparation. The budgeted path separately tries an existing uniform AQ code per staged band. | Default/exception coding can reduce maps; it does not compute AQ or integrate its encoder path. |
| Uniform AC scale | Selected compatible helper is integrated; five scale settings have constructed native-depth/alpha/DC/publication checks. Dense quality/perf adoption remains open. | Existing quantizer fields already suffice. V4 group deltas subsume PR90's proposed signaling; its tuning implementation remains separate. |
| Trellis / higher efforts / lab RDOQ | One-pass requires effort 0 and rejects the refinement/lab coefficient switches. | Defaults do not perform the search or reduce its intrinsic compute cost. |
| Exact-block overlays | Excluded by current one-pass configuration; existing ordinary grammar remains available. | A preset cannot add the overlay preparation and publication path. |
| Stripe height and column groups | Existing geometry choices; work is within the admitted current source band. | No new scheduler field. Minimum source footprint and parallelism remain real constraints. |
| Larger DC cadence | Existing file grammar supports it, but current one-pass forces one DC group per AC source band. | A different default cannot make future source rows available earlier. |
| Checksums / trailing index / provisional rows | Existing finalization and early group publication; Complete remains separate from provisional output. | No default changes validation scope or removes index/checksum work. |
| Unit ordinals / explicit color-transform option | Explicitly refused by the current one-pass profile. | Existing syntax is not the blocker; missing writer qualification is. |

The private catalog experiments train on the complete quantized image. They
prove representation/access and rate comparisons, **not causal one-pass catalog
learning**. Reusing an immutable definition does not reuse ANS states or require
another group's pixel/AC payload. The index must identify the definition bytes.

## Current lossless single-pass compatibility

Lossless already has a public one-source-pass, bounded-group encoder:
`ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(columns, rows,
max_working_bytes))`. Native `GroupEncodeSession` accepts each strided rectangle
once and requires its output to drain before another rectangle. It preserves
all eleven native sample types, exact alpha, significant-bit padding and raw
float bits. This differs from the ordinary adaptive whole-image encoder.
The native session is no_std + alloc; the common `EncodeRequest::group_encoder`
adapter uses the pixel descriptor/metadata types it can represent. The row
adapter has its own buffering/output contract. See
[existing group APIs](STREAMING_AND_SETS.md#bounded-group-writer-api) and
[common drainage](ZENCODEC.md#caller-drained-group-output).

Source at `0906ec85`, rechecked 2026-11-12:
`stream_encode.rs::GroupEncodeConfig::from_config`,
`GroupEncodeSession::{new_rows,encode_group,analyze}` and
`codec/groups.rs::EncodeRequest::group_encoder`.

| Property | Current lossless group path |
|---|---|
| Source passes / model preparation | One source traversal. Identity maps and fixed positive symbol weights are known before the first group; no future-image analysis is needed. Local residual/token buffers are still required. |
| Exactness | Integer and float storage bits are preserved, including NaN payloads and exact alpha. No lossy quality setting or 8×8 transform restriction applies. |
| Prediction / compression | Default identity decorrelation and MED prediction, no global palette/range/value-table discovery, no image-trained entropy probabilities. The session reuses prebuilt encode tables. |
| Parallelism | Public group encoding is serial (`analyze` uses one thread). Independent groups retain existing parallel decode opportunities. A thread setting does not turn this session into a parallel producer. |
| Input ordering | Rectangle suppliers feed groups directly. Forward raster input uses one-row groups or explicit full-width band scratch for taller groups; source/sink/scratch storage is separate from codec admission. |
| Index / checksums | Basic sans-I/O/common group executors accumulate neither. Existing indexed/checksummed Write adapters use explicit caller scratch. These adapters already have wire grammar. |
| Seeds / generalized defaults | Public group heads explicitly write their fixed models with no seed selection. Lab standalone lossless bank registration is separate. The bank-ID migration does not add group seed selection or local model switching. |

The compression penalty can be large. The retained
[three-photo comparison](../benchmarks/group_forward_real_2026-10-13.md),
source `d093b870`, used 1024-square RGB8 crops and 256-column groups:

| Crop | One-row group bytes | Sixteen-row group bytes | Earlier adaptive whole-image bytes |
|---|---:|---:|---:|
| 1000 | 3,201,120 | 2,435,931 | 728,022 |
| 1400 | 3,549,613 | 2,671,207 | 788,945 |
| 1600 | 2,972,465 | 1,929,589 | 584,011 |

All group outputs passed exact readback. These are historical observations,
not current-head size/throughput measurements. Their single timing observations
used different source-I/O patterns and are not a qualified speed comparison.
The older synthetic gigapixel RSS observations have a separate provenance limit;
all-sample correctness does not extend those measurements to other sample types.

Lossless evaluation holds quality exact and compares bytes/bpp against actual
encode/decode throughput and bounded storage. General defaults/banks could
reduce model declaration cost and supply useful predefined probabilities;
causal selection still needs full symbol support or a current-group fallback.
A change of numeric maps or entropy models must be explicitly interpretable by
an independently accessed group. Larger groups and local reversible predictor
choices already have syntax; scratch reuse, fewer allocations and parallel
encoding are implementation work. New lossless local-model/map interpretation
is a separate grammar decision, not supplied by the lossy AC-context option.
No lossless performance campaign is started by this audit.

## Retained performance, with its actual scope

The [four native local-context comparisons](../benchmarks/native_local_ans_2026-11-10.md)
use frozen codec `b810d94a`, binary XXH3-128
`23c3f6f61031e1c8d2821d8261b567ec`, two retained UHD sources and scale 40/80.
They do not time the bank field or the later Lossy 0.7 default/reference framing.

| Cell | Framewise encode MP/s, 1/2/4/8 | Learned-global one-pass | Local-ANS one-pass |
|---|---|---|---|
| HDR16 q40 | 102.32/161.85/260.42/344.60 | 131.24/195.25/225.64/248.55 | 110.20/146.63/174.65/177.41 |
| HDR16 q80 | 96.63/127.32/223.26/324.87 | 119.28/169.06/178.91/192.90 | 94.05/117.99/124.13/130.23 |
| RGB8 q40 | 106.24/186.64/306.47/446.27 | 115.99/170.26/202.46/204.66 | 94.40/125.96/137.81/142.16 |
| RGB8 q80 | 80.52/135.79/242.19/340.51 | 100.80/136.08/159.16/167.33 | 75.66/88.88/98.78/103.99 |

Local contexts save 2.64–11.65% against learned-global one-pass at identical
coefficients/pixels, but remain 1.78–2.70% larger than framewise and are slower
than learned-global in every measured worker cell. At eight workers their
103.99–177.41 MP/s is below 248.832 MP/s, the raw pixel rate of 3840×2160 at
30 Hz. That arithmetic is not a sustained deadline or application claim.
A/A time ratios span 0.9717–1.0287. Decode results and all resource receipts
remain in the linked report/pointer; e.g. final build peak RSS 3.36 GiB,
full correctness wrapper 0.13 GiB, timed-process wrappers 0.37–0.53 GiB.
No new throughput result is inferred from the bank-ID change.

The later [47-cell full-image attribution](../benchmarks/native_model_attribution_2026-11-11.md)
measures ordinary native model placement, not one-pass throughput. Its mixed
candidate saves 1.18% summed bytes with 2.04× eight-worker geometric mean encode
time. The [group/split pilot](../benchmarks/video_latency_2026-11-08/group_split.md)
contains resource-contaminated/exploratory timing and incomplete strict coverage;
it cannot supply a missing qualified frontier. ARM timings remain exploratory
by owner instruction. Do not combine these cohorts into a single speed verdict.

## Experiments still needed, beyond generalized signaling

These identify remaining decisions; this audit does not start these campaigns.

1. **Causal model coverage and cost.** Compare built-in models, first-band
   models, current-unit inline models and reuse of already-defined models on
   changing content with late rare/wide symbols. A static seed with zero
   probability for a later token is invalid for that unit. Require full legal
   support or a complete current-unit fallback; never use future statistics.
   Measure training/table construction, entropy passes and whole native
   1/2/4/8 encode/decode, including actual definitions/references and memory.
2. **DC representation.** Isolate fixed conservative DC widths/maps from
   first-band probabilities and group-local native mapping/predictor choices.
   Probability defaults do not recover a better sample representation. Any
   per-group numeric head must define its geometry, interpretation and indexed
   dependency explicitly. Existing off-line DC trials do not settle this.
3. **Transform and source-footprint integration.** Use the existing kernel
   study rather than repeating unchanged tile timings. Remaining work is native
   one-pass integration and rate/quality: 16×16/16×8/8×16 versus 8×8, odd edges,
   small/exact/AQ interactions, strided inputs and native depth. A 16-row
   transform needs its source footprint even if its selector costs zero bits.
4. **Geometry and scheduling.** Measure band/column/DC cadence choices with
   bounded retained input, actual native throughput and sparse-read
   amplification. Nonuniform DC spans need a native index/dependency reader;
   defaults alone do not establish correct coverage or eliminate buffering.
5. **Quantization/refinement.** AQ, trellis and per-unit AC control need their
   own one-pass implementation and matched-quality evaluation. Generalized
   defaults reduce signaling, not analysis/search work or unknown-future rate
   allocation. Changes to reconstructed pixels require the owning dense metric
   and generation-loss gates; pure entropy representation changes do not.
6. **Missing paths and qualification.** Add supported writer paths deliberately
   for merges/small/exact/ordinals/precision, then require actual selected
   positive witnesses, withheld-input publication, exact route parity and
   resource admission. Presets cannot replace these implementations. Qualified
   ARM and release-platform/no-regression gates remain distinct from the
   owner's retained exploratory observations.

## Flag-field audit

Header feature/presence masks, lossy/lossless parameter masks, frame controls,
scientific calibration/georeferencing and palette entry controls already use
varints. The new bank ID and seed scope both use varints. For extensible control
masks below 128 that costs one byte, exactly as a fixed byte would. Bit 7
and higher require additional bytes; the representation is not free for every
mask. The control fields use the existing u64 limit, not an unbounded bit set.

Remaining byte-shaped controls are not all extensible masks:

| Field | Current representation / consequence |
|---|---|
| Legacy seed byte | Read compatibility only; packed ID/scope superseded by the new field. |
| `cORg` optional-field flags | Now varint; known bits 0/1 retain identical bytes. Unknown meanings are refused. |
| `gMAP` alternate-color flags | Now varint across container/validation/upgrade. Known values retain bytes; unsupported fields remain opaque in ancillary metadata parsing. |
| Native component quantizer-slot mask | Now varint; current four-slot values retain identical bytes and unknown slot bits remain invalid. |
| `pSUM` reserved field | Now read as a varint constrained to zero. Valid bytes and the bounded prefix requirement remain unchanged. No new option is defined. |
| Sequence-index reference presence | Closed 0/1 tag in a fixed 96-byte record, followed by an algorithm ID and reserved bytes. A variable-width extension also needs a record-layout decision; a blanket substitution would break indexed offsets. |
| Transform/split maps, pixel bit-plane masks, entropy tables | Packed coded data with defined alphabets/widths. Replacing these with per-symbol varints can increase rate; default/exception coding is the relevant experiment. |

A varint supplies an extensible integer representation, not permission to ignore
unknown semantics. Unknown interpretation-changing flags still require refusal;
independently length-delimited ignorable fields can be skipped as specified.

## Qualification gate for this increment

The gate is finite: native library tests plus the seed-bank, matrix,
color-origin, bitmap-checksum, validation/upgrade, profiles/presence,
near-lossless, subblock/superblock, scientific metadata, decoder-memory,
common-encode and native-component regressions. Wide-ID positive witnesses
must actually select the requested bank with both AC coders, count complete
file overhead and preserve strided pixels/padding at 1/2/4/8 workers. Malformed
new selections and the original packed-field rejection controls both remain.

Strict default/portable Clippy, decoder-only no-default compilation, the seed
utility consumer and SPEC/oracle registry conformance complete the gate.
This migration does not claim a current-head speed result, a generalized
parameter-set implementation or release-platform qualification.

[Final native qualification and reproducible identities](../benchmarks/bank_ids_2026-11-11.md)
record the passing gate and its platform/performance limits.
