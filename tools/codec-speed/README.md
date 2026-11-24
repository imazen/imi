# Real-photo codec speed check

`just codec-speed-build`, then `just codec-speed-run NEW-OUTPUT photo.png ...`.
Run both through the normal resource wrapper. Use an exclusive release target
directory and record the source revision, Cargo.lock, compiler, CPU and binary
hash. No `target-cpu=native` flags. Input PNGs must be RGB8 and at least 1024².

The tool takes a centered 1024² crop without resizing. Input decoding, cropping,
validation and file I/O are outside the timers. It records each encoded file,
raw crop and crop coordinates. Preparation timings account for the encode budget;
final timings come from **zenbench PR #33**, pinned to
`78a9dba0bdb4edfe09fd666ae0208944c3d2cd6d`.

- IMI: default lossless; default XYB/run-level lossy at q50 and q80, effort 7.5.
- zenjpeg: sequential (baseline) YCbCr 4:2:0 at q50 and q80, restart every four
  MCU rows. The same bytes are decoded serially and in an eight-worker pool.
- zenpng: lossless efforts 1, 2 and 7; ordinary non-iDOT PNGs. Decode with one
  thread and the two-thread inflate/unfilter pipeline.

Generate plans with `python3 tools/codec-speed/pair_plan.py PREPARED PROBE PLANS`,
where PROBE is the release `pair_probe` executable. Run them with
`just codec-speed-pair ZENBENCH PLANS NEW-OUTPUT`. Install the pinned PR using
`cargo install --git https://github.com/imazen/zenbench --rev
78a9dba0bdb4edfe09fd666ae0208944c3d2cd6d --locked --bin zenbench` (one shell line).

Zenbench runs three randomized fresh-process blocks with an A/A baseline clone.
Decode plans compare the same file at 1 versus 8 threads (PNG: 1 versus 2),
checking SHA-256 of reconstructed pixels. Each process performs three warmups
and measures ten allocating decodes as one latency observation. Encode processes
warm up once, measure one single-thread encode, and require both outputs to
match the retained encoded fixture. The encode plan compares the same raw input
across settings; it does not equate the lossy outputs' visual quality.

Report the slowest of the three process observations for each arm/image, and
retain A/A diagnostics. Input I/O, hashing, comparison and pool construction are
outside timing. Decoder allocation/destruction are inside; encoder output
allocation is inside and output destruction is outside. This is exploratory,
not a regression acceptance test. Process observations stay clustered.

Count `encode_total_ms` from every encode child, including warmups and A/A,
plus preparation time, against the two-minute budget. Moderate PNG levels and
fixed three-block counts keep this study small; no optimization sweep is run.
Images are processed sequentially, with parallelism only inside each decode.

Lossless outputs must equal the original crop exactly. All thread counts must
return identical pixels. Equal lossy quality numbers are **not** matched visual
quality, so this is a throughput/size snapshot, not a rate-distortion ranking.
Metadata is excluded from the newly encoded samples.

For bounded CPU attribution, `just codec-speed-profile PROBE PREPARED NEW-OUTPUT`
uses perf samples and four two-second loops (encode/decode, IMI/zenjpeg q80 on
photo 1400). Run through the normal resource/host wrapper. This is a diagnostic,
not a replacement timing table: startup, validation and profiler overhead remain
in the trace. Retain the program hash and report exclusive symbol samples. The
published default IMI comparison uses full-resolution XYB, not its opt-in
subsampled layouts; JPEG is 4:2:0. Neither matched quality nor equal codec work
is implied.

For the bounded fast-lossless comparison, `just codec-fast-lossless PROBE RAW-CROPS
ZENBENCH NEW-OUTPUT` reuses the three retained 1024² RGB8 crops (1000/1400/1600).
It prepares QOI through pinned zenbitmaps and IMI efforts 0, 2.5 and 7.5, checks
exact original pixels at each requested thread count, then runs PR33 three-block
A/A plans. Encode is serial; IMI decode requests 1/2/4/8 workers, QOI is serial.
The setting argument accepts actual fractional effort. No lossy or JPEG path is
used. The script accounts for preparation, warmups and measured encode calls
against an additional 60-second encode budget; it retains all full process
artifacts. Report worst-of-three throughput and bitrate, with individual images
and A/A noise. QOI here means zenbitmaps' vendored rapid_qoi implementation, not
QOIR or an assertion about every QOI implementation. No format change or
performance win is implied by preparing the comparison.

