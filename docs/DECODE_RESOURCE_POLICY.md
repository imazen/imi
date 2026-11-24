# Decoder resource admission policy

2026-10-04. `imij::decode_resources::{DecodePolicy, DecodeSession, DecodeUsage}`
provides resource admission and cooperative cancellation for serial slice
decoding of the primary image, including synchronous caller-buffer row delivery. The implementation is in
[`decode_resources.rs`](../imij/src/decode_resources.rs); public behavior is
covered by [`decode_resources.rs` integration tests](../imij/tests/decode_resources.rs).

The policy combines input, metadata, chunk, unit and work allowances with a
conservative plan for codec working capacity. The plan is checked before
dimension-dependent decoder buffers are created and before pixel reconstruction
mutates caller output. It is an admission estimate, not a live allocator tracker,
process memory limit, CPU instruction limit or execution-time deadline.

## Scope and API

`DecodeSession::new(policy)` creates a session without cancellation.
`DecodeSession::with_cancel(policy, cancel)` additionally accepts an
`Fn() -> bool + Sync` callback; `true` requests cancellation.
The default session stores no callback and selects a statically known no-op
reconstruction check once per image. Explicit cancellation keeps its callback.
`Error::Cancelled` is a new variant of the unreleased error enum; downstream
exhaustive matches must add an arm even if they retain ordinary entry points.

`session.decode(data)` returns `(ImageInfo, Vec<u8>)` with tightly packed,
native-endian samples. `session.decode_into(data, out, stride)` writes into
caller storage; stride is in **bytes**. Both decode one primary image with one
worker. `session.decode_rows(data, row, on_row)` delivers tightly packed rows
through a synchronous callback without retaining the complete output image.
All three methods use the existing container and lossless/lossy parsers, with admission
checks, and retain the parsed model for reconstruction. The bounded lossy route
uses the ordinary fixed reconstruction chunk and does not retain lab scratch
across calls.

