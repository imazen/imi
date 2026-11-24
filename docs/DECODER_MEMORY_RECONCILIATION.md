# Decoder memory estimate reconciliation

2026-10-04. Source-only audit of **pre-correction `1d72a3bd`**, followed by the
small LUT-admission correction described below. No builds, codec/corpus runs,
heap measurements or native timing were performed for this audit.

**An encoder-targeted 32 MiB file can fail 32 MiB session admission.** One cause
was a concrete excess LUT charge, now corrected by the root implementation.
Metadata/chunk allowances and open-DATA repeated working plans remain separate
causes. Equal numbers in the two APIs do not establish equivalent budget scopes.

## Reproduce the arithmetic, with provenance

[`planner_audit.py`](../experiments/decode_resources/planner_audit.py) mirrors
reviewed estimator arithmetic, not the allocator. It pins 21 relevant function
bodies and checks the used constants. It records full hashes of 12 source files,
all assumptions, grid counts, estimate categories and examples. The mirrored
formula output is **not allocation proof**, not an actual encoder-produced file,
and not a universal upper-bound qualification.

The current corrected checkout intentionally fails its pre-correction formula
guard. Reproduce against the immutable commit, using scratch only:

```sh
mkdir -p local-scratch/zqi-resource-planner-20261004/reproduce
git archive --format=tar --output=local-scratch/zqi-resource-planner-20261004/source.tar 1d72a3bd imij/src
tar -xf local-scratch/zqi-resource-planner-20261004/source.tar -C local-scratch/zqi-resource-planner-20261004/reproduce
python3 experiments/decode_resources/planner_audit.py --source-root local-scratch/zqi-resource-planner-20261004/reproduce --output local-scratch/zqi-resource-planner-20261004/audit.json
```

The local receipt is `local-scratch/zqi-resource-planner-20261004/audit.json`:
SHA-256 `eb1bb1aeeede2850fc4dccd00e1dcda41182b9050021fae907b9ea103959feb5`.
Script SHA-256 `1c0533f101bd428934a6945fadf094d5c6673af78ed2a5ae434840f826d30625`.
Receipt source identities:

| File | SHA-256 |
| --- | --- |
| decoder_memory.rs | `6d040128be66b6e734099823623b37286ced09d753efb496c46dff7a663505fd` |
| decode_resources.rs | `0ccfe8c9b03935208b6307136d99f3456e877c49ab6c83aa8d18edd7c43790d5` |
| v1.rs | `cff2c0dd798dc89ecefb06411bea77a285b2523d1805973a242d6fbaf2c1156a` |

The fixed lossless grid considers 5,940 legal explicit shape/configuration
combinations. Of 4,103 encoder-fit cases, none crossed the 32 MiB session boundary;
the grid missed the narrow threshold region. Separate monotonic binary searches
find the boundary examples below. The 2,160-case lossy grid includes 1,320 encoder
fits and zero encoder-fit/session-denied cases under its explicitly recorded DC/
alpha map assumptions. This does not imply no lossy mismatch exists. No encoded
models, data-dependent palette choices, actual Vec capacities or target ABI sizes
were observed. The script compares one worker and default channel layouts;
maps and nested heads are assumptions, not inferred from a pixel corpus.

## Concrete 32 MiB examples

32 MiB = 33,554,432 bytes. The following are arithmetic examples with positive
legal dimensions, explicit no groups, 16 lanes, no requested palette, one worker,
and no metadata unless specified. Stripe height is 16, clamped to image height.
Columns are tightly packed logical output; these examples fit default output
dimensions/bytes. Compressed size and successful encoding remain unmeasured.

