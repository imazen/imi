# JPEG-native comparison harness

Measurement-only source, staged at `tools/native-study` in the exact native
component candidate `d75527f658a55ae17e776a7622d970d9e4bded52`. It requires that
candidate's API; this is not a member of the main workspace. Copy `main.rs` to
`src/main.rs` and the candidate's `tools/imij-jpeg/src/importer.rs` unchanged to
`src/importer.rs`. This reuses the owning JPEG adapter, not a second parser.

The same centered 1024² real-photo RGB8 crops feed baseline sequential JPEG420
at q1/5/15/30/50/65/80/95 (q1 is a retained preparation-only extension). Each JPEG is stored as native IMIJ with band8 and both
entropy coders, preserving coefficients/tables. Direct XYB is a separate arm.
Efforts are 0/3/5/7.5. All sizes include the entire file; record actual SSIM2 and
RGB differences, never presume coefficient identity means identical RGB.

Preparation and every timed result verify their own retained pixels and bytes.
Native encode timings include JPEG coefficient extraction and native encoding;
this is transcoding cost, not an RGB encoder. A JPEG→IMIJ pipeline also pays
JPEG encoding first. Do not silently compare transcode throughput to RGB encode.

Timing uses zenbench PR33, three fresh-process blocks plus A/A, worst of three.
Only q50/q80 are timed to bound the study; the full ladder supplies RD curves.
Decode has three warmups (first checks pixels) and one observation averaging
three allocating decodes. The preparation pixel check uses 1/8 threads, and
native files also check exact coefficient/table preservation and the normative
native reconstruction. Metric scores are outside encode timers.

The controller must account for all encode/transcode calls, including warmups,
against the retained two-minute budget; stop explicitly rather than drop a
failed or incomplete cell. This small study does not qualify a release or
establish native ARM behavior. No quality/default choice is made solely from
this SSIM2 snapshot.

Whole-image-band sensitivity reuses effort 7.5 and both native coders with
64 MCU rows on these 1024² inputs. It verifies exact pixel identity against
band8, then times single-threaded decode separately; it makes no eight-worker
claim. `summarize.py` accepts main, q1 and whole-band preparation roots.
`whole_decode.py` records the six additional PR33 plans.