`just codec-fast-lossless-profile PROBE RESULTS NEW-OUTPUT` samples four
two-second single-thread loops on retained photo 1400: QOI and IMI effort 0,
encode and decode. It reuses the same checked probe and perf settings as the
lossy profile. Startup, validation and sampling overhead remain included; these
are attribution diagnostics, not substitutes for the PR33 throughput table.

`just codec-lossy-tiers PROBE INSPECTOR RAW ZENBENCH NEW-OUTPUT` adds the missing
explicit Compact/Balanced/Fast policies, forced SplitHuf and unchanged default
at q20/q50/q80, effort 7.5. It uses the same real crops and PR33 worst-of-three
protocol, with requested 1/2/4/8 decode threads. Build the timing probe with
default features, preserve it, then build `inspect_tier` with `--features inspect`
separately for untimed wire inspection. Each generated file is checked at all
thread counts; timing processes compare against serial reconstruction. Decode
plans compare thread counts of the same file, not potentially different policy
rasters. Encode plans check exact retained bytes. Policies can change layouts
and reconstruction, so these are not matched-quality rates. The additional
encode budget is 60 seconds; raw observations and actual coder choices remain.

For the direct policy comparison, `just codec-lossy-tiers-paired PRIOR ZENBENCH
NEW-OUTPUT` reuses those immutable files and binaries, interleaving all policy
and thread arms within each image/quality block. It performs no encodes. The
same-file serial/threaded pixel assertions remain in the child; the summarizer
also requires every full pixel digest to match the retained per-file oracle
from all previous thread routes. Only output byte count is shared across
policies in PR33's check field because their reconstructed pixels may differ.
The earlier within-policy thread comparison remains retained separately.

`just codec-lossless-change BASELINE CANDIDATE PRIOR-LOSSLESS ZENBENCH NEW-OUTPUT`
compares an encoder change against the retained lossless fixtures. It first
prepares all nine image/effort files using the candidate, checks original pixels
at 1/2/4/8 threads and requires complete encoded byte identity with the baseline.
PR33 then interleaves the two programs plus A/A in three blocks. Both warm and
measured encodes must reproduce those original bytes. Report worst-of-three
throughput, each source's timing, unchanged bitrate, A/A and paired diagnostics.
It changes no effort policy or timing threshold and measures no decoder speed.

`just codec-feed-change BLOCKING-PROBE FEED-PROBE PRIOR-LOSSLESS ZENBENCH
NEW-OUTPUT` measures the caller-feed interface on the same nine retained files,
without encoding. It compares serial blocking callbacks with a 64 KiB input
queue and one copied typed provisional row, including request construction,
admission and final validation. Both arms validate exact source pixels before
timing; each process then completes three warmups and ten timed decodes.
PR33 uses three paired blocks plus A/A. Report worst-of-three throughput,
paired latency changes and A/A separately. The input and validation rasters
stay alive outside the clock, so process RSS is not decoder-only memory.
This measures the additional interface cost, not a shared-path no-regression
claim or animation/page streaming.

## Full-reference IQA storage

`just seaweed-cross-image GROUP NEW-OUTPUT PACKER RANGE-CLIENT BUCKET PREFIX`
packs one already verified 126-image IQA group as independent entries and a
pristine-reference group, compresses both archives as whole Zstd objects, then
checks local S3 uploads, full readbacks and HTTP range access. Store credentials
and endpoint come from the caller's environment. This does not encode images or
provide cross-file IMI references. See the
[SeaweedFS study](../../benchmarks/seaweed_cross_image_2026-10-13.md).

Build `iqa_storage` with `just iqa-storage-build` in an exclusive release target.
`just iqa-storage-prepare DATASET NEW-INPUTS` validates the original KADID-10k
`dmos.csv` and RGB8 PNG headers. `just iqa-storage-run PROBE MANIFEST PNG-DIR
NEW-OUTPUT` runs every group, independently checks zenpng rasters with installed
Pillow, verifies exact native pixels/attributes and packages both archive arms.
No missing image is skipped. PNGs remain original; no resize or distortion
regeneration occurs. Preserve original decimal scores and all output files.

