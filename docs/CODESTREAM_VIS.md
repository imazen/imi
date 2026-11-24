# Codestream visualization (Codestream Atlas)

Goal from the user (2026-10-06): a precise, visual specification of the
codestream. It starts as an interactive page, mirrored to an artifact until a
GitHub Pages site exists, with animations and a 3D view of the codestream and
the coding process. The same structure should later cover every zen codec.

## Accuracy comes from the decoder, not from drawings

The visualization is generated from what the production decoder reads.
`imij::lab::codestream_trace(data)` runs the production V2/V3 streaming reader
or the production V4 complete-input row driver with a lab-only recorder switched
on. The readers report:

- spans (file, header, chunks, heads, stripes, records) over absolute byte ranges;
- named fields with their decoded values and exact byte ranges;
- the image region each record codes;
- all of it in the order the decoder consumes the bytes, which is what the
  decode-order playback animates.

Outside `lab` builds every hook is an inline no-op, and the field macro
`tf!(name, reader, expr)` expands to `expr` alone, so production code is
unchanged. In lab builds a disabled recorder costs one thread-local check
per hook.

Each trace carries self-checks: every child lies inside its parent, the leaf
spans tile the file without overlap, and any unattributed byte ranges are
listed rather than hidden. The `codestream_trace` example exits non-zero on
any problem. Every viewer fixture (lossless; lossy RunLevel/SplitHuf;
column groups; adaptive and 4x4 layouts; RGBA with alpha records; template
palette and bilevel stripes) attributes every byte with no problems.

## Trace schema `zen-codestream-trace/0` (codec-neutral)

```json
{ "schema": "zen-codestream-trace/0", "codec": "imi",
  "spec": { "base": "SPEC.md", "V4": "SPEC_V4.md", "LOSSY": "docs/LOSSY.md" },
  "glossary": { "slot": "One coded channel (SPEC 2.2). ...", "lane": "..." },
  "file": { "bytes": 8865, "base64": "..." },
  "image": { "width": 192, "height": 128, "channels": "Rgb", "sample": "U8" },
  "decode": "ok",
  "nodes": [ { "id": 0, "parent": null, "kind": "file", "spec": "1",
               "label": "IMI file", "start": 0, "end": 8865,
               "value": "...", "region": [x, y, w, h] } ],
  "checks": { "problems": [], "unattributed": [[a, b]] } }
```

Nodes are listed in read order. `kind` is drawn from a small shared
vocabulary (file, header, chunk, head, records, record, payload, stripe,
stream, field); codecs may add kinds. `spec` names the specification section
that defines the node: a SPEC.md section number, or `LOSSY` for the lossy
unit records that [LOSSY.md](LOSSY.md) specifies (SPEC 2.8 gives the lossy
head). `V4:3`, for example, selects section 3 of [SPEC_V4.md](../SPEC_V4.md).
The bundle retains both specification maps and their source hyperlinks.
Labels describe themselves: lossless stripes and groups name their rows,
columns and slots (`stripe 0 (rows 0-63; slots G, R, B)`), lossy records
name their planes (`AC stripe 0 group 0 (planes Y, X, B-Y: DCT
coefficients)`, `DC stripe 0 group 0 (Y, X, B-Y DC, one value per 8x8
block)`), and slot streams name channel and depth. `value` is a display
string (capped at 160 characters). The byte range always
addresses the full data. The optional `glossary` defines, once per document,
the codec's terms that labels use (IMI: stripe, group, slot, lane, stream,
predictor, decorrelation, palette, bilevel, template, typical row, record and
the lossy terms); the viewer shows each definition whose term appears in the
selected node or an enclosing span.

Payload heads list their records one node each (`record 35: template palette
tables`, `record 37: unit record ordinals`, with id, length and payload fields), named from the record registry
and linked to the section that defines them. When `unit_ordinals` (SPEC 2.7,
feature 26, head record 37) is active, each main unit record traces its leading
`unit record ordinal` varint before the unit's inner streams. A template palette stripe
(SPEC 2.14) shows its typical-row bitmap and its index-plane rANS stream; a
bilevel stripe (SPEC 2.10) shows its value stream. A quick-pixel unit
(SPEC 2.13) shows its mode byte and then its raw samples, or its packet stream
with a census of the packet families (literals, cache indexes, differences,
luma differences, runs and the pixels the runs cover). A bit-smart unit
(SPEC 2.13.1) shows its LEB128 plane lengths and each byte plane with its kind
and metadata: the constant, the base and mask, the dictionary, or the earlier
plane it repeats. Slot streams name
their channel and depth, e.g. `slot 0 (G, 8-bit)` or, in a lossy file's
nested DC image, `slot 0 (X DC +32768, 16-bit)`.

