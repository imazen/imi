# Persistent hash stability review — 2026-11-17

**Recommendation: retain current XXH3-64/128 identities.** Corrected standard
rapidhash v3 can be specified as a frozen **64-bit** algorithm, but the name
“v3” alone is insufficient. It is not a specified 128-bit replacement. Any
future adoption needs an explicit width decision, immutable algorithm contract,
format signaling and migration review. This review allocates no IDs and changes
no runtime, dependency, hash selection or wire specification.

## Sources and scope

Observed live on **2026-11-17**; conclusions describe these exact sources:

| Source | Inspected identity |
| --- | --- |
| IMI | `a19abcf6996cc9e1de268a08ea58c53eeb790a41`, clean review workspace; [hash abstraction](../imij/src/hash.rs), [V4 object binding](../imij/src/v4/object.rs), [tile framing](../imij/src/v4/tile.rs), [archive format](../imij-set/FORMAT.md). |
| IMI's zenhash dependency | [`ce2bbdf669b90513d30d8dd86ad764ee3de68c6e`][zpin], pinned in both manifests. **Contains no rapidhash APIs.** |
| Current zenhash main | [`85d32a33c5afe687674ff25b1045450ef635bd7a`][zmain], package version still `0.1.0`; [implementation][zr], [features][zc], [API snapshot][za], [stability tests][zs]. |
| Official rapidhash master | [`1ae7842f59f25929a338e5a76e3fa350673e48e1`][up], also zenhash's stated port/reference revision. Its `rapidhash.h` is byte-identical to zenhash's vendored header: `xxh3-128:89d2683118c79f2aec8dad3e29e58e3e` (canonical numeric hex). |

Method: source/history inspection, live release/issue/comment reads, and inspection
of existing independent fixtures and hosted CI. No new build, benchmark,
statistical suite or cross-target execution was run. The private IMI source
links above are relative to this report's source revision. Source clones were
read without modifying their tracked files. Earlier conclusions were not used
as premises.

## What “rapidhash v3” identifies

The upstream family has **standard**, **Micro** and **Nano** functions, each
returning `uint64_t`. In corrected v3, Micro agrees with standard through
80 bytes, Nano through 48; beyond those ranges they are different algorithms,
not interchangeable dispatch tiers. Standard has seven accumulation lanes and
112-byte blocks, Micro five/80, Nano three/48. [Header][up], [README][ur],
[maintainer confirmation][i29].

The following choices must not be collapsed under one persistent identity:

| Choice | Output consequence |
| --- | --- |
| Original versus corrected v3 | The original release snapshot has reversed short-input reads. Correcting them changes outputs at lengths 5–7; see history below. |
| Standard / Micro / Nano | Different long-input outputs, despite shared short outputs. Zenhash implements **standard only**. |
| `RAPIDHASH_FAST` / `RAPIDHASH_PROTECTED` | Protected mode XORs the product halves into the original operands; ordinary mode replaces them. This changes the function, including its mix operations. Protected mode is absent from zenhash. |
| Seed / secret words | Both are algorithm parameters. Changing either generally changes output. Default seed is **0** in current v3, with eight fixed u64 secret words; v1's default was `0xbdd89aa982704029`. |
| `COMPACT` / `UNROLLED` | At the reviewed current header these schedule the same standard recurrence, with different loop unrolling. They are intended to preserve output; equivalence is source reasoning here, not a newly executed macro matrix. V2.2's historical `SUPER_COMPACT` is not a current zenhash option. |
| Integer/struct hashing or preprocessing | Native integer byte order, Rust `Hash`, case folding, random secrets and alternate multiplication formulas do not identify the canonical byte function. |

These distinctions follow the [fixed header][up], [v1 header][v1],
[zenhash implementation][zr] and [Rust portability contract][rh]. A full
64×64→128 multiply is an **internal operation**, not a 128-bit digest. Neither
reviewed upstream nor zenhash exposes rapidhash-128. Concatenating two seeds,
retaining multiply halves, repeating or zero-extending a u64 would define a new
construction; none establishes 128-bit collision strength. The [open 128-bit
request][i26] includes a suggestion, not an adopted specification.

## Version and compatibility history

Dates below are release publication dates where available; individual fixes use
commit dates. Full object IDs are linked so movable tags are not the definition.