`just iqa-storage-repack PROBE FULL-STUDY NEW-OUTPUT` verifies new archives against
unchanged native payloads, including actual image header summaries and optional
sequence indexes. `just iqa-storage-summary FULL-STUDY NEW-SUMMARY ARCHIVE-SUMMARY`
reconciles complete sizes, actual selected references and sampled dependency work.

`just iqa-storage-timing PROBE FULL-STUDY PNG-DIR ZENBENCH NEW-OUTPUT` measures fixed
I01: encode at one thread, full scan and final-image access at 1/2/4/8 threads.
PR33 uses three randomized blocks plus A/A and a bounded 180-second admission wait.
Report worst of three. One encode warmup or three decode/access warmups precede
one timed operation. Source loading, hashing and pixel assertions are outside the
clock, except returned sequence reconstruction checks inside encoding. Full
encoded groups are loaded; this does not measure archive or remote range I/O.
Access throughput counts the requested output pixels while its time includes all
reference reconstruction. Full-study size results and fixed-group speed results
have different coverage; keep them separate. See the
[completed comparison](../../benchmarks/iqa_storage_2026-10-12/README.md).

`just iqa-storage-archive-access PROBE ARCHIVE I01-GROUP PNG-DIR independent|reference THREADS`
opens an actual archive, finds the final I01 image or its reference-page group,
checks the indexed payload digest, and reconstructs the selected image and its
declared prerequisites. It reports lookup, verified-copy and decode times
separately. Three complete warmups precede one observation; source PNGs are loaded
before timing and every decoded prerequisite is checked against them afterward.
This is local warm-cache access, not network range latency.

`just iqa-storage-archive-batch PROBE ARCHIVE I01-GROUP PNG-DIR independent|independent-list|reference THREADS`
measures the 25 fifth-level distortions in one archive session. The reference
arm verifies the group once and retains its pristine raster; the independent
arm fetches each indexed image. `independent-list` uses one sorted archive
listing to locate those images. All arms check every requested output against
its original PNG. The paired `iqa-storage-archive-timing` and
`iqa-storage-archive-batch-timing` recipes use zenbench PR 33 for three process
rounds with A/A controls and retain per-stage times.

`just iqa-storage-archive-decode-profile PROBE ARCHIVE I01-GROUP PNG-DIR independent|reference`
loads and verifies original pixels and the archive entry first, then runs a
four-second single-thread decode loop for CPU attribution. The reference arm
retains the decoded pristine raster and a frame decoder; the independent arm
reuses an output buffer. Run it under `perf record` and the normal host/resource
wrappers. Its iteration count is diagnostic, not a fresh-process speed result.

`just iqa-storage-archive-binary-pair BASELINE CANDIDATE INDEPENDENT-ARCHIVE REFERENCE-ARCHIVE I01-GROUP PNG-DIR ZENBENCH NEW-OUTPUT`
compares two preserved decoder executables on the same reference-backed page.
It retains three paired fresh-process rounds and A/A controls, checks source
pixels and archive digests, and records both executable hashes. This includes
the complete group lookup and verified copy; use the decode phase to assess
decoder work separately from archive access.
`iqa-storage-archive-batch-binary-pair` applies the same check to 25 distortions
that share a retained pristine reference.

For an independent lossless JPEG XL size comparison, build the ZenJXL `cjxl-rs`
CLI from a recorded source and lockfile. `just jxl-iqa-archive-size ENCODER
MANIFEST ORIGINAL-GROUPS PNG-DIR NEW-OUTPUT fast-decode` encodes the same
original KADID inventory at effort 13 and `--faster-decoding 4`; use `max` for
effort 13 and the default decode-speed tier 0. A caller may pass `--groups I01`
as the final argument for an explicitly scoped group. The collector checks
each PNG against the prior source table, journals each JXL byte length and hash,
and fails on partial or changed outputs when resumed. These totals describe
independent image payloads; they exclude per-image attributes and archive index
bytes, and cannot be substituted for the reference-predicted IMI archive size.
After a collection finishes, `just jxl-iqa-archive-oracle DECODER ORIGINAL-GROUPS
COLLECTION NEW-OUTPUT DISK-SCRATCH` checks each decoded RGB8 raster against the
original source hash with an independent JPEG XL decoder. Neither recipe
measures decode speed.