| Shape/condition | Encoder estimate | Pre-fix session plan | Same plan with actual LUT eligibility |
| --- | ---: | ---: | ---: |
| Gray U16 1,516,224 × 1, Lsb16 | 33,554,148 | 33,684,772 | 33,422,628 |
| RGB U16 532,992 × 1, all Lsb16 | 33,554,020 | 33,946,788 | 33,160,356 |
| RGBA U16 401,088 × 1, all Lsb16/nonconstant alpha | 33,553,380 | 34,077,220 | 33,028,644 |
| RGB U16 300,000 × 1, all Lsb16, open DATA | 19,108,516 | 38,986,056 | 37,413,192 |
| RGBA U8 260,000 × 1, all Lsb8, 16 MiB retained metadata | 21,461,924 | 38,240,868 | 38,236,772 |
| RGBA U8 350,000 × 1, all Lsb8, 65,536 borrowed-chunk descriptors including DATA | 28,841,924 | 37,232,004 | 37,227,908 |

These are possible head/layout assumptions. For the identity examples,
channel values including 1 and 65,534 force full-depth U16 mapping in
`mapping::detect`; disabling palette explicitly avoids a data-dependent palette
flag. This describes a constructible input, without claiming the wide file was
encoded or decoded here. The first configuration already fits the planner, so
its geometry search would preserve those requested settings.

Ordinary encoders emit bounded DATA. The open-DATA example requires legally
reframing its bounded primary DATA as zero-length/open DATA; it is not ordinary
encoder output. Metadata can be a retained opaque ICC chunk at its allowance,
without asserting ICC semantic validity. The chunk example can use repeated
valid preview-file payloads: the primary slice decoder borrows them and does
not reconstruct their images. There is no metadata-byte charge for pRVW, but
the descriptor plan charges every chunk. Other input/chunk/work policy caps
must also permit the request; a working byte budget never bypasses them.

## Actual LUT mismatch and its minimal correction

Pre-fix session `Admission::lossless` charged `4 * 2^bits` for **every** map with
bits ≤16. Actual `OutMap::new` excludes:

* Const, Shift and Offset: scalar/SIMD arithmetic, no expansion LUT.
* `map.is_identity(depth)`: every Lsb map, even narrow Lsb; Replicate/Scale at
  full depth; zero-base Offset (already excluded).
* Coded widths above 16.

F16InF32(16) and UnormF32(1..16) at container depth32 **do allocate** a LUT.
Nonidentity Replicate/Scale below full container depth also allocate. The SIMD
writer being able to use a short arithmetic transform does not remove a LUT
that `OutMap::new` still constructs. Count constructor ownership, not kernel
usefulness. Constant-alpha maps remain in layout.maps but own no LUT.

Encoder `lossless_tables` reserves depth−1 expansion width at depth≤16, since
full-depth maps are identity; for depth32 it reserves 16-bit expansion. That
part is compatible with the actual constructor. For RGB16 identity admission
charged 786,432 phantom LUT bytes while encoder planning reserved a conservative
393,216-byte expansion ceiling; actual LUT payload is zero. At the threshold,
session minus encoder is 392,768 bytes: phantom-LUT difference plus fixed/chunk
allowances minus the encoder's small scheduler reserve.

The controller has implemented `v1::OutMap::lut_entries(map,depth)`, reused in both actual
construction and admission, preserving normative output arithmetic. The controller reports
16 resource tests and three OutMap tests passing, including actual retained Vec
capacity for identity/algebraic and nonlinear F32 maps, plus strict workspace
Clippy. The closed small Gray16 under-128-KiB regression is executed evidence;
the wide 32-MiB table above remains analytical. This audit did not rerun those
tests. Original V4 native/heap evidence predates this correction and is not
claimed to measure it.

This is the smallest concrete fix: share the real eligibility predicate before
extracting larger planners. A bytes helper should derive its element size from
u32. Do not change mapping, create/deallocate new LUTs, change the bitstream,
or shrink fixed AC tables as part of admission reconciliation.

## Other differences: scope versus live allocation