| Version/event | Exact source | Compatibility significance |
| --- | --- | --- |
| v1.0, 2024-08-06 | [`588978411df8683777429f729be5213eb1bfd5f3`][v1], [release][rel1] | Original released algorithm; three secrets, different seed/length/loop/finalization rules from v3. |
| v2.0, 2025-05-13 | [`0d48fd50149dbef269ed97d63f24d14c14b57973`][v2], [release][rel2] | Algorithm revision: eight secrets, seed 0 and changed mixing/long-input structure. No v1 output compatibility. |
| v2.1, 2025-05-20 | [`87b34986098720d630ccf6a089f54f0635765a65`][v21], [release][rel21] | Release explicitly says output values changed. Source changes the 49–64-byte mixing secret and path boundaries. |
| v2.2, 2025-05-20 | [`cd62a6b899c96ef32dff14e61452e658acb3a783`][v22], [release][rel22] | Source changes 1–3-byte packing/mixing; adds `SUPER_COMPACT`. A minor-looking algorithm label did not imply output preservation. |
| First v3, 2025-05-27 | [`bbaf1a70775b785f11dab29dc7d9bd717b4eb6a6`][v3old] | Family of three streamable algorithms; not identical to the currently tagged release. |
| Standard short-read fix, 2025-05-27 | [`4a9b33ad58c59416fa65f4aedcba33b413770881`][fixstd] | Swaps `a`/`b` reads for 4–7 bytes in standard. At 4 bytes the reads coincide; lengths 5–7 distinguish original/corrected outputs. This fix is **after** the original v3 snapshot. |
| C portability fixes, 2025-05-28 / 06-02 | [`3f525283`][fixcpp], [`27b743ff`][fix32], [`1370c3f7`][fixmsvc] | C++11 qualifier, fallback-multiply local initialization/declaration and MSVC qualifier repairs. Do not infer a separately measured digest change from the “uninitialized” commit title: its diff splits initialized declarations. |
| Nano / Micro short-read fixes, 2025-05-30 | [`069b4d36`][fixnano], [`04c36610`][fixmicro] | Corresponding read-order fixes at 5–7 bytes. Separate from the earlier **standard** fix. |
| Re-released v3, 2025-08-16 | [`bc4b4baa48a15ff52ff4725e1ccdcda62815221c`][v3now], [release][rel3] | Same tag name now identifies corrected code. Maintainer confirms re-release in [their #33][i33]; [their #43][i43] remains open with release-identity questions. |
| Post-tag through current master | [`7391d624`][loopfix], [`24b4c6dd`][guard], [`e04c9f35`][guardmove], then [current HEAD][up] | Loop setup/unrolling restructuring, include guard and guard placement. Reviewed header diff retains corrected byte loads, constants and finalization; no further digest-changing formula found. No newly executed equivalence proof. |
| Rust port crate v4.0.0 | [release][rust4] | Crate major 4 still implements **algorithm v3**. Release calls out Micro/Nano 5–7-byte changes; it must not be treated as a complete history of standard C v3. |

Upstream's [determinism answer][i14] and [cross-endian discussion][i18] support
same-input portability. They do not establish an immutable meaning for a tag
that was subsequently replaced. In contrast, [xxHash's v0.8.0 release][xxstable]
explicitly freezes XXH3 outputs across future versions and systems for a given
input/parameter set. This is a stability distinction, not a quality ranking.

## Zenhash API and output contract

All rapidhash entry points at [current main][zr] were inspected:

- `rapidhash_v3(bytes)` and `RapidhashV3::{new, default}` use seed 0/default
  secrets; `_with_seed` and `with_seed` accept a u64 seed.
- `rapidhash_v3_with_seed_and_secret` takes `&[u64; 8]`;
  `RapidhashV3::with_seed_and_secret` copies `[u64; 8]`. Secret words are numeric
  values, not native-endian byte strings. Unlike XXH3's variable-length custom
  secret, this interface always has eight words.
- `update` / `Hasher::write` feed raw bytes; `digest` / `Hasher::finish` return
  the same u64 without consuming state. Empty updates are neutral; clones can
  continue independently; `reset` preserves both seed and secrets. `Debug`
  reports the byte count. `io::Write` is available only with `io`; `flush` does
  not finalize/reset. No `digest128`, randomized-secret generator or const
  hashing API exists for rapidhash.

The implementation explicitly promises **every split equals one-shot** and
matching corrected C standard-v3 output. It retains seven lanes, at most 112
unconsumed bytes and the previous block's last 16 bytes for overlapping tails.
The loop consumes only while more than 112 bytes remain. For long input,
finalization mixes the **remaining length**, not total input length; zenhash's
wrapping `total_len` counter is for reporting and does not enter the digest.
Replacing this with total-length mixing would create another algorithm. Repeated
seeded one-shot calls over chunks are not this streaming construction. [Source][zr].

`r32`/`r64` use explicit little-endian slice reads, including unaligned slices;
the multiply uses exact Rust u128 arithmetic. Rapidhash has no ISA dispatch or
target-width approximation. `rapidhash` is opt-in with no runtime dependencies;
`std`, `io`, `avx512` and WASM SIMD affect integration or other algorithms, not
the intended rapidhash byte function. Native, 32-bit, big-endian and WASM byte
hashes therefore have the same specified semantics. Compiler lowering may change
speed, not the contract. `Hash`/typed integer writes remain unsuitable as
persistent serialization. [Common readers][zcommon], [manifest][zc],
[Rust `Hash` portability][rh].

Zenhash pins its reference by commit in source and fixtures, and promises
reference-compatible portable outputs. Its [README][zreadme] still describes a
pre-release API. I found no separate explicit promise that the name
`RapidhashV3` forever freezes one algorithm independently of future upstream
changes. Its existing compatibility promise and tests are useful evidence;
the format must still own the immutable definition.

Updating only the Git revision would also break IMI's present feature selection:
current zenhash requires explicit `xxh3`, while IMI's pinned graph predates that
gate. This is an integration condition, not authorization to update either
manifest. [IMI manifest](../imij/Cargo.toml), [current feature changes][zchange].

## Issue review, including unresolved items

Enumerated the [complete upstream all-state issue/PR collection][iall], read all
open issue bodies/comments and relevant closed discussions. Status is the
observed GitHub status on 2026-11-17; closed
does not itself prove a format guarantee or an attack was eliminated.

| Item | State | Finding relevant to this decision |
| --- | --- | --- |
| [Release clarification, #43][i43] / [re-release, #33][i33] | Open / closed | Tag identity ambiguity is real; source confirms standard as well as Micro/Nano differences. #43 had no comments at observation. |
| [128-bit output, #26][i26] | Open | No adopted 128-bit algorithm in reviewed HEAD. |
| [Streaming API, #39][i39] / [earlier streaming, #3][i3] | Open / closed | “Streamable” does not mean the C header provides an incremental API. Current header has one-shot functions; zenhash supplies bounded partition-invariant state. Discussion includes corrected/mismatched claims, so source/tests take precedence. |
| [32-bit/WASM, #46][i46] | Open | Exact full-width multiply can cost more on WASM. A suggested approximation explicitly changes output; it is not a portable optimization for this format. No speed verdict here. |
| [constexpr compilation, #42][i42] | Open | C++ constant-evaluation compatibility issue; not evidence of a runtime byte-hash mismatch in zenhash, which offers no const rapidhash API. |
| [Integer specialization, #36][i36] | Open | Discussion identifies host-memory integer byte-order pitfalls. Explicit encoding remains necessary. |
| [Case-insensitive extension, #30][i30] | Open | Would change the input function; exclude from canonical byte identities. |
| [Pascal port, #44][i44]; [Go port, #32][i32]; [RNG, #28][i28] | Open | Port/RNG requests do not establish persistent hash compatibility. Their bodies/comments were inspected; no additional stability guarantee found. |
| [Variant differences, #29][i29]; [protected mode, #17][i17] | Closed | Maintainer confirms variants are different functions; protected mode is optional, not the default. |
| [Seed-independent collision, #10][i10]; [multiply-zero collisions, #25][i25] | Closed | Non-cryptographic use remains explicit. V3 length mixing changes the collision construction; discussion explains why fixed secrets plus a seed do not provide authentication/HashDoS protection. |
| [Rust port stable/streaming, #73][p73]; [Hasher stability, #32][p32] | Closed | Separate stable byte-oriented v3/streaming APIs from the evolving in-memory `RapidHasher`. Crate/trait names alone are insufficient. |
| [Rust port i386 failures, #90][p90] | Open | Comments report layout/expected-value test fixes, including intentionally different in-memory 32-bit hashing. Do not misattribute these to zenhash's exact byte-oriented v3. |

Open PRs were also inspected in the collection: [premixed seed #49][pr49],
[integer helpers #37][pr37]/[#38][pr38], [constexpr #45][pr45] and adopter-doc
[#48][pr48]. They are not current master definitions. No upstream posts were
made. [Zenhash's all-state collection][zissues] contained three merged PRs and no open
issues at observation.

## Evidence, limits and what XXH3 replacement changes

Existing zenhash evidence is stronger than only self-comparison:

- [316 individual C vectors][zvectors] from the pinned unmodified header,
  generated by [independent C tooling][zgen], plus a second Rust implementation
  using `RapidSecrets::seed_cpp(seed)` (not its different secret-generation
  convention). [Rapid tests][ztests] compare one-shot, streaming and that oracle.
- Tests cover lengths 0–1024 at offsets 0–7; split positions through
  `min(length, 240)`; selected C lengths through 100,003; deterministic varied
  chunks, prefix digests, clone, reset and empty writes. Expanded [C fixtures][zrefs]
  add structured inputs, custom secrets and correlated seeds. These are bounded
  cases, not exhaustive inputs/partitions or multi-gigabyte qualification.
- [CI run 42755683465][zci] at **exact main `85d32a33`** completed successfully;
  all 51 reported jobs succeeded. Its [workflow][zworkflow] includes native
  x86/ARM Linux, Windows x64/ARM, ARM macOS, i686, big-endian s390x and WASM
  scalar/SIMD128/wide-arithmetic runs. This is inspected hosted evidence,
  not a new execution here. Feature/target coverage is factored, not every
  platform × feature product. [Coverage limits][zs].

The [published SMHasher3 log][sm] labels its algorithm rapidhash v3 64-bit and
reports 250/250 passing. That supports the tested statistical workload, not
output stability, this port's correctness, arbitrary secrets, 128-bit collision
width or cryptographic properties. No speed or statistical superiority was
measured in this review. Both hash families here are non-cryptographic.

For IMI, switching requires **recomputing applicable stored identities**, even
when pixels and payloads are unchanged. Existing [hash IDs](../imij/src/hash.rs)
mean XXH3-64 seed 0/default secret (ID 0, eight little-endian bytes) and XXH3-128
(ID 1, sixteen little-endian bytes). A zero-padded slot does not increase digest
width. Rapidhash64 keeps the 64-bit output width of XXH3-64 but changes its
values; versus XXH3-128 it loses 64 output bits and the existing 128-bit
interoperability. Statistical test passes cannot compensate for that width loss.

Partition invariance, unaligned input, repeated finalization and endian/ISA
equivalence need not be lost with this precise zenhash byte API. Seed/secret
semantics and long-tail finalization do change. Preserve canonical byte framing
and domain separation explicitly; do not substitute seeds for domain prefixes.
IMI storage-order lowercase hex is also different from canonical numeric hex
used in diagnostic receipts; name the representation, not just “hex”.

[V4 section 6.1](../SPEC_V4.md#61-object-and-spatial-tile-integrity) fixes setup, object and tile
hashing to XXH3-128 with 16-byte trailers and separate `IMI/v4/setup\0`,
`IMI/v4/object\0`, `IMI/v4/tile\0` domains. Integrity modes 1/2 are not generic
hash algorithm IDs. A new `HashAlgorithm` value alone would not revise those
semantics: setup contexts, object trailers, tile aggregates, all writers/readers
and signaling require a deliberate compatible extension or version decision.
The existing unknown-mode refusal must remain.

Archive footer algorithms govern page/blob identities; legacy generations and
sequence-index bindings retain their own IDs. A migration must recompute every
affected page/root/footer/blob/index binding, retain readers for old IDs, and
handle mixed-generation references. Existing compaction deliberately preserves
algorithms needed by bindings. Do not relabel stored XXH3 bytes, reuse IDs 0/1,
or claim a seamless replacement. [Archive format](../imij-set/FORMAT.md),
[sequence bindings](../imij-set/src/sequence.rs).

## Conditions for any future frozen variant

The technically specifiable candidate is **corrected standard rapidhash v3,
unprotected, upstream `1ae7842f59f25929a338e5a76e3fa350673e48e1`, seed 0,
the header's exact eight default secret words, full 64×64→128 multiplication,
little-endian loads and eight little-endian digest bytes**. This descriptive
name is not a proposed wire ID. Freeze its algorithm, not a package major or tag.

Before adopting it for persistent data, the format owner would need to:

1. Approve the 64-bit width for each intended use, or commission and separately
   qualify a precisely specified 128-bit construction. There is no existing
   rapidhash128 choice established by this review.
2. Normatively freeze seed initialization, all constants, short-read order,
   block/tail boundaries, length mixing, finalization, input limits, digest byte
   order, domain/framing bytes and truncation rules. Specify that all chunk
   schedules, empty updates, snapshots and reset obey the byte-function contract.
   Future implementation bug fixes must restore that contract; changed outputs
   need another algorithm identity.
3. Retain independent **individual** golden input/output pairs, not only aggregate
   fingerprints or values generated by the implementation under test. Include
   original/corrected 5–7-byte witnesses; 0/1/3/4/8/16/17, 48/80/112/224 edges
   and neighbors; every remainder, unaligned slices, overlaps, structured and
   long inputs; fixed/nonzero seeds and secrets if supported. Add a bounded
   >4-GiB incremental witness if that input range is promised. Freeze generators
   and the licensed C oracle; use a second independently maintained port only
   after resolving its exact algorithm/seed convention.
4. Gate byte and serialized-digest equality across native x86/ARM, 32-bit,
   big-endian, WASM scalar/SIMD/wide arithmetic, supported compiler versions and
   feature graphs. Include prefix digest/continue, clone, reset and mixed chunk
   schedules. Existing zenhash CI/fixtures are a substantial starting point;
   add IMI domain/framing and cross-version fixtures rather than assuming them.
5. Design signaling, legacy-reader behavior, archive/reference migration and
   format-level conformance first. Preserve existing XXH3 fixtures/sealed data.
   Qualify any performance motivation separately at the actual consumer sizes;
   no such win is established here.

## Small reviewer packet

- **Decision now:** retain current. A frozen corrected standard-v3 **64-bit**
  option is technically possible after the conditions above; adoption as a
  128-bit substitute is unsupported.
- **Three decisive facts:** original v3 was re-tagged after digest changes;
  zenhash ports the corrected fixed header with real partition-invariant state;
  no reviewed rapidhash128 construction exists.
- **Read first:** [standard read-order fix][fixstd], [release ambiguity][i43],
  [zenhash's whole/streaming source][zr], [independent fixtures][zrefs],
  [V4 object domains](../imij/src/v4/object.rs).
- **Missing evidence:** an immutable format-owned rapidhash contract, an approved
  width/signaling/migration design, format-specific independent vectors,
  multi-gigabyte qualification if promised, and any consumer performance benefit.
  Existing hosted portability tests were verified; new runtime tests were not run.

[zpin]: https://github.com/imazen/zenhash/tree/ce2bbdf669b90513d30d8dd86ad764ee3de68c6e
[zmain]: https://github.com/imazen/zenhash/tree/85d32a33c5afe687674ff25b1045450ef635bd7a
[zr]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/src/rapidhash.rs
[zc]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/Cargo.toml
[za]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/docs/public-api/zenhash.features.txt
[zs]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/docs/stability.md
[zcommon]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/src/common.rs
[zreadme]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/README.md
[zchange]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/CHANGELOG.md
[zvectors]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/tests/reference/rapidhash-v3.txt
[zgen]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/tools/rapidhash-reference.c
[ztests]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/tests/it/rapid.rs
[zrefs]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/tests/reference/README.md
[zci]: https://github.com/imazen/zenhash/actions/runs/42755683465
[zworkflow]: https://github.com/imazen/zenhash/blob/85d32a33c5afe687674ff25b1045450ef635bd7a/.github/workflows/ci.yml
[zissues]: https://api.github.com/repos/imazen/zenhash/issues?state=all&per_page=100
[up]: https://github.com/Nicoshev/rapidhash/blob/1ae7842f59f25929a338e5a76e3fa350673e48e1/rapidhash.h
[ur]: https://github.com/Nicoshev/rapidhash/blob/1ae7842f59f25929a338e5a76e3fa350673e48e1/README.md
[v1]: https://github.com/Nicoshev/rapidhash/blob/588978411df8683777429f729be5213eb1bfd5f3/rapidhash.h
[v2]: https://github.com/Nicoshev/rapidhash/blob/0d48fd50149dbef269ed97d63f24d14c14b57973/rapidhash.h
[v21]: https://github.com/Nicoshev/rapidhash/blob/87b34986098720d630ccf6a089f54f0635765a65/rapidhash.h
[v22]: https://github.com/Nicoshev/rapidhash/blob/cd62a6b899c96ef32dff14e61452e658acb3a783/rapidhash.h
[v3old]: https://github.com/Nicoshev/rapidhash/blob/bbaf1a70775b785f11dab29dc7d9bd717b4eb6a6/rapidhash.h
[v3now]: https://github.com/Nicoshev/rapidhash/blob/bc4b4baa48a15ff52ff4725e1ccdcda62815221c/rapidhash.h
[fixstd]: https://github.com/Nicoshev/rapidhash/commit/4a9b33ad58c59416fa65f4aedcba33b413770881
[fixnano]: https://github.com/Nicoshev/rapidhash/commit/069b4d36b1e925ebc970de61ecdeccfc907a42a2
[fixmicro]: https://github.com/Nicoshev/rapidhash/commit/04c36610e4cba44a26f6b30c9f1605d81dbeef29
[fixcpp]: https://github.com/Nicoshev/rapidhash/commit/3f525283155155a590936d0d0497255164fdf73e
[fix32]: https://github.com/Nicoshev/rapidhash/commit/27b743ffb5087f7ab1c6c70d87ce287770c4370d
[fixmsvc]: https://github.com/Nicoshev/rapidhash/commit/1370c3f7ac148afd9c460b37a8bb75ac569f6d08
[loopfix]: https://github.com/Nicoshev/rapidhash/commit/7391d6249338c0a7b4096fe125140db79c5da988
[guard]: https://github.com/Nicoshev/rapidhash/commit/24b4c6dd33b6129e766cd0ca8330465967162f71
[guardmove]: https://github.com/Nicoshev/rapidhash/commit/e04c9f35fa5a11c8c11de0f7cc1bdad38978d429
[rel1]: https://github.com/Nicoshev/rapidhash/releases/tag/rapidhash_v1.0
[rel2]: https://github.com/Nicoshev/rapidhash/releases/tag/rapidhash_v2.0
[rel21]: https://github.com/Nicoshev/rapidhash/releases/tag/rapidhash_v2.1
[rel22]: https://github.com/Nicoshev/rapidhash/releases/tag/rapidhash_v2.2
[rel3]: https://github.com/Nicoshev/rapidhash/releases/tag/rapidhash_v3
[rust4]: https://github.com/hoxxep/rapidhash/releases/tag/v4.0.0
[iall]: https://api.github.com/repos/Nicoshev/rapidhash/issues?state=all&per_page=100
[i3]: https://github.com/Nicoshev/rapidhash/issues/3
[i10]: https://github.com/Nicoshev/rapidhash/issues/10
[i14]: https://github.com/Nicoshev/rapidhash/issues/14#issuecomment-2619332290
[i17]: https://github.com/Nicoshev/rapidhash/issues/17
[i18]: https://github.com/Nicoshev/rapidhash/issues/18
[i25]: https://github.com/Nicoshev/rapidhash/issues/25
[i26]: https://github.com/Nicoshev/rapidhash/issues/26
[i28]: https://github.com/Nicoshev/rapidhash/issues/28
[i29]: https://github.com/Nicoshev/rapidhash/issues/29
[i30]: https://github.com/Nicoshev/rapidhash/issues/30
[i32]: https://github.com/Nicoshev/rapidhash/issues/32
[i33]: https://github.com/Nicoshev/rapidhash/issues/33
[i36]: https://github.com/Nicoshev/rapidhash/issues/36
[i39]: https://github.com/Nicoshev/rapidhash/issues/39
[i42]: https://github.com/Nicoshev/rapidhash/issues/42
[i43]: https://github.com/Nicoshev/rapidhash/issues/43
[i44]: https://github.com/Nicoshev/rapidhash/issues/44
[i46]: https://github.com/Nicoshev/rapidhash/issues/46
[pr37]: https://github.com/Nicoshev/rapidhash/pull/37
[pr38]: https://github.com/Nicoshev/rapidhash/pull/38
[pr45]: https://github.com/Nicoshev/rapidhash/pull/45
[pr48]: https://github.com/Nicoshev/rapidhash/pull/48
[pr49]: https://github.com/Nicoshev/rapidhash/pull/49
[p32]: https://github.com/hoxxep/rapidhash/issues/32
[p73]: https://github.com/hoxxep/rapidhash/issues/73
[p90]: https://github.com/hoxxep/rapidhash/issues/90
[xxstable]: https://github.com/Cyan4973/xxHash/releases/tag/v0.8.0
[rh]: https://doc.rust-lang.org/std/hash/trait.Hash.html#portability
[sm]: https://gitlab.com/fwojcik/smhasher3/-/raw/main/results/raw/rapidhash.txt