## V4 tracing

### Local review and visible-map regression, 2026-11-16

The local review uses the qualified V4 WASI engine from `fd375f41` and the
current Atlas page. `atlas-review-prepare` adds eight recognizable examples
from the already bundled imazen-26 coast photograph 1407 and line plot 7000.
The codec's own PNG reader takes a centered 512×512 crop without resampling;
its lossless/Quick output is checked against its converted source pixels.
Every example has successful production decode and complete byte attribution.
Original examples and sources are preserved. The review gallery starts with
these eight examples; a checkbox reveals structural fixtures. Encoder input
also starts with the photograph. Generated catalogs remain outside git.

A tile annotation can have a larger node ID than its children. Computing
depth in array order left six nodes with non-finite depths in the real-source
lossless witness; that made every byte-map rectangle invalid. The viewer now
resolves parent depth first. The map starts open, the byte scrubber starts at
the complete byte count, and the image starts fully visible with autoplay off.
Stable scrollbar gutters prevent content width from changing as panels open.
V4 lossy coding is classified from its header/setup instead of nested numeric
DC notes. The display recognizes V4 in retained trace-enum descriptions while
preserving raw trace values and encoded byte ranges.

`atlas-review-check` passes in Chromium, Firefox and WebKit over the local HTTP
URL. It checks nonempty map/image pixels, byte scrubbing, panel/viewport layout
at 700/1100/1440 CSS pixels, model/grid controls, production WASI trace/reference
pixels and all six form presets. Final UI receipt before repackaging: 10 s,
run-heavy peak RSS 0.45 GiB, min available 46,223 MiB, peak load 0.66. These are
functional checks, not throughput measurements. Package preservation checks
also pass. Deployment and the real public gigapixel exercise remain separate.

```sh
just atlas-review-prepare COMPLETE_STAGE/codestream-atlas/data.js QUALIFIED_WASI FRESH_DATA_JS
# Supply that catalog to site-package-v4 with its qualified WASI engine.
just atlas-review-serve COMPLETE_REVIEW_STAGE 3318
just atlas-review-check http://localhost:3318/codestream-atlas/ FRESH_RESULTS
```

The active review service and exact local artifact paths are recorded in
[the current review triage](V4_REVIEW_TRIAGE_2026-11-16.md).

### Production trace coverage

The native trace labels V4 bound records (`pREC`), record pages (`pIPG`),
directory (`pIDX`) and locator (`sLOC`). Bound records include their ordinal.
Page notes show the first record, count, and length encoding. Compact pages
also show the base and residual width. The directory's notes identify the
schema and nominal page size. These decoded details annotate the packed
physical byte spans. The canvas still displays the primary image, so native
level selection and view overlays remain separate work.

Scoped check declarations and the exact byte spans for group-pixel, tile-pixel
and encoded-object check results appear under V4 section 6.2. Older 16-byte
object digests keep their own spans. These labels identify bytes and scope.
The decoder's existing verification policy governs which checks run.

The native library tests exercise 25 bound records and two pages in one
file, and both 64-bit and 128-bit checks in each scope on a lossless and a
lossy file. The local Chromium 153 bundle rendered schema 2, a nominal 16
records per page, and the two packed residual widths of 1 and 2 bits. The
3,951-byte file decoded successfully, every byte was attributed, and the
byte map and timeline controls initialized. The V4 grid and Python bundle
gates also passed.

The final V4 browser gate runs production trace/decode and resolved-grid controls
in Chromium, Firefox and WebKit. The Atlas form also encodes six presets through
the WASI worker: lossless, Quick, bit-smart Quick and three lossy configurations.
Every result declares V4, has complete byte attribution and matches the browser
binding's reconstructed pixels; the lossless cases match source code values.
The form offers the two supported entropy coders and shows group IDs as mandatory
V4 syntax. Native typed lab inspectors read DC, AC-table/seed summaries, channel
maps and plain 8×8 coefficient exports through the same V4 models. Source-bound
receipts and platform limits are in [V4 implementation](V4_IMPLEMENTATION.md).

