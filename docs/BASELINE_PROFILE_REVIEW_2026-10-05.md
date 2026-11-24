# Baseline conformance profile and codestream simplification review

> **User decision, 2026-10-06:** lossless must be part of the baseline
> profile in some form. Proposals here that make lossless an optional
> extension are superseded on that point; the format is now named IMI.
> See `docs/BACKLOG.md` (decisions of 2026-10-06).

2026-10-05, profile study. Source audit + read-only byte analysis;
no builds, encodes, or wire/default changes. The controller owns final profile and
design decisions; this document supplies evidence and seams.

Every claim names its source. "Format as built" statements were verified
against `zqi/src` at wave start, not assumed from SPEC.md; discrepancies
are §10.

## 1. Scope and terminology

Three "baseline" concepts must not be conflated:

* **Storage tier** (`Tier::Baseline`/`Extended`, `lib.rs:195-199`):
  samples ≤16 bits vs 32-bit. Enforced at parse (`v1.rs:1082-1085`,
  `lossy/mod.rs:2013-2015`) — including nested heads (§10.10) — and
  compiled out by the `extended` cargo feature (`Cargo.toml:15`).
* **Decode-speed tier** (`DecodeSpeed`, `container.rs:46-71`): advisory
  header byte recording the encoder's stated decode-cost target. Not a
  capability gate and not a profile marker — `decode_speed()` derives
  it from independently-settable knobs (`mod.rs:330-341`), so it cannot
  distinguish "tier-driven encode" from "hand-picked modes" and is never
  conformance evidence.
* **Conformance profile** (the subject of this document): a named subset
  of the wire that a class of decoders must implement and a class of
  encoders is restricted to emit. Does not exist yet.

The profile below is orthogonal to both existing tiers: it can require
the baseline *storage* tier while restricting much more than that.

## 2. The stream as built (verified against source)

```text
file   = SIGNATURE(8B) | varint(header_len) header | chunk*
chunk  = tag(4B) varint(len+1) payload            [container.rs:140-144]
       | tag(4B) 0 <self-delimiting payload>      (open form; critical only)

header (legacy)  = w h ch sample flags cicp orient coding speed [extensible tail]
header (compact) = 0 ver(1|2) mask w h [format? premult? cicp? orient? speed?]
                   [v2: significant_bits x channels]            [container.rs:358-479]

DATA payload, lossless = 0x01 | maps(nch × mode,bits,value16|32)
  | lanes(1..16) | hs:varint | gcols:varint | pal_flag | model bitstream
  | stripe records: varint len { decor | pal-mark 0x80..., nibble preds,
    per-slot varint len + 16-lane rANS stream }                 [v1.rs:1064-1182]

DATA payload, lossy =
  0x81: color sb gcols scale[3] qscale floor mul[3] matrix entropy (34B)
  0x82: color sb gcols | varint n, params(n∈6..38): flags qscale
        [scale3][floor][mul3][matrix][layouts3] entropy        [envelope.rs]
  then varint tl, tables bitstream (rANS models or HufTables),
  nested flags u8 (bit0 AQ, bit1 32-bit DC, bit2 keep-blocks),
  varint+lossless-head DC image, [varint+lossless-head alpha],
  records in stripe×group order:
    [selector map iff Adaptive4] [compact rANS tag 0|1|2 states] AC lanes
    interleaved with nested DC/alpha records on their own schedule
                                                            [mod.rs:2001-2189]

extension chunks (all ancillary):
  iCCP eXIf xMPp mDCv cLLI pRVW gMAP aNIM fRAM mCMP pDAT pIDX sLOC
                                                            [container.rs:108-133]
```

Layout byte (per color plane, 0x82 only): bits 0/1 half-sample x/y,
bits 2/3 transform side code (0,1,2 → log₂−1, so 2/4/8), bit 4 Adaptive4
(requires side 8), bits 5-7 rejected (`subblock.rs:36-45`,
`layout.rs:76-101`). Compact rANS tags: 0 raw, 1 implicit RANS_L +
exception bitmap, 2 common u32 + bitmap; ≤61B expansion, stack-restore
only (`compact.rs:1-10,115-150`).

## 3. Support matrix

M = mandatory decoder support; E = baseline-encoder restriction;
X = extension; S = service/resource limit, not wire conformance;
D = platform dispatch, outside conformance.