| Category | Actual path and estimates | Classification |
| --- | --- | --- |
| Parser/model fixed space | Encoder lossless tables include 32 KiB; session adds 16 KiB header and 16 KiB per parsed model head. Lossy table helper additionally includes its own header/model allowances. These reservations are not exact heap object sizes. | Conservative categories overlap; factoring must preserve or qualify allowances, not blindly sum all helpers. |
| Mapping/storage | Encoder uses source channel count/depth; session uses coded slots and narrow u16 storage when every slot fits16. Constant alpha eliminates a slot. | Deliberate encoder upper bounds, apart from the corrected eligibility mismatch. |
| Lossless references | Actual still-session stripes reject inter predictors and allocate no RefRows. Both estimates reserve three u32 planes per coded slot anyway. | Conservative still-image allowance; large near-budget plans can greatly exceed observed live state. Do not remove without inter/stream scope review. |
| Concurrent group/worker state | Serial still decode traverses one group at a time. Stream retains all current groups; native threaded driver can own multiple states. Encoder reserves max(all groups, workers plus scheduling); session owns one state. | Deliberate different execution scope. Encoder cannot guarantee memory when actual caller concurrency exceeds its target. |
| Scheduling/descriptors | Final ordinary serial lossless driver no longer allocates row scheduling lists; session does not charge them. Encoder keeps scheduling for threaded qualification. Lossy still constructs row lists/items even bounded serial, but bypasses parallel slots/results. | Deliberate/conservative differences, not an omitted serial lossless allocation. |
| Metadata/chunks | Session charges expanded/copied metadata, doubled possible gain-map fallback and128 bytes per chunk. Encoder excludes metadata. Borrowed preview/frame/plane input is not copied here. | Intentional extra request scope; same byte value can reject even after LUT correction. |
| Open DATA | Payload-length parser constructs models/descriptors and drops them before final preparation parses again. Session repeats complete model/table/state plans; work/units also repeat. Pixel state is not created by that first parsing visit. | Working sums nonoverlapping planned owners; conservative false rejection. Both parsing visits remain real cumulative work. Separate lifetime-accounting follow-up. |
| Input/output | Encoder excludes them; session input/output have separate limits, and output caller padding is outside logical output bytes. | Separate resources; never infer process heap from the codec plan alone. |
| I/O/frame/index APIs | Record buffers, retained directory/frame metadata and previous canvas exist beyond ordinary slice scope. | Separate coverage; do not import their buffers into a slice codec comparison or claim session protection. |

The working accumulator monotonically adds within a call. Its advertised peak is
the peak conservative plan, not an allocator-observed simultaneous capacity.
Open-DATA repeated sums are not a decoder memory corruption/overflow finding.
Do not simply subtract model or unit counters: repeated parsing work is real,
and retained metadata/descriptors have different lifetimes. A later phase plan
must retain prior live owners, observe a temporary parse peak, drop temporary
owners, then admit final state. Failures/cancellation preserve charged work and
recorded peak. No phase-counter implementation is proposed in this patch.

## Allocation-site audit: what is covered and what still needs observation

**Lossless:** `parse_head_checked` allocates mapping/layout vectors, DecModels,
palette model and model-construction temporaries. Each DecModel owns a16-KiB
table and up to256 token triples; each current Model has four frequency/weight
vectors. `OutMap` owns optional expansion LUTs and small transform/map/slot lists.
`StripeState::new_with` owns four row planes per coded slot plus an extra slot0
row, per-row predictors, Words/LaneStates and outer Vecs. Palette owns expanded
entry planes, 256*bpp output table and group-width scratch; palette stripe normal
prediction uses one index slot. RefRows adds three u32 slot planes only on inter
paths. The formula includes both palette and ordinary row allowances even when
not simultaneous. Unit record descriptors are borrowed slices, not copies.

Actual lossless row payload for an ordinary nonreference/nonpalette narrow
stripe is `physical_width * (4*slots+1) * 2`, distinct from the estimate that
adds `physical_width *12*slots` reference reserve. Descriptor cap32*units and
8-KiB state/model fixed reservations are conservative allowances for vector
growth/small owners; they are not precise ABI sizeof accounting.

**Lossy:** `parse_lossy_head_checked` owns transfer LUT, fixed maximum rANS or
Huffman tables, coefficient frequency/weight models and nested DC/alpha heads.
`decoder_table_bytes()` deliberately uses the larger fixed rANS/Huffman table
allowance. Lower `max_ac_tables` does not shrink that allocation. Only one
transfer table exists per parsed image, shared across workers.

