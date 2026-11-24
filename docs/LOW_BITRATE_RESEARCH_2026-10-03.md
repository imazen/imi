# Low-bitrate research shortlist, 2026-10-03

Read-only literature review over indexed image-compression papers and their
primary PDFs/HTML (no builds, benchmarks, corpus downloads, or code changes).
Converted text can occasionally garble equations and columns, so accessible
primary PDFs and HTML versions were checked directly where useful.

The ranking is a proposed experiment order, not measured ZQI gains. Encoder
work should precede more wire modes. Current fixed coefficient quantization,
`floor(abs(c)/step + 0.4)`, leaves an optimization avenue that changes neither
the decoder nor the transform. Existing effort promises identical decoded
pixels at fixed quality/color/AQ; coefficient RDO, masking, or AQ policy changes
cannot silently become part of that effort contract.

## Ranked candidates

### 1. Bounded coefficient RDO, including explicit tail/whole-AC-zero decisions

**Primary basis:** Grajek and Stankowski, *Rate-Distortion Optimized Quantization
in Motion JPEG* (2022), section 3. It scans nonzero AC backward and evaluates
unchanged, +1, -1, and zero candidates with block distortion and coded rate;
DC is excluded to preserve block parallelism. The evaluation used video frames
and PSNR, with reported BD comparisons at JPEG q70–85: evidence that compliant
encoder optimization works, not proof of ZQI low-quality perceptual gains.
[Author-hosted paper](https://www.multimedia.edu.pl/publications/Rate-Distortion-Optimized-Quantization-in-Motion-JPEG.pdf).

**ZQI adaptation (inference):** first add a small candidate search around the
scalar result using `D + lambda*R`, where R prices actual run-level/EOB tokens,
magnitude/sign bits, and the following affected context. Explicitly compare
dropping the final nonzero, contiguous tail, or every AC in a block. That
captures discontinuities in EOB/run costs that independent rounding misses.
Freeze provisional tables, optimize, then rebuild tables and verify actual
file bytes; entropy estimates alone can miss table and state overhead. Use
canonical coefficient scaling; weighted transform-domain squared error is a
cheap proxy, not exact perceptual error after nonlinear XYB-to-RGB conversion.
Leave DC fixed initially and check finalists through the normative inverse.

**Wire/compute/memory:** existing syntax; no decoder change. A bounded search
adds encoder work and a fixed per-block scratch array. SIMD initial transforms
and quantization stay intact; search can run independently over lane/block
groups. A full trellis is optional later, with state for the coding context/run
rather than VVC's dependent-quantization state. Do not transplant VVC's
quantizer lattice: that would change reconstruction syntax.

**Pruning support:** Wang et al., *Low Complexity Trellis-Coded Quantization in
Versatile Video Coding* (2020 preprint; TIP 2021), sections III–IV: RD-based
last-nonzero departure and candidate pruning. Its VVC speed figures do not
predict ZQI speed. [Preprint](https://arxiv.org/abs/2008.11420).

### 2. Perceptual coefficient zeroing with a small reconstructed neighborhood

**Primary basis:** Alakuijala et al., *Guetzli: Perceptually Guided JPEG Encoder*
(2017), sections 2.1 and 2.2.2: zeroing is the main local optimization; nearby
blocks need coordination because edge distortions and nearby artifact energy
interact. Its optimizer is extremely slow and targets visually lossless JPEG.
[Paper](https://arxiv.org/abs/1703.04421).

**ZQI adaptation (inference):** use candidate zero decisions from item 1 but
rank/check them in reconstructed XYB/RGB neighborhoods, including both sides
of block edges. A bounded 2x2-block or row-halo pass could protect gradients,
thin edges and text while removing masked high-frequency coefficients. Limit
the number of checked coefficients and iterations. Start with zeroing before
an unrestricted ±1 search: Guetzli's own attempts to combine magnitude
shrinking and zeroing were unsuccessful. Its aggregate compression figures
are not a low-quality ZQI estimate.

**Wire/compute/memory:** existing syntax; decode unchanged. Encoder-only
reconstruction and perceptual scoring can be expensive; avoid a full-image
Butteraugli optimizer as the default. A row-neighborhood implementation needs
source/reconstruction halos and deterministic ownership of boundary updates.
Scalar selection can coexist with existing SIMD transforms. This is a
quality-altering policy, separate from entropy-only effort.

### 3. Quantization-error-aware AQ rather than mean absolute AC activity

**Primary basis:** Watson, *DCT Quantization Matrices Visually Optimized for
Individual Images* (1993), uses frequency sensitivity, luminance adaptation,
contrast masking and pooled quantization errors. Rosenholtz and Watson,
*Perceptual adaptive JPEG coding* (1996), chooses per-block multipliers to
flatten modeled perceptual error, explicitly examining signaling cost.
[1993 paper](https://www.math.ucdavis.edu/~saito/data/quantization/watson-spie93-dctq.pdf),
[1996 paper, indexed NASA URL](https://human-factors.arc.nasa.gov/publications/RosenholtzWatson1996.pdf).
The latter URL is search-indexed as the paper but a live open redirects to
NASA's division page (no local text available at review time).
Its method is a lead, with the 1993 full text as directly read evidence.

**ZQI adaptation (inference):** choose among a small set of already representable
AC multipliers by reconstructed/weighted coefficient error per *actual
quantizer*, with finer quantization where a smooth area has visible banding
or an edge is fragile. Include the nested AQ-channel byte cost and penalize
gratuitous multiplier changes. Unlike activity-only scaling, coefficient
magnitude itself is not treated as distortion. Coefficient-specific weights
can guide item 1 without an AQ channel; the current AQ channel supplies only
one block multiplier shared across AC channels.

**Wire/compute/memory:** existing optional AQ syntax; DC is currently unscaled
(`aq_dequant` changes factors 1..63). Decoder AQ work already exists, but
enabling AQ is not free. Encoder can reuse coefficients and per-row candidate
scratch; no image-sized map is necessary beyond the existing coded map. AQ
already lost on P15 for one mean-|AC| rule (E6b); this proposes a different
decision mechanism, with no promise it wins at low quality. Historical JND
models are viewing-condition-dependent, especially beyond near-lossless.

### 4. Optimize actual bytes for tables and stripe geometry before new entropy modes

**Primary basis:** Duda, *Encoding of probability distributions for Asymmetric
Numeral Systems* (2021), discusses the trade between probability precision,
header cost and coding penalty. Recoil (Lin et al., 2023) documents the
parallelism/overhead cost of independent rANS partitions; its GPU results
are not ZQI performance evidence.
[Duda paper](https://arxiv.org/abs/2106.06438),
[Recoil paper](https://arxiv.org/abs/2306.12141).

**ZQI adaptation (inference):** existing clustering optimizes cross entropy
with effort-dependent caps/passes (`coef.rs::cluster`); try candidate table
counts and merges and keep the minimum
of serialized tables/context map + actual AC bytes. This belongs in existing
effort because pixels remain identical. Tables are global, not repeated per
AC stripe. Each AC stripe/group still pays its final lane states.

The audit found photo1415 at q30 has 8,192 state bytes out of 8,862 AC
bytes (128 stripes, 16 states, 4 bytes): for this case, better probability
tables cannot fix most of the cost. Existing larger stripe heights are the
first same-format candidate, subject to coefficient scratch, concurrency and
streaming latency. The [subsequent stripe probe](../benchmarks/low_quality_stripes_2026-10-03.md)
measures that tradeoff directly. rANS normalization precision is fixed by the
current format; changing it is not an encoder-only table-search knob.

**New-wire possibility, only if needed:** omit explicitly empty AC units or
reduce active lanes on tiny/near-empty units. Audit current parser/kernel
contracts first; this is not encoder-only. Keep one bounded representation
instead of adding another general coder. Recoil-style checkpoint metadata is
a research comparison, not a recommendation to replace independent stripes:
it introduces synchronization and pixel/context-state problems beyond rANS.
Recent LZ experiments already show structured-content headroom but generally
lose on photos; they do not justify replacing run-level rANS.

### 5. A compact quantizer-aware reconstruction filter, with cross-stripe halos

**Primary basis:** Sneyers et al., *The JPEG XL Image Coding System: History,
Features, Coding Tools, Design Rationale, and Future* (2025), section 7.2,
describes paired encoder sharpening/decoder 3x3 Gabor-like smoothing and a
more costly edge-preserving filter. Filtering crosses group boundaries.
[Full primary HTML](https://arxiv.org/html/2506.05987v2#S7.SS2).
Midtskogen and Valin, *The AV1 Constrained Directional Enhancement Filter
(CDEF)* (2017 preprint; ICASSP 2018), sections 2–3, provides direction finding
from reconstructed 8x8 pixels and constrained nonlinear taps designed for
vectorization. [Paper](https://arxiv.org/abs/1602.05975).

**ZQI adaptation (inference):** compare one simple 3x3 paired pre/post filter
against one-pass direction-aware/quantizer-aware deringing. Filter strength
can derive from known step/AQ values, with at most a global enable/strength
option initially; avoid a large per-block parameter surface. Evaluate smooth
gradients, block edges, text and ringing separately. Encoder must optimize
against filtered reconstruction, not unfiltered output plus a late blur.

**Wire/compute/memory:** normative filtering changes decoded pixels and needs
a new signaled mode/version; an optional display filter can test quality but
cannot establish codec RD claims under a different decoding contract. SIMD
arithmetic is feasible in safe Rust/magetypes across native/wasm. A 3x3 filter
needs a one-row halo; CDEF-like taps need a wider halo and direction-search
work. Retain independent entropy stripes but exchange/delay reconstructed
boundary rows: treating each 8-row stripe edge as an image edge would recreate
the seam problem seen in removed 4:2:0. Avoid a whole-image output pass.
Start with this compact alternative before actual lapped transforms, which
also introduce cross-block filtering and partition-boundary complexity.

### 6. Coarse/DC-based LF prediction with quantizer-aware smoothness gating

**Primary basis:** *Committee Draft of JPEG XL Image Coding System* (2019),
arXiv:1908.03565, Annex H, predicts low-frequency AC from decoded DC via an
upsampled coarse image, gated smoothing and transform-domain reconstruction.
This is a historical draft, not evidence the tool survives in current JXL.
The 2025 overview section 6.2 instead describes adaptive LF smoothing for
coarsely quantized gradients.
[2019 draft](https://arxiv.org/pdf/1908.03565),
[2025 LF discussion](https://arxiv.org/html/2506.05987v2#S6.SS2).

**ZQI adaptation (inference):** extend W7 only with a concrete new mechanism:
gate a ramp/coarse reconstruction by *DC quantization uncertainty*, rather
than only slope agreement, and evaluate prediction/reconstruction with RDO.
Predict from decoded DC, not source DC, to avoid mismatch. Quantized-domain
residual prediction can preserve existing output exactly if adding the
prediction back recovers every original integer coefficient. A decoder-side
coarse-image interpolant plus residual is a different quality-changing mode.

**Wire/compute/memory:** new syntax and decoder arithmetic for either route;
AC lanes can remain parallel because coarse DC is already separately decoded.
Neighboring DC rows must be available before AC reconstruction, including at
8-row stripe boundaries; check the DC streaming dependency before claiming
unchanged memory. A few coarse rows cost O(width/8), but lookahead can alter
latency and worker scheduling. Preserve deterministic scalar/SIMD arithmetic.

W7 already measured a limited same-pixels entropy win: gated mode about
0.4–0.6% overall and 0.6–1.5% on photos at q30–95; ungated prediction harms
graphics. There is no integrated decoder/timing result. Those gains alone
do not close the low-quality AVIF gap. Directional pixel intra prediction
does not suit independent single-block-row stripes (W8).

## Literature references and evidence limits

| Source | Identifier |
|---|---|
| RDOQ in Motion JPEG, 2022 | DOI 10.24132/csrn.3201.29 |
| Low Complexity TCQ, 2020/2021 | DOI 10.1109/TIP.2021.3051460 |
| Guetzli, 2017 | arXiv:1703.04421 |
| Watson DCT matrices, 1993 | DOI 10.1117/12.152694 |
| JPEG XL overview, 2025 | arXiv:2506.05987 |
| JXL committee draft, 2019 | arXiv:1908.03565 |
| CDEF, 2017/2018 (first preprint 2016) | arXiv:1602.05975 |
| Probability distributions for ANS, 2021 | arXiv:2106.06438 |
| Recoil, 2023 | DOI 10.1145/3605573.3605588 |

No paper provides measured ZQI improvements. RDOQ's PSNR and Guetzli's
near-lossless Butteraugli regimes do not establish low-quality results.
Confirm with actual full-file RD curves against imazen AVIF encoders, several
perceptual metrics, output inspection, stripe-boundary maps, core+alloc/wasm
correctness, and bounded scratch. Byte-only tests may run on dev; timing
claims require an idle approved measurement host.

If encoder-only work leaves a structural gap, a two-choice 8x8/16x8 transform
(16 pixels wide, 8 high) within one block row is a bounded partition experiment suggested by the
JXL overview's transform tools. It still needs syntax, quantizer design and
SIMD changes; adjacent blocks currently occupy different lanes, so pairing
would require revisiting lane mapping and the coarse DC representation.
It ranks below the six candidates because no isolated
paper result predicts its value in ZQI. E1 rejects one lifting DCT, not every
larger float transform, but it does not justify adding many transform modes.