| Dimension | Wire form | Source anchor | Profile proposal |
|---|---|---|---|
| Signature, varints, chunk framing | 8B sig, LEB128 len+1 | `container.rs:23,140-144` | M: both closed and open DATA |
| Header | legacy + compact v1/v2 | `container.rs:177-479` | M: read both; E: write compact |
| Open (len=0) DATA | self-delimiting critical chunk | `container.rs:537-545` | M read; E may still emit |
| Chunk case semantics | critical/ancillary/private/safe-copy | `container.rs:135-138` | M: skip ancillary by length, fail unknown critical |
| Once-only chunks | DATA iCCP eXIf xMPp mDCv cLLI gMAP aNIM | `container.rs:630-636` | M enforce |
| iCCP/eXIf/xMPp | ancillary → retained verbatim in Metadata | `container.rs:637-641` | M display: may skip; M roundtrip: retain iff the impl claims metadata preservation |
| mDCv/cLLI | ancillary → parsed to fixed-size typed fields, malformed = Corrupt | `container.rs:642-649` | M display: may skip; typed parse required only if used |
| gMAP | ancillary; valid → typed gain map, malformed → retained opaque | `container.rs:281-351,650-656` | M display: skip or retain-opaque; interpreting = X |
| pRVW | ancillary; payloads collected raw into `File.previews` (NOT in `Metadata.chunks`, not validated at decode — measured: garbage payload still decodes `ok`); nested zqi parse only via `zqi::previews()` | `v1.rs:844-888`, `container.rs:557,650` | M display: skippable; decode = X |
| aNIM/fRAM | ancillary; generic decode parses nested frames | `frames.rs`, `container.rs:627-629,654-655` | M display: skippable (file still yields frame 0); frames = X |
| mCMP | ancillary → decoded at parse into inner metadata slot | `container.rs:598-622`, `metadata_pack.rs` | M display: skippable; decode+dedup required only for metadata fidelity (§3 class c) |
| pDAT/pIDX/sLOC | ancillary → **dropped, never retained** | `container.rs:130,622-624`, `planes/wire.rs` | M display: skippable; a generic decode→remux LOSES them; retention/indexed access = X |
| Coding selector | 0 lossless / 1 lossy | `container.rs:428-432` | M both; >1 = Unsupported |
| Decode-speed byte | advisory 0-3 | `container.rs:433-442` | M parse; legacy tolerates unknown (→Fastest), compact rejects. Never profile evidence (§1) |
| Lossless payload | version 0x01 | `v1.rs:1077-1079` | M; other versions → Corrupt (§10.1) |
| Channel maps | 8 modes, context-valid | `mapping.rs:11-29`, `v1.rs:1092-1104` | M: Lsb,Shift,Replicate,Scale,Const(alpha); X: Offset,F16InF32,UnormF32 (all three are extended-tier-only modes — see sample types) |
| Lanes | u8, 1..=16 | `v1.rs:1107-1110` | M all values; E: encoder default 16 |
| Stripe height | varint >0 | `v1.rs:1111-1114` | M any; S: physical-row amplification is a service-limit problem (§6) |
| Column groups | varint, < width | `v1.rs:1115-1119` | M (record schedule only; cheap) |
| Palettes | flag + 0x80 stripe form | `v1.rs:932,1124-1164` | M decode; E optional to emit |
| Predictors | nibbles 0-6; 7-9 inter only | `predict.rs:12-28` | M 0-6; 7-9 = frames extension |
| Decor modes | u8 0-3 | `v1.rs:1333` decor | M 0-3 |
| Sample types, outer image | U8,U16,F16 / U32,I32,F32 | `lib.rs:134-147`, `v1.rs:1082-1085` | M ≤16-bit; outer 32-bit = X |
| Sample types, nested DC | U16 or I32 (flags bit1) | `mod.rs:2095-2110`, encode `mod.rs:1539-1551` | **M both** — a ≤16-bit file can carry I32 DC (encoder emits it when any DC exceeds ±32768). I32 goes through the same `parse_head_checked` tier gate (`v1.rs:1082-1085`), so today's `extended` feature switches nested-DC support too: profile A ≠ "build without extended". Outer-32 = X is a *separate* extension |
| Significant bits | compact v2 field | `container.rs:203,443-452` | M decode; E: emit only when non-default |
| CICP | 4B | `container.rs:401-414` | M carry; transfer-aware paths where the format uses it (U16/U32 XYB, `mod.rs:2069-2076`) |
| Alpha | premult flag; nested lossless in lossy | `mod.rs:2133-2143` | M exact; nested image is the same lossless grammar |
| Lossy payload | 0x81 fixed / 0x82 envelope | `mod.rs:2007-2060` | M read both; E: write 0x82 |
| Lossy color | Xyb=0, YCoCg=1 | `color.rs:11-25`, `mod.rs:2016` | M both (Fastest tier emits YCoCg); other IDs → Corrupt |
| Quant params | scale/floor/mul/matrix ≤2 | `envelope.rs`, `mod.rs:2064-2066` | M all fields |
| Entropy modes | 0 RunLevel,1 Packed,2 Split,3 SplitHuf | `coef.rs:60-89` | M: RunLevel + SplitHuf; Packed/Split = X or second-tier decode (§5, §7) |
| Subblocks/sampling | layout byte, 0x82 only | `layout.rs`, `subblock.rs` | E: baseline files = all-full8; decode = X (reject cleanly) |
| Adaptive4 selector map | per-record map | `mod.rs` Adaptive4 path, `layout.rs:112-129` | X with subblocks |
| Compact rANS states | tags 0-2 | `compact.rs` | M (default encoder emits them; ~100 LOC, stack-only) |
| Nested DC image | lossless U16/I32, Rgb/Rgba(AQ/keep) | `mod.rs:2091-2132` | M |
| Nested alpha | lossless Gray, source type | `mod.rs:2133-2143` | M when header has alpha |
| Keep blocks (inter lossy) | nested flag bit2, RGB only | `mod.rs:2092-2099` | X (frames) |
| Frames/pages | aNIM + fRAM nested files | `frames.rs` | X |
| Plane sets / ROI | pDAT/pIDX/sLOC | `planes/` | X; plane records carry complete nested tiles |
| Threading | stripes/groups independent | — | D: scalar must equal SIMD bit-exact |
| Service limits | DecodePolicy/Session | `decode_resources.rs:14-91` | S: budgets are runtime config, never wire bytes |
| Plane-set limits | PlaneSetLimits | `planes/mod.rs:378-411` | S for the X path |