Serial bounded decode owns global `unit_rows`/work-item descriptor lists and
one reusable Scratch (std pool retains it between items, not between bounded
decode calls; no_std recreates per item). Scratch retains q/q2 and EOB/EOB2,
DC/AQ pair buffers, raw DC pixels, planes/linear row, RGB temporary and alpha.
Two DC value buffers can coexist with raw conversion input and `dc_values`'
temporary raw i32 Vec. Nested DC StripeState is live during raw DC decode;
alpha StripeState is live during later alpha decode. Conservatively summing
both exceeds some actual phases, but avoids an unqualified lifetime shortcut.

Per-block requested q+EOB payload is `3*64*4 +3` bytes per stripe, plus spare
slots; paired stripes double it. SplitHuf additionally owns wid up to48 bytes
per block and local Huffman scratch: luma n, chroma2n, table ids/flat each up to
48n plus16 spare bytes. Counts can attain16 per channel on legal heads. These
temporaries overlap the retained coefficient buffers, including the earlier
paired stripe. The current coefficient allowance separately doubles payload
growth and reserves320 bytes per block for side information, so no definite
side-info omission follows from the source. Capacity/reallocation overlap still
needs instrumentation, not an assumed universal 2x Vec guarantee.

Reconstruction uses at most32 blocks per chunk, padded plane rows, optional
linear color row and a `coded_runs` Vec covering the group. `decoder_recon_bytes`
covers chunk planes/linear growth; the additional32*group-block allowance can
cover run descriptors (16 bytes per pair on64-bit, before growth). Nondefault
subblock/sampling paths explicitly reserve16 KiB per worker of planned stack.
No new transform-sized heap owner was found in those paths.

**No definite uncharged live codec buffer was established** on the final serial
paths by this read. Several owners are covered by coarse overlapping reserves,
not direct byte identities. Thus this is neither proof all capacities fit nor
evidence a measured process would exceed the target. Largest checks still needed:

1. Model construction peak while prior models remain, actual OutMap Vec capacity
   at maximum eligible widths, and record/row/item vector growth on many tiny
   stripes/groups. Compiler/ABI-dependent small objects need a64-bit and32-bit
   check; encoder intentionally retains portable64-bit descriptor allowances.
2. Paired coefficient growth plus maximum SplitHuf side information, both retained
   DC/AQ buffers plus raw conversion, and state/staging across alternating group
   sizes. Legal external DC stripe height can exceed encoder-standard grouping;
   admission uses actual nested hs, so do not substitute the encoder's usual
   `sb * max(1,8/sb)` rule for arbitrary input.
3. Actual simultaneous owners versus cumulative conservative additions for open
   DATA and metadata before/after it; gain-map fallback and compressed expansion.
   Test both retained capacity and transient old/new reallocation overlap.

## Small follow-up after the landed predicate

Extract common **state arithmetic** into private helpers only after matching
categories: lossless physical padding/row/palette/reference state; paired lossy
coefficients/side-info; transfer table; nested DC conversion/staging. Pass actual
slots/maps/parsed nested geometry for admission and conservative maximum inputs
for the encoder. Keep codec shared, per-worker/group state, scheduler/descriptors
and parse/metadata additions named. Preserve existing encoder estimates/search
order initially; any changed bound/search is an explicit behavior change.

Meaningful gates are actual retained-capacity/live-owner traces with real decode
parity, budgets around observed table/state thresholds, bounded malformed heads
before allocations, and matching encoder/session codec-category checks. Do not
write tests that merely compare these mirrored Python constants to copied Rust
constants. Row-stream admission must add simultaneous nested group/full-width
staging ownership as [the row coverage design](RESOURCE_COVERAGE_NEXT.md) describes.
Keep open-DATA lifetime accounting and broader reader/frame/ROI enforcement as
separate changes; preserve cumulative work and API compatibility throughout.