The V4 trace comes from the actual outer-file, setup, directory, object,
tile/model/unit and reconstruction readers. Outer admission is shared with
normal decoding instead of parsing the file again to choose a driver. Header,
setup, placement, definitions, IDs, bank/lifetime/target fields, directory
entries and integrity trailers retain their encoded byte positions. Fields
whose bodies remain opaque say so; the trace does not invent symbol-level
entropy attribution.

A `tile` span covers the contiguous opening-marker through closing-marker
range and carries its clipped pixel rectangle. Its group `record` children
retain their own rectangles and complete framed sizes, including local digest
bytes. DC records cover their source-pixel coverage; AC and alpha records cover
their group. Model IDs and other full-width field values are display strings,
not JavaScript numbers. Unit records retain their typed model dependency notes.
Only a successfully checked object gets the local-integrity verified note.

The existing viewer consumes those record annotations for its image and 3D
layers. Spatial tiles also have a separate dashed outline in the image view;
they are storage extents, and do not replace individual group jobs. The field
sheet selects V4 specification sections. `bundle.py` requires the sibling
SPEC_V4.md and preserves both spec maps; a missing specification fails before
writing output.

The native trace reconstructs one row band, not a decoded raster. Its admitted
complete-input descriptor/model plan remains subject to `DecodePolicy`; Atlas
additionally caps its own output row at 64 MiB before allocating it. This is a
diagnostic-tool allowance, not a wire canvas limit or a process-RSS guarantee.
Lossy AC records also carry `V4 effective cells`: a compact run list emitted
by the production settings resolver after inheritance and sparse overrides.
The viewer selects a color plane and draws its actual transform footprints,
including 16×16/16×8/8×16 merges, small transforms and sampling. Edge drawings
are clipped to visible pixels while their labels retain the transform size.
This semantic annotation does not invent byte spans or reinterpret model IDs.
It is absent when tracing is disabled. Runs avoid a dense per-pixel diagnostic
map. The plane selector and legend report which transform paths are displayed.

Native trace and JavaScript geometry checks pass. Chromium, Firefox and WebKit
also pass the current V4 grid and WASI trace/decode gate. Missing WebGL contexts
fall back to the pixel view without an unhandled page error; the byte fields and
transform grids remain available. Encoder controls and the complete local site
package also pass their final integration gates. The real-source public
gigapixel/Atlas exercise remains outstanding; local synthetic/browser gates
do not establish that the public deployment serves the qualified V4 assets.