**Notes on ancillary-chunk obligations** (`apply_chunk`,
`container.rs:584-668`). The wire has four retention classes and they are
NOT interchangeable:

* (a) *Verbatim-retained*: iCCP/eXIf/xMPp and any unknown ancillary
  chunk land in `Metadata`/`meta.chunks` → survive decode→remux. gMAP
  also falls back to opaque retention when it doesn't parse
  (`container.rs:650-656`) — malformed gMAP is kept, not rejected.
* (b) *Parsed-typed*: mDCv/cLLI become typed fields (malformed =
  Corrupt); pRVW/aNIM/fRAM are decoded by the generic path — a
  display-only decoder MAY skip them by chunk length instead.
* (c) *Consumed-dropped*: mCMP is inflated at parse into the inner
  identity's slot (inner ∈ {iCCP,eXIf,xMPp}, record ≤2·16MiB+64,
  expanded ≤16MiB). Its dedup uses the *inner* identity in `seen`:
  raw-iCCP→mCMP(iCCP) and mCMP(iCCP)→raw-iCCP both fail `repeated chunk`
  (`container.rs:604-616,630-636`).
* (d) *Dropped*: pDAT/pIDX/sLOC return `Kind::Other` with nothing
  stored (`container.rs:130,622-624`). Skipping is fine for display,
  but generic decode→remux loses them — "lossless remux of plane-set
  files" belongs to the planes extension, not baseline.

A baseline display decoder therefore needs only header+DATA+chunk-frame
semantics; metadata roundtrip adds (a)+(c); (d) always requires the
planes extension. Whether skipped ancillary payloads must still be
*validated* is a profile decision — today the generic path validates
mDCv/cLLI/mCMP as it consumes them; this report recommends "validate
what you retain or expose", not "reject what you skip".

## 4. Where the bytes are (counted)