Nested lossless DC and alpha images required by lossy reconstruction share the
session's admission accounting and cancellation checks, as does the nested
exact-block image (its head through the lossless formula, two decoded nested
stripes and the exact pixels' work at its head; its stripes count as units). Their actual declared
lanes, maps, palette state and stripe geometry are accounted for, including DC
stripes taller than an outer lossy stripe.

Preview and frame chunks are counted as outer chunks and their borrowed
descriptors receive a working-capacity allowance. Their image payloads are not
recursively decoded. Indexed plane records are counted even though this route
ignores their payloads. This API neither advances animation frames nor traverses
previews, gain maps, plane tiles or ROI requests.

The existing `decode`, `decode_with_limits`, `decode_into`, threaded slice,
`StreamDecoder`, `decode_reader`, `FrameDecoder` and indexed-plane entry points
do **not** inherit `DecodePolicy`. Their existing validation and limits remain
in force; they are unrestricted with respect to this aggregate session policy.
The zencodec animation executor separately instantiates these same admission
and reconstruction checkpoints for frame advancement. It carries cumulative
usage between frames and budgets retained references; see
[the adapter contract](ZENCODEC.md#composited-animation-decoder). This does not
change the policy of ordinary native `FrameDecoder` calls.
In particular, `Limits` remains an image/output limit. The encoder's
[`DecoderMemoryLimit`](DECODER_MEMORY.md) is a geometry-planning target and does
not activate decoder admission. Its estimates and the session's conservative
charges have different scopes; using the same numeric byte limit in both APIs
does not currently guarantee that the encoded file will pass session admission.

`session.decode_reader(reader, on_row)` (2026-10-06) is the session form of
`decode_reader`: the same streaming parser, instantiated with the session's
admission in the row profile. Input is charged as it is delivered and capped
at the remaining cumulative allowance (bytes consumed by a failed call stay
charged); the header, every chunk, retained metadata (after its bytes
arrive), models, nested DC/alpha heads, records and planned working memory
are admitted from the parsed heads before their buffers grow, and
cancellation is checked at parser, stripe and nested-row boundaries. Rows
are delivered as they decode, so a later entropy error or a trailing chunk
can fail the call after callbacks. The policy's output limits replace a
per-call `Limits`. Tests: `imij/tests/decode_reader_session.rs`.
A `cSUM` file checksum counts as an outer chunk, not as retained metadata;
the output limits' `Limits::verify_checksum` turns its checks off. Slice
methods check every piece while parsing, before any output; the reader form
checks pieces as they arrive (before decoding from them when the table
precedes `DATA`, after the rows when it trails).

## Caller-buffer rows

`decode_rows` accepts `FnMut(&ImageInfo, u32, &[u8]) -> Result<(), Error>`.
Indices increase from zero, and each callback receives exactly
`info.row_bytes()` native-endian bytes. Consume or copy them before returning;
the next row reuses caller storage. Bytes beyond that row extent stay untouched.
The callback runs synchronously and needs neither `Send` nor `std`.

The complete supplied slice, including suffix chunks, is parsed before the
first callback. A short caller row returns `InvalidInput` without writing it or
calling back. Full logical output-byte/pixel limits still apply even though the
codec delivers one row at a time. Parsed state moves directly into the existing
streaming decoder; row delivery does not reparse the file.

Rows use a separate private working-capacity profile. Lossless current column
groups, including nested lossy DC/alpha groups, remain live together. Their row
states and descriptors are all admitted. Lossy staging includes full-width RGB,
DC/AQ/alpha rows, transient DC conversion and group extraction. Shared tables
are not charged again merely because their ownership moves into a stream.
Existing conservative serial allowances remain; a row request may need a higher
working cap than serial slice reconstruction of the same file.

Cancellation is checked at row and group boundaries, including nested rows,
and immediately before each callback. No incomplete row reaches the callback.
A later entropy failure or cancellation can partially alter the caller scratch
row; earlier callbacks cannot be undone. Callback errors abort immediately.
Already admitted work remains charged. Callback work, allocations, retained
rows and caller scratch belong to the application, outside codec working memory.

## Defaults and builders

Start with `DecodePolicy::default()` and replace the allowances required by the
application. All methods return the updated policy. Budget denial returns
`Error::TooLarge`.

| Builder | Default | Accounting scope |
|---|---:|---|
| `with_output_limits(Limits)` | 120,000,000 pixels; 1 GiB | Separately for each primary image |
| `with_max_input_bytes(u64)` | 1 GiB | Cumulative supplied slice lengths |
| `with_max_header_bytes(u64)` | 64 KiB | Fixed container header body per image |
| `with_max_metadata_bytes(u64)` | 16 MiB | Cumulative conservative metadata payload allowance |
| `with_max_chunks(u64)` | 65,536 | Cumulative outer chunks, including DATA |
| `with_max_units(u64)` | 1,000,000 | Cumulative admitted stripe/group records, including DC and alpha |
| `with_max_work_units(u64)` | 8,589,934,592 | Cumulative coarse byte/sample/table work allowance |
| `with_max_working_bytes(u64)` | 256 MiB | Planned codec working capacity per call |

Byte units above are binary: KiB is 1,024 bytes, MiB is 1,048,576 bytes and GiB
is 1,073,741,824 bytes. The working allowance is additionally capped at the
host's `isize::MAX`; large caller values do not disable address-space checks.
Geometry arithmetic uses `u128`, and cumulative additions are checked before
conversion to public `u64` counters.

The fixed header allowance measures the declared header **body**, excluding
the signature and its length varint. It does not independently cap the DATA
coding head or entropy-model serialization. Those consume input and work
allowances, and their parsed tables receive working-capacity allowances.

Output `max_bytes` applies to the image's logical tightly packed byte count,
including when using `decode_into`. Caller stride padding and the total size of
the supplied output allocation are outside that byte count. Checked stride
extent and minimum output length are still validated. There is no cumulative
decoded-output byte or pixel counter: callers control retained output buffers.

For example, in a program with the ordinary Rust standard library:

```rust
use core::sync::atomic::{AtomicBool, Ordering};
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::{Error, ImageInfo, Limits};

fn decode_image(
    data: &[u8],
    cancelled: &AtomicBool,
) -> Result<(ImageInfo, Vec<u8>), Error> {
    let mut limits = Limits::DEFAULT;
    limits.max_pixels = 1_048_576;
    limits.max_bytes = 4 * 1024 * 1024;
    let policy = DecodePolicy::default()
        .with_output_limits(limits)
        .with_max_input_bytes(256 * 1024)
        .with_max_header_bytes(4 * 1024)
        .with_max_metadata_bytes(64 * 1024)
        .with_max_chunks(1_024)
        .with_max_units(65_536)
        .with_max_work_units(16 * 1024 * 1024)
        .with_max_working_bytes(16 * 1024 * 1024);

    let mut session = DecodeSession::with_cancel(policy, || {
        cancelled.load(Ordering::Relaxed)
    });
    session.decode(data)
}
```

For a request containing several images, create one session and call its
`decode`, `decode_into` or `decode_rows` methods repeatedly. Each call requires `&mut self`,
which prevents simultaneous decoding through that session. Separate sessions
have separate allowances; the application must bound their simultaneous number
and any output it retains. The API also works with `core + alloc`; the example's
`Vec` would need the corresponding `alloc` import in a `no_std` program.

## Counters and failed calls

`session.usage()` returns a `DecodeUsage` snapshot. Five counters accumulate
across calls: `input_bytes`, `chunks`, `metadata_bytes`, `units` and `work_units`.
`peak_planned_working_bytes` records the greatest successfully admitted planned
capacity reached by any call in the session.

Charges are not transactional. A failed or cancelled call retains every charge
that succeeded before the failure. A charge that itself exceeds its allowance
is rejected without adding that charge, while earlier counters can already
have advanced. For example, a chunk count or metadata charge can succeed before
its working-capacity charge fails. A cancellation check at the beginning can
fail before charging any input. Reusing a failed session therefore retains the
request's consumed allowances; constructing another session starts new ones.

`units` reserves the number of records implied by an admitted geometry. It is
not a count of successfully reconstructed stripes: records can subsequently
prove truncated or malformed. Models, record scans and nested geometry contribute
to `work_units`, which also charges each supplied slice's entire length and
metadata expansion. It is a deterministic coarse accounting unit, not a measured
instruction count or time estimate.

Open DATA requires a parser visit to find its endpoint, followed by the visit
that retains its reconstruction model. Both visits charge their model, unit,
work and planned working allowances. A corresponding length-bounded DATA file
can consequently be admitted under a smaller allowance. The open-DATA
regression verifies double unit admission and increased work on the same image.

Current metadata accounting charges raw metadata payload lengths, the declared
expanded length of `mCMP`, and twice a `gMAP` payload length to allow transient
copies before a fallback. DATA, borrowed previews/animation/frame payloads and
ignored plane payloads contribute zero metadata bytes; they still consume
input, chunk, work and descriptor allowances. Empty ancillary chunks therefore
consume chunk and descriptor allowances even with a zero metadata-byte cap.
Metadata before and after DATA shares the same cumulative cap.

## Planned working capacity

The current per-call plan adds conservative allowances for container setup,
metadata, chunk descriptors, models, sample-expansion tables, decoded record
indexes and decoder scratch. Each call resets its current working plan; the
session retains the greatest admitted value as its peak. Sequential images do
not add their working capacities together.

The plan can sum phases whose allocations do not remain live simultaneously.
It also reserves worst-case palette/reference state and Vec growth. Its peak
can therefore exceed the actual maximum of live requested heap capacity.

For lossless decoding, let `G` be the maximum column-group width, `L` the
declared lane count, `S` the number of coded slots, `R` the stripe height clipped
to image height, and `B` the working sample size: two bytes when every coded
slot fits 16 bits, otherwise four. Physical row positions are
`P = 16 * ceil(G / L)`. The principal state allowance is:

```text
P * ((4*S + 1)*B + 12*S) + 4*R + 8192
```

The first term includes the four ordinary rows and extra slot-zero row, plus
conservative reference rows. Optional palette state adds
`S*256*16*B + 256*bytes_per_pixel + G`. Tables add 24 KiB per slot/palette model
and the expansion-table payload selected by the shared `OutMap` allocation
rule. Identity, constant, shift and offset mappings allocate no expansion table;
nonidentity mappings of at most 16 coded bits reserve four bytes per entry.
Record indexes add 32 bytes
per admitted unit. Serial slice decoding holds one group's row state at a
time. Row delivery instead charges all simultaneous groups plus their state
list; actual nested DC stripe height governs nested scratch. A one-lane wide row is consequently much more expensive than a row of
the same output width with sixteen lanes.

For lossy decoding, let `K = ceil(G/8) * min(stripe_block_rows, ceil(height/8))`.
The current paired coefficient/EOB and side-information allowance is:

```text
2 * (2 * (K*(3*64*4 + 3) + 260) + K*160 + 64)
```

Additional allowances cover fixed entropy tables, the applicable shared
transfer table, bounded reconstruction planes, RGB/alpha staging, row/task
descriptors, record indexes and transform working space. DC and alpha are also
charged through the lossless formula. DC pixel/conversion buffers use the
actual nested group width, stripe height and pixel size. Scheduler allowances
include `64*height*groups + 512*outer_units`; using one worker does not eliminate
the existing whole-image row descriptors.

These formulas describe the current implementation's admission strategy and
must stay in step with decoder allocation changes. They exclude compressed
input storage, allocated/caller-owned output buffers, thread stacks, allocator
bookkeeping and runtime storage. Included stack working-space allowances do
not measure the process's actual stack reservation.

Measured requested heap capacity and RSS are separate quantities. Input and
output must be accounted for independently when comparing a process measurement
to the codec plan. RSS also depends on page residency and runtime behavior.
Passing admission establishes only that these source-derived charges fit the
configured policy; it is not proof that every actual allocation or process
resource has been independently bounded.

## Cancellation and output validity

Cancellation is cooperative at parser/model/record and decode-unit boundaries,
including rows of nested lossless DC/alpha images. The callback uses no clock
and provides no hard wall-time or cancellation-latency guarantee. Cancellation
returns `Error::Cancelled` and preserves prior admission charges.

`decode_into` completes budget admission before writing pixels. Admission
denials and cancellation during admission leave output unchanged. After
reconstruction starts, cancellation or malformed entropy can leave partially
written pixels. An error does not roll back those writes; only successful
completion establishes a complete decoded image. Padding beyond each logical
row remains caller storage, and the deterministic reconstruction-cancellation
tests verify that this padding stays unchanged.

The allocating `decode` method returns no output buffer on failure. This does
not undo the session's counters or make the failed execution cost free.

## Completed deterministic validation

The 15 integration tests passed in release with default features and with
`--no-default-features --features extended`, exercising the library's `no_std`
runtime path. Strict focused Clippy passed for the tests and the fresh-process
[`bounded_resource_probe`](../imij/examples/bounded_resource_probe.rs) example.

| Tested behavior | Evidence in the integration tests |
|---|---|
| Pixel/sample parity and padded caller output | All six lossless sample types, one/sixteen lanes and grouped stripes |
| Nested resource propagation | Four lossy entropy modes; exact 9 AC + 9 alpha + 3 DC units, cap 21 accepted and 20 denied; alpha preserved; exact-block files need more units than the plain file, accepted at their count and denied one below |
| Physical geometry amplification | 4097×1 Gray8 with one versus sixteen lanes; same output size, different planned working capacity |
| Unit and chunk overhead | 323 one-pixel units; empty ancillary chunks; borrowed preview/frame and ignored plane chunks |
| Metadata accounting | Raw chunks before and after DATA, 520-byte compressed expansion from a small record, gain-map fallback allowance |
| Boundary and session behavior | Exact input/header/output/work/working boundaries; cumulative second-call denials; working plan reset between successful calls |
| Parsing and cancellation | Every truncated prefix of a small bounded file; open-DATA repeat visits; cancellation before admission, during chunk scanning, lossless reconstruction and nested lossy alpha rows |

These are deterministic regressions and parity checks. They are not a
coverage-guided fuzzing campaign, an independent allocation proof or a universal
DoS-resistance qualification. The broader review and remaining validation work
are described in [HOSTILE_INPUTS.md](HOSTILE_INPUTS.md) and
[VALIDATION_PLAN.md](VALIDATION_PLAN.md).

The row-session extension is covered by 13 integration tests, including
simultaneous nested groups, exact admission boundaries, suffix validation and
cancellation within group/alpha delivery. All execute under scalar and SIMD128
WASI as well as native builds. [Combined validation](../experiments/u32_rows/validation.json)
records 249 all-feature passes and strict Clippy. No new native timing or live
allocator-peak qualification is claimed for this extension.

Supported `pSUM` verification charges 34 descriptor bytes plus the full native
raster bytes to cumulative work before reconstruction. Stride padding is
excluded. The fixed hash state is charged to planned capacity. Verification
checks cancellation at intervals of at most 64 KiB of sample bytes, row
boundaries and before success; the byte quantum preserves raw sample bits.
Absent and unsupported tables add no verification work or cancellation checks.
For `Read`, an admitted hash charge does not guarantee that later work charges
will fit: consumed-input work is charged after row delivery, and trailing input
is also validated then. A later `TooLarge` can therefore follow callbacks.
An allowance smaller than the supported hash charge refuses at the table head,
before DATA reconstruction or callbacks.