The native V4 fixtures require successful decoding, no overlapping/out-of-parent
leaves and no unattributed byte. Numeric fixtures cover actual clipped Morton
tiles/groups, tail/front/no directory and indexed open DATA; their serialized
rectangles match an independent source-geometry calculation. Both lossy coders
exercise settings/models, regions, dependency notes and a corrupt checksum
outcome. The existing V3 ordinal/index trace witness remains unchanged and passes.
[Source identities and build receipts](V4_IMPLEMENTATION.md#production-v4-atlas-tracing)
qualify these checks, not browser performance.

## Viewer

`tools/codestream-atlas/`: `index.html` (static, no build step) plus a
generated `build/data.js` (`bundle.py` packs traces, decoded RGBA8 pixels from
`codestream_trace --rgba`, and the SPEC.md/SPEC_V4.md section text). Views:

- **Byte map:** an icicle chart over byte offsets, one row per nesting depth.
  Click a span to zoom to it.
- **Syntax tree** with values and sizes.
- **Sizes** read "8,413 bytes (compressed to 32%, 2.7 bpp)": coded bytes,
  their share of the uncompressed bitmap in the output format, and bits per
  pixel (file header line, picker, field sheet, encoder status).
- **Field sheet:** byte range, value, region, bits per pixel, the normative
  spec text of the node's section, and a hex dump colored by field.
- **Image plane:** decoded pixels with record regions, or a bits-per-pixel heat map.
- **Decode-order playback:** walks the records in read order, revealing
  only pixels whose records have been read.
- **Decode timing:** each file's native decode times, measured on the build
  host with input and output in memory (median of REPS decodes at each profiled
  thread count), and a timeline of one observed decode: one bar per lossless
  unit, per worker lane, from a profiling `how_far::Pulse`
  ([decode progress](ZENCODEC.md#decode-progress)). Clicking a bar opens the
  unit's record. "Time in this browser" runs the same `imij-wasm profile-decode`
  command in the page's worker and compares the browser's median with the native
  1-thread time. Lossy units appear in pairs (two stripes decode together);
  native JPEG payloads show times but no units.
- **Appearance:** selectable light and dark themes persist in browser storage;
  without a saved choice, the viewer follows the system preference.
- **3D:** bars over the image plane whose heights are bits per pixel per
  record. DC records sit translucent beneath the AC layer. three.js
  (0.170.0, an ES module from cdn.jsdelivr.net) loads only when the 3D view
  is first opened.

The Atlas is published on **Cloudflare Pages** at
[imi.imazen.org/codestream-atlas/](https://imi.imazen.org/codestream-atlas/)
(since 2026-10-24; previously on GitHub Pages, which is now legacy). The site
is public although the repository is private, and it carries SPEC.md section
text and the in-browser encoder.

To redeploy, run `just deploy-atlas PHOTOS`: it rebuilds the four atlas files
into the staged complete site and uploads that site (a Pages deployment
replaces the whole site). See [site deployment](SITE_DEPLOY.md).

## Reuse across codecs (plan)

1. **Shared recorder and schema.** Move `trace.rs` (recorder, macros, JSON
   writer) into a small no_std+alloc crate that every zen codec can depend on
   behind its lab/trace feature. The macros stay zero-cost when off.
2. **One emitter per codec, placed where the codec's own reader consumes
   bytes.** Candidates: JPEG marker segments, scans and MCU rows (zenjpeg);
   RIFF chunks and VP8 partitions/macroblock rows (zenwebp); JXL
   box/codestream/TOC/groups (jxl decoder); AVIF/HEIF boxes plus AV1 OBUs and
   tiles (zenavif-parse, rav1d-safe); PNG chunks and IDAT/deflate blocks (zenpng).
3. **One viewer.** It reads only the schema; codec-specific vocabulary
   arrives as node kinds and spec links.

## Next steps

- Bit-level and symbol-level layers: per-lane rANS states and word streams
  are traced; a sampled symbol trace for one block row (bit ranges, the coding
  step animation) is not. Fields gain optional `bit_start`/`bit_end`.
- Head record payloads (unit table hints, bilevel and template probability
  tables) appear as one field each; their inner fields are not traced.
- **The lossy unit record needs a normative SPEC.md section.** SPEC 2.8
  specifies the lossy head; the DC, alpha and AC records are specified in
  docs/LOSSY.md. The trace shows what the decoder reads; it does not replace
  a written specification.
- Cross-check against the independent `stream_walk.py` byte accounting.
- Coding-process animation (DCT, quantization, color transform) for selected
  blocks, from encoder-side lab traces.
- Needs the user: which repository hosts the shared crate.

## In-browser encoding (2026-10-06)

The Atlas page can encode IMI files itself: an "Encode an IMI file" panel
takes a bundled source or an uploaded PNG, conversion options (channels,
sample type, widening rule, significant bits) and lossless or lossy settings
(quality, effort, color, entropy or decode-speed tier, compact streams,
stripe height, column groups, AQ, per-plane transform and sampling). It runs
`tools/imij-wasm` (a WASI command, `#![forbid(unsafe_code)]`, no exports) in a
Web Worker through `tools/codestream-atlas/wasi-worker.js`, a small WASI
preview1 shim (argv, stdin, stdout/stderr, exit, clocks, random; no file
system). Each result is traced and decoded by the same engine and opens in
the Atlas with its compression stats. A plain page saves the .imi through a
blob link. Inside the artifact viewer, pages may save only through
the `downloads` capability, whose extension allowlist has no `.imi`, so the
page writes a stored (uncompressed) zip holding the .imi unchanged.

Verified: imij-wasm `tests/parity.py` 66/66 (wasmtime vs native: identical
encodes, traces and errors; lossless round trips exact); end to end in
headless Chromium via CDP (RGB8 lossy, RGBA8→U16 10-bit lossless, 1 MP photo
lossy with adaptive layouts and AQ in about 90 ms, 16-bit PQ lossy), each with
zero trace problems.

The engine is built with WebAssembly SIMD (`tools/imij-wasm/.cargo/config.toml`
sets `+simd128`), which compiles in imij's wasm128 kernels. Against a scalar
build of the same source (wasmtime on the dev box, 1 MP photo, median of 15
shuffled rounds; informal, not the qualified timing protocol): lossy q50
encode 55.1 → 33.1 ms, lossless encode 236.7 → 180.9 ms, lossy decode
59.6 → 26.5 ms, lossless decode 103.6 → 86.8 ms (decode is process wall time
including startup). Parity with native stays 66/66 (70/70 since the template
and bilevel fixtures were added on 2026-10-15, 76/76 with the value-table
fixture on 2026-10-16).

Fixtures: `tools/codestream-atlas/make_fixtures.sh ENGINE TRACE OUTDIR
SOURCE.png... [--lossless-only SOURCE.png...]` center-crops each source to
192x128 (`imij-wasm encode --crop`), encodes it lossless and as five lossy
variants, and traces each file. Each crop is also encoded as three quick-pixel
files: 8-bit packets, and bit-smart byte planes of the samples widened to
16 bits by a shift (constant low bytes) and by replication (repeated planes).
`codestream_trace --rgba` previews 16-bit samples by their high byte. Sources
after `--units` are cropped to 512x512 and encoded lossless in 64 units
(32-row stripes, 128-column groups) for the decode timeline. Sources
after `--lossless-only` are encoded
whole, lossless at the default effort, at effort 10 and with palettes off:
the synthetic `dither16.png` (16-colour ordered dither) and `bilevel.png`
(two gray values) from `emit-fixtures`. Both use template palette stripes by
default; with palettes off, `bilevel.png` uses a bilevel stripe (730 bytes
against 601 as a template palette) and `dither16.png` uses predictor
stripes. `sparse16.png` (RGB16, each channel 120 scattered values along a
smooth index field) shows value-table channel maps (SPEC 2.1 map 9): every
channel carries a 120-entry table and the file is 1,893 bytes, 5.1% of the
raw samples (2,119 with palettes off). The bundled set uses `local-scratch/llb-data/1024/png-high/1407.png` (a 1024-pixel
rendition of imazen-26 `1400-lilith-nature/1407_...8160x6120.jpg`),
imazen-26 `7000_plots_line-00056-s6bcec02a_1024x1024.png` and imazen-26
`5200_epa_climate-impact-2021_cover_p001_2968x3841.png`.

Build (see [docs/SITE_DEPLOY.md](SITE_DEPLOY.md) §3 for the full deploy chain):

```sh
cd tools/imij-wasm
cargo build --release --target wasm32-wasip1   # WASI engine for in-browser encoding
cargo build --release                           # native, for fixtures and parity
./target/release/imij-wasm emit-fixtures FIXDIR --hdr-png local-scratch/zqi/corpus/hdr/1228.hdr.png
python3 tests/parity.py target/release/imij-wasm target/wasm32-wasip1/release/imij-wasm.wasm FIXDIR
cd ../..
cargo build --release -p imij --example codestream_trace --features lab
# Generate traces from fixture PNGs + photo sources (see make_fixtures.sh --help):
bash tools/codestream-atlas/make_fixtures.sh \
  tools/imij-wasm/target/release/imij-wasm \
  target/release/examples/codestream_trace \
  TRACES \
  FIXDIR/rgb8.png PHOTO1.png PHOTO2.png \
  --lossless-only FIXDIR/dither16.png FIXDIR/bilevel.png FIXDIR/sparse16.png \
  --units PHOTO3.png
# Optional: native timing on the measurement host while nothing else runs:
tools/codestream-atlas/profile_fixtures.sh tools/imij-wasm/target/release/imij-wasm TRACES 15 1 8
# Bundle:
python3 tools/codestream-atlas/bundle.py TRACES SPEC.md data.js FIXDIR/*.png [more.png]
```

The atlas deploy directory is four files: `index.html` (from source), `data.js`
(generated), `imij-wasm.wasm` (WASI build) and `wasi-worker.js` (from source).

Fixture binaries are not committed; `emit-fixtures` regenerates them byte for
byte and `tools/imij-wasm/fixtures/FIXTURES.tsv` records their sha256.