Census inputs: the frozen 2026-10-03 entropy-study corpus
(`benchmarks/encoder_lz_entropy_2026-10-03/results/coefs-json/`,
`INPUTS.sha256` verified against that study's freeze manifest),
reproduced byte-identically by
`experiments/baseline_profile_20261005/census.py`. Scope: 96 files with
*explicitly selected* entropy modes — NOT encoder defaults or
tier-picker output — 8 images (64-px to 12 MP), RGB8, no alpha. Shares
describe this corpus only.

* Container + signature + chunk framing + record varints:
  **27-31 B/file** — fixed floor, not worth wire work.
* Lossy head (0x81/0x82 params + global AC tables + nested heads):
  median 976-1988 B by entropy mode; **0.03-5.6% of file at 1 MP**, up
  to ~50% at 64 px. RunLevel carries the largest model set (NRAW=1158
  vs PK_NRAW=672 contexts).
* Nested DC: 3.0-34.3% of file over the 84 non-64-px rows (median
  8.6%; "1MP+" counts tag classes — the 921,600-px screens included).
* AC payload: the rest (62-97%).
* Per-AC-record overhead: 16 lanes × 4 B restart states + record varint;
  the compact-state form removes most of this where it wins
  (EVAL_OVERHEAD: 52-64% at q0/sb1, 0% ≥ q10).

AC bytes vs RunLevel on the same images (census): Packed +7.4% median,
Split +8.8%, SplitHuf +8.2% — with a −34.4% SplitHuf minimum on the
near-empty 64-px q30 streams. These are byte-size observations only;
they do NOT explain the encoder's mode choice. When `decode_speed` is
set, the picker (`mod.rs:1671-1691`) selects SplitHuf purely by
*predicted decode-time* ratio — per-mode linear models fitted to
1-thread Zen-4 times on 344 image/quality pairs, held-out median ratio
error 0.035-0.10 (`predicted_huf_ratio`/`huf_max_ratio`,
`mod.rs:367-390`; provenance LOSSY.md E17, zqi-cmp/fit_cost.py) — except
images ≤ 2^16 px (`SMALL_IMAGE`), which are coded both ways and take
SplitHuf when predicted-faster *or smaller*; only that branch compares
bytes. The −34.4% minimum is consistent with the small-image byte
comparison but says nothing about picker behavior at ≥1 MP. No AC mode
dominates on bytes.

## 5. Parser/envelope redundancies and ranked candidates

Ranked by serialized savings, decoder/scratch/seek cost, malformed-input
complexity, missing evidence, wire compatibility. Savings per file.

| # | Candidate | Bytes saved | Decoder cost | Malformed complexity | Missing evidence | Wire compat |
|---|---|---|---|---|---|---|
| 1 | Baseline encoders emit only RunLevel/SplitHuf | 0 | none | none | none | full. Scope: the picker emits only these two *when `decode_speed` is set* (`mod.rs:1671-1691`); `with_entropy` still emits Packed/Split otherwise — enforcement needs an explicit policy/checker seam (§7), not the tier byte |
| 2 | Baseline *decoders* may treat Packed/Split as extension | 0 | −2 token grammars, ~600-1000 LOC kernels/parity surface | needs a "known ID, unsupported" refusal — none exists today (§10.2) | per-mode peak-memory/thread data (E15 caveat) | IDs reserved; existing files decode under full profile |
| 3 | Compact header for all new encodes | ~7 typical | drops one writer grammar | none | none | readers keep legacy |
| 4 | 0x82 envelope for all new lossy encodes | ~10-25 vs 0x81 fixed head (34B → 6-38B varint, defaults implicit) | drops one head grammar | low (bounded 6-38B, strict flags) | none | readers keep 0x81 |
| 5 | Baseline maps ⊆ {Lsb,Shift,Replicate,Scale,Const} | 0 | none (context checks already exist) | none | none | extended maps remain for extended tier |
| 6 | Subblock/sampling layouts excluded from baseline files | 0 (already opt-in) | baseline decoder skips subblock.rs+selector maps entirely | reject via envelope flag-16 → clean refusal | RD/adoption data pending | layouts stay valid extension |
| 7 | Predictors 8/9 removal | ~0 | −2 recon branches | low | never measured separately from 7 (SIMPLIFICATION) | frames-only predictors; measure first |
| 8 | Header preset bundles / shared dictionary | median 19-56 B, ≤0.07% on photos; ≤20% on 8-64 px icons (EVAL_OVERHEAD) | +dictionary distribution/IDs | dictionary versioning is a new failure mode | icon corpus is 3-family crops, thin | needs new wire IDs — not recommended |
| 9 | Seeded-LZ AC/header dictionaries | ≤0.5% whole-file (overhead study; AC dominates) | +LZ dep in hot path | history/dictionary hostile-input surface (HOSTILE_INPUTS §"future LZ77") | no parse-cost data | new IDs — not recommended |
| 10 | Forbid open DATA in baseline encodes | 0 | none | none | none | writers that can't seek need open form — keep legal |

Not candidates: lane count 1..16 (encoder geometry, already restricted),
column groups (wide images + nested DC alignment need them), palettes
(graphics gap is zqi's weakness), compact state tags (encoder picks the
shortest form; decode is trivial), mCMP (ancillary remux option),
lossless/lossy dual coding (the profile's reason to exist).

## 6. Presets, shared tables, dictionaries — assessment

Measured (EVAL_OVERHEAD_2026-10-04, 5,250 eval files + 180 small cells):
exact-preset bundles save median **19 B** (0.019%), unseeded header LZ
30 B, seeded LZ 54 B; embedded dictionaries always lose. A shared
dictionary buys ~0.02-0.07% on photos — a tiny-image niche only, and
distributed, never embedded. **Recommendation: no preset/dictionary
wire IDs in baseline.** The 0x82 envelope + compact headers already
internalize the same defaults (6-38 B). A tiny-image profile could
revisit unseeded header LZ (≤20% at 8-64 px) with a real icon corpus —
an extension, not baseline.

Restart states: the overhead study's state-omission experiment showed
the underlying opportunity (repeated RANS_L states at low rate); the
shipped tag-1/tag-2 compact forms already capture it natively, without
a dictionary. No further state-side wire change is justified by current
data.

## 7. Profile boundary alternatives

**A — "Core still image" (recommended default).**
Single still image, lossless + lossy. Decoder MUST: both header
grammars, closed+open DATA, chunks/case semantics; ≤16-bit *outer*
samples; all four channel layouts; baseline maps; predictors 0-6; decor
0-3; palette form; lanes 1-16; any stripe/group geometry; 0x81+0x82
heads; XYB+YCoCg; all quant params/matrix; RunLevel + SplitHuf +
compact restart states; **nested DC images of type U16 *and* I32**
(nested heads cross the same tier gate — a baseline decoder still needs
32-bit nested-image decode even though the outer image never is);
nested alpha; full8 layouts only for baseline-produced files.
Decoder MAY: skip ancillary chunks for display (§3 retention notes);
treat Packed/Split as extension (needs an Unsupported-class refusal —
§10.2); skip mCMP unless claiming metadata fidelity.
Extensions, separately gated: outer 32-bit samples + their maps;
Packed/Split AC; subblock layouts incl. Adaptive4; frames; plane sets;
mCMP-as-metadata.
Encoder restricted to: compact header (v2 only when significant bits
used), 0x82 lossy head, RL/SH, full8, closed or open DATA, no
out-of-profile chunks. **Enforcement is an explicit policy seam, not a
wire byte**: `decode_speed` is advisory and cannot prove conformance
(§1), so baseline emission is either (i) a documented encoder-policy
surface — `LossyConfig` default / `with_decode_speed` produce
baseline-conformant parameters *only when the caller does not also set
layouts, extended types, AQ, frames or plane opts* — or (ii) a
conformance checker (§11's structural verifier on emitted files).
Cost: conformance text + checker + policy enum; zero wire change.

**B — "Lossless baseline".**
A minus all lossy: smallest decoder surface (no f32 transforms/DCT or
nested images), suits archival/embedded. Cost: drops the format's
mainline use. Can coexist with A as a sub-profile.

**C — "Full minus experiments".**
A plus 32-bit samples, Packed/Split decode, subblock layouts, frames —
everything except plane sets. Closest to "format as shipped"; doesn't
reduce kernel/test surface much.

**Default recommendation: A**, with named extensions E1…E6
(outer 32-bit samples + extended-tier maps, packed/split AC, subblock
layouts, frames, plane sets, mCMP metadata decode). Rationale: the *verified default configuration*
(`LossyConfig::default()` / `with_decode_speed` on ≤16-bit input
without further knobs) already emits only baseline-shaped files —
RL/SH, XYB/YCoCg, full8, compact streams — so adoption is cheap on
that path. It is NOT self-enforcing: the same API can select layouts,
Packed/Split, extended types, frames, planes, and the wire cannot tell
the difference; hence the checker seam. A retains the load-bearing
capabilities (exact alpha, CICP/HDR signaling, nested DC incl. its I32
form, wide-image groups); each exclusion has a wire-level accept/reject
seam (modulo §10.1), so extensions need no new IDs. B can ride the
same document as a strict subset marker.

Extension mechanism (existing, verified): unknown ancillary → skip/drop
(plane chunks *dropped*, §3 class d); unknown critical → Unsupported;
header version ∉{1,2}, coding >1, mCMP method>1/seed≠1, extended samples
w/o feature → all Unsupported. Payload-level unknowns (versions, entropy/
color/map/compact-tag/transform-map IDs, reserved bits) → Corrupt today
(§10.1): a new payload ID is indistinguishable from corruption until
that is reclassified (controller-owned).

## 8. Mode × kernel test-combination reduction

Today's parity surface: 4 AC modes × {scalar, AVX2, AVX-512
single/pair, NEON, WASM-simd128} × subblock layouts (Dct8/Dct4/Dct2/
Adaptive4 × 4 samplings + adaptive maps) × nested-image cadence.
Under profile A the AC-entropy axis shrinks 4→2 modes — a **2×**
reduction on that axis only — and the layout/sampling axis leaves
baseline conformance entirely (full8 only). These are independent axes;
do not multiply them into a single "reduction". And profile conformance
does not shrink the *full* implementation's test obligations: Packed/
Split/subblocks stay implemented and tested for the full profile — the
conformance matrix bounds what a *baseline decoder* must pass, and lets
a baseline-only implementation skip building the excluded kernels at
all. Platform parity stays a property of the implementation (scalar
oracle hash-equality per VALIDATION_PLAN), not of the profile — SIMD
kernels do not multiply conformance vectors. Lossless side is already
minimal on the wire: modes live in per-stripe bytes, not per-format
permutations.

## 9. Conformance test matrix (source anchors)

| Area | Accept cases | Reject cases | Anchor |
|---|---|---|---|
| Signature/header | legacy+compact v1/v2, all field combos | bad sig, header ver ∉{1,2}, reserved mask bits, trailing compact fields, w/h=0 or >u32::MAX | `container.rs:358-479`, tests `compact_rejects_*` |
| Varints | canonical + noncanonical-within-10B | 10th byte >1, >10 bytes | `lib.rs:469-482` |
| Chunks | closed+open DATA; unknown ancillary skipped (measured: retained in `Metadata.chunks`); dup once-chunks rejected; aNIM after DATA rejected — **other ancillary after closed DATA accepted+applied** (measured: post-DATA iCCP → `icc=true`); non-letter tag rejected; stray fRAM tolerated | open ancillary; unknown critical | `container.rs:530-667` |
| Maps | each mode × legal sample type; alpha Const | Const non-alpha; F16InF32/UnormF32 non-F32; Offset baseline; Replicate/Scale bits>16 on extended; bits>depth | `v1.rs:1087-1105` |
| Lossless head | lanes 1-16, hs>0, gcols<width, pal 0/1 | lanes 0/17, hs=0, gcols≥width, pal>1 | `v1.rs:1107-1128` |
| Stripe records | decor 0-3; PAL 0x80 + n-1 entries + nibble preds ≤6 (≤9 inter); per-slot lens | predictor >9 (or >6 intra); truncated streams | `v1.rs:724-815`, `predict.rs` |
| Lossy head | 0x81 fixed; 0x82 flags subsets; envelope 6-38 B | version ∉{0x81,0x82}; flag bits ≥6; param len ∉6..38; trailing param bytes; compact flag with Split/SplitHuf | `mod.rs:2007-2060`, `envelope.rs:73-122` |
| Lossy legality | kind×color combos per `mod.rs:2018-2031`; custom-precision/full-range constraints | I32 lossy; U32 without extended; XYB+U16 without known transfer | `mod.rs:2011-2031` |
| AC records | RL/Packed/Split/SplitHuf; compact tags 0-2; selector map iff Adaptive4 | entropy >3; compact tag >2; bitmap tail bits; odd payload; state <RANS_L | `coef.rs:80-89`, `compact.rs:115-150` |
| Nested images | DC Rgb/Rgba per flags; U16/I32; gcols/8 alignment; alpha Gray w/ source sample | nested head trailing bytes; gcols mismatch; DC hs ∤ sb | `mod.rs:2091-2143` |
| Frames | aNIM kind/loop/count; fRAM rect+duration+flags; keep bit2 ⇒ RGB only | frame rect/duration overflow; keep outside inter | `frames.rs`, `mod.rs:2092-2099` |
| Planes | PSET v1/v2; contiguous records; shared-component masks complete | overlapping/gapped records; dup names; lossy RAW; trailing index bytes | `planes/wire.rs:308-669` |
| mCMP | method 0/1, seed 1, inner ∈ {iCCP,eXIf,xMPp} | inner other; >2·16MiB+64 record; >16MiB expanded; noncanonical lens; inner-identity dup in either order | `container.rs:598-622`, `metadata_pack.rs` |
| Service limits | DecodePolicy budgets charge input/meta/chunks/units/work/working | each boundary denies with TooLarge | `decode_resources.rs:14-511` |

Vectors: handcrafted per VALIDATION_PLAN — every boundary (1/16 lanes,
255/256/257 palette entries, edge blocks, odd dims, q extremes, float
specials, channel×type combos) and every reject branch above, through
slice, reader, stream and session APIs at scalar + one SIMD tier.

## 10. Doc/source discrepancies and parser findings

1. **Payload-level version/ID checks return `Corrupt`; container-level
   ones `Unsupported`** — a class of wart, not one site:

   | Check | Error today |
   |---|---|
   | compact header ver ∉{1,2} (`container.rs:369`) | Unsupported |
   | coding byte >1 (`container.rs:431`) | Unsupported |
   | unknown critical chunk (`container.rs:542,660`) | Unsupported |
   | mCMP method >1 / seed ≠1 (`metadata_pack.rs:165-166`) | Unsupported |
   | 32-bit samples w/o `extended` (`v1.rs:1084`, `mod.rs:2014`) | Unsupported |
   | **lossless payload ver ≠0x01 (`v1.rs:1078`)** | **Corrupt("unsupported version")** — message says unsupported, class says corrupt |
   | **lossy payload ver ∉{0x81,0x82} (`mod.rs:2009`)** | **Corrupt** |
   | **plane index version (`planes/wire.rs:318,323`)** | **Corrupt** |
   | unknown lossy color (`mod.rs:2016`), entropy >3 (`mod.rs:2052`, `envelope.rs:115`), map mode >7 (`v1.rs:1088`), predictor >9 (`v1.rs:2116`), decor >3 (`v1.rs:2100`), compact-state tag >2 (`compact.rs:126`), transform-map kind (`layout.rs:181`), reserved bits (envelope flags ≥6 `envelope.rs:82`; layout bits 5-7 `subblock.rs:38`) | Corrupt |

   Every Corrupt row is a place a future extension could legitimately
   add a version/ID/bit allocation.

   Consequence: below the container layer "valid file from a newer
   profile/extension" is indistinguishable from "damaged file" — §7's
   extension seams are weaker than the list suggests. A nested lossless
   head carries its own version byte, so the gap applies inside lossy
   DC/alpha payloads too. Parser fix is controller-owned: reclassify these, or
   document that payload-version bumps are hard breaks.
2. **"Known but excluded" has no refusal class for reduced decoders.**
   A baseline implementation not shipping Packed/Split, layout flag-16,
   or extended-tier maps hits the generic `Corrupt` paths above or must
   hand-roll an `Unsupported` mapping; the profile should require an
   `Unsupported`-class refusal on every excluded-but-valid ID (model it
   on `v1.rs:1084`).
3. **Legacy header tolerates unknown decode_speed (→ Fastest);
   compact rejects it** (`container.rs:433-442`) — asymmetric but
   deliberate: legacy is the extension-tolerant envelope.
4. **Map-mode validity is context-dependent**: Const alpha-only;
   F16InF32/UnormF32 F32-only; Offset extended-only; Replicate/Scale cap
   bits ≤16 on extended (`v1.rs:1092-1104`). Profile text must state the
   cross product, not a flat list.
5. **`Metadata.orientation` 0 normalizes to 1 on write**
   (`container.rs:189-193`); parse requires 1-8.
6. **Lossy I32 is unreachable**: `InputKind::of` has no I32 arm
   (`mod.rs:394-413`); U32 is lossy-capable only under `extended`.
7. **Subblock coefficient buffers reserve 64 slots/parent/channel
   regardless of sampling** (`DECODER_MEMORY.md:121-124`, consistent
   with code); the 16 KiB/worker planner charge is a conservative
   allowance, not measured.
8. **`decode_speed` is advisory** (`mod.rs:330-341`); decoders may
   ignore it (HOSTILE_INPUTS: not evidence of resource bounds).
9. **Plane chunks are dropped, not retained** (`container.rs:130,
    622-624`): generic decode→remux is *not* byte-lossless for plane-set
    files; `Metadata.chunks` never carries them. Remux fidelity must
    exclude plane sets or route retention through the planes extension.
    Parse-failure behavior also differs per tag: mDCv/cLLI → Corrupt,
    gMAP → demoted to opaque retention, mCMP inner errors → Corrupt —
    a spec must say which ancillary payloads are validated vs skippable.
10. **`extended` gates nested DC too, so it cannot be the profile
    switch.** `parse_head_checked` applies the tier check to nested
    heads (`v1.rs:1082-1085` via `mod.rs:2106-2121`), so building
    without `extended` removes *mandatory baseline* behavior: I32 nested
    DC (emitted whenever a DC coefficient exceeds ±32768,
    `mod.rs:1539-1551`). A profile needs finer seams — mandatory
    nested-DC I32 vs optional outer 32-bit samples/maps — which the
    feature flag and `Tier` enum conflate today. **Measured:**
    `enc-f16i32dc.zqi` (F16 outer = baseline-tier, I32 nested DC) decodes
    on the default build but is refused
    `unsupported:32-bit samples (extended tier)` on a
    `--no-default-features --features std` build — a baseline-legal outer
    file refused by a baseline-tier build. Reachability note: U16/U32
    inputs normalize to ≤255 pre-transform so their DCs never overflow
    i16 at any quality; only unbounded linear-light input (F16/F32) can
    produce I32 DC, and **F16 is the only baseline-tier outer type that
    can reach it** — this row is not theoretical.
11. **SPEC.md vs built**: header test vectors match; prose describes
    the same grammars, but the "once" chunk list and the mCMP record
    bound (2×16MiB+64) are implementation-only details a spec must copy.

## 11. Independent minimal verifier

Two tiers, matching VALIDATION_PLAN's "independent scalar decoder"
requirement without duplicating it:

* **Structural verifier (~600-900 LOC, no pixels):** signature →
  header → chunk walk → DATA head (lossless maps or lossy
  envelope+tables bounds) → record schedule from geometry (stripes =
  ⌈h/8/sb⌉×groups; DC/alpha on their cadence; selector-map bytes =
  ⌈blocks×3/8⌉ iff Adaptive4; compact-state grammar) → record payloads
  vs declared/open-bounded length. **Update (same day, measured):**
  `experiments/stream_optimization_20261005/stream_walk.py` (~550 LOC)
  already implements this — all four AC modes, alpha, column groups,
  both header forms, `0x82` compact-state grammar, fRAM recursion,
  byte-exact on all corpus + fixture files. The remaining work is
  porting it to a conformance-suite shape, not writing it.
* **Pixel vectors:** frozen corpus `.zqi` files (hashes in
  `benchmarks/encoder_lz_entropy_2026-10-03/`) plus handcrafted minimal
  files per §9 row; scalar-only decode vs SIMD pixel hashes. This is
  the conformance gate; the structural tier is a cheap embedder
  pre-check.

## 12. What this study did not do

No source edits, no wire/default changes — census reuses the frozen
2026-10-03 corpus (explicit-mode files, not encoder defaults; §4). No
new RD or speed claims. Packed/Split removal from the *wire* (vs
baseline emit/decode) remains the controller's call per SIMPLIFICATION.md
(reserve IDs, never renumber).

*Update (same day):* the study's later profile wave DID encode
fixtures — `enc-*.zqi` + `vec-*.zqi` in
`experiments/baseline_profile_20261005/FIXTURES_MANIFEST.tsv` —
covering i32-DC/alpha/AQ/skip-map/groups/layouts/lossless/open-DATA/
mCMP/gMAP cases the frozen corpus lacks, each decoder-verified on both
public APIs (`decode_reader` + `decode_into_threads`). The §4 census
claims still describe only the frozen corpus, as written.
