# Branch and open-issue triage — 2026-10-27

Private audit; recommendations only. No source edits, builds, branch movement/deletion,
merges, issue mutations or push were performed. The only tracked change is this audit.

## Decisions and uncertainty first

- **Do not bulk-delete ahead branches.** All 41 non-`archive/` names were inventoried:
  30 local bookmarks, 26 origin bookmarks, 39 distinct targets including the stale
  `main@git` target. Counts are name unions, not sums. There are four open issues,
  all read with their comments, and no open PRs in the resolved PR inventory.
- **Nested-image decoder work needs qualification, not blind integration.**
  `dcdec-fixed-followup-source` contains preserved controlled-study work and reports
  acceptance pending. Its current-source correctness/performance gates were not run.
  Preserve the archived study and protect the workspace-related original; do not
  confuse rebased schedule variants with independent improvements.
- **Preserve failed and unadopted experiments.** `cheapctx`, weighted contexts,
  count grammar, exact-unit option 1 and the original superblock lab contain
  historical implementation/evidence absent from main. Archiving is appropriate;
  merging their obsolete grammars is not recommended by this review.
- **No unconditional issue closure is supported.** #73 remains actionable;
  #64 needs a decision about a new inline-copy kernel; #13 and #14 are partly done
  with concrete remaining contracts. The old #14 “close after PR71/72” comment
  predates a later source-based integration update.
- **Default branch, workspace tips/parents remain protected.** In particular,
  `dcdec-reviewed-original`, `experiments/superblocks` and
  `review/smallxf-integration-20261012` are immediate workspace parents. Do not
  delete their bookmarks as a consequence of this audit. No workspace cleanup is
  authorized here. There are 16 workspaces in the snapshot, including this one.

## Snapshot and method

`jj git fetch` reported no change. Resolved `main@origin` and local `main`:
`78e8a6e11a2a53133903a13dc89488b1c818b659` (merged PR81).
The audit workspace was a clean empty child at `39228da6` when claimed.
`main@git` was `98218a02ca4994f2e0e03e3d71c035c427b7cd48`, seven commits behind;
it is an export observation, not the comparison base. No export was requested.
The Git checkout's HEAD is a different workspace and was not used as main.

Inventory used `jj bookmark list --all-remotes` with explicit target IDs, and
`jj workspace list` plus `working_copies() | parents(working_copies())`.
Read-only Git used the shared repository Git directory, never a workspace `.git`.
For each distinct target: `git rev-list --left-right --count MAIN...TIP`,
`git cherry MAIN TIP`, commit subjects, and blob comparisons against main for
paths changed from that target's merge base. Origin archive ancestry was checked
with `git merge-base --is-ancestor TIP ARCHIVED_TIP`; exact archive targets are
separated from descendant coverage below. `gh issue list --state open --limit 200`
returned four issues, including bodies and all 17 returned comments;
`gh pr list --state all --limit 200` returned the PR states used here. PR1, PR2 and
PR69 closing comments were fetched separately. No benchmark claims were rerun.

`git cherry` +/- counts mean unmatched/equivalent **non-merge patches**, not
useful/useless work; merge commits and empty history can make totals differ from
ancestry-ahead counts. File equality is supporting evidence only: changed files
may have evolved after adoption. The disconnected `gh-pages` history has no merge
base and is treated as historical deployment data, not a source candidate.

## Complete branch inventory

L = jj local bookmark; R = origin; G = exported Git bookmark when different.
Absent L/R is `—`. Hashes below are unambiguous prefixes; full IDs follow.
A/B = ancestry ahead/behind resolved main; +/- = `git cherry` unmatched/equivalent.
When L/R differ, both metrics appear as `L: ...; R: ...`.
Evidence keys refer to the substantive assessments following the table.

| Branch | L | R | A/B; +/- | Related PR | Recommendation / evidence / confidence |
|---|---|---|---|---|---|
| `csum-checksum` | `7c28b06b790f` | `b8ce06434101` | L: 5/1620; 5/0; R: 4/1643; 4/0 | [#1](https://github.com/imazen/zqi/pull/1) closed | Archive L and R before retiring names; C1; high |
| `csum-checksum-v2` | `5c724324b9c9` | `7f80ff75ca60` | L: 7/1620; 7/0; R: 7/1641; 7/0 | [#2](https://github.com/imazen/zqi/pull/2) closed | L already archived; archive R before retiring names; C1; high |
| `dcdec-fixed-followup-source` | `25b3afc22ab7` | — | 15/1016; 15/0 | — (no matching head) | Keep archive; retire alias only after ownership check; D1; high |
| `dcdec-fixed-measurement9c12` | `9c12de8fb75b` | — | 8/1016; 8/0 | — (no matching head) | Covered by archived descendant; retire alias after ownership check; D1; high |
| `dcdec-original-f43-source` | `f43a22007049` | — | 2/1064; 2/0 | — (no matching head) | Exact archive exists; retire alias after ownership check; D1; high |
| `dcdec-original-review-48e` | `48e593c2c245` | — | 3/1064; 2/1 | — (no matching head) | Archive empty wrapper before retiring alias; D1; high |
| `dcdec-reviewed-original` | `5efb37cf9467` | — | 2/1014; 2/0 | — (no matching head) | KEEP: protected workspace parent; D1; high |
| `exact-blocks-lab` | `5c03a48c103d` | `5c03a48c103d` | 28/1621; 27/1 | — (no matching head) | Archive historical prototype before retiring name; E1; high |
| `experiments/cheapctx` | `d9070780c0b0` | `d9070780c0b0` | 23/1073; 23/0 | — (no matching head) | Archive failed experiment; do not integrate; E2; high |
| `experiments/dct16-merged` | `c954e75fe101` | `c954e75fe101` | 7/1582; 7/0 | — (no matching head) | Preserve via superblocks descendant; archive alias when unowned; E3; high |
| `experiments/exactunit-opt1` | `050fdd891cf8` | `050fdd891cf8` | 4/1085; 3/1 | — (no matching head) | Archive rejected option; do not integrate; E4; high |
| `experiments/interslice` | `0032f635a579` | `0032f635a579` | 4/1095; 4/0 | — (no matching head) | Archived descendant exists; retain study, no blind integration; E5; medium |
| `experiments/nz-count-grammar` | `84d20b9694b9` | `84d20b9694b9` | 30/1589; 27/1 | — (no matching head) | Archive failed experiment; do not integrate; E6; high |
| `experiments/nz-count-grammar-devin-wip` | `5ddb1d3ea1be` | `5ddb1d3ea1be` | 27/1589; 24/1 | — (no matching head) | Archive separately: WIP is not ancestor of final study; E6; high |
| `experiments/superblock-final2` | — | `51f400e615c5` | 39/1144; 1/38 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `experiments/superblock-rebased` | — | `e9f30a22702b` | 30/1187; 9/21 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `experiments/superblock-review` | — | `64ef19df2e1e` | 46/1143; 0/46 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `experiments/superblock-sbfinal` | — | `63b456790241` | 25/1200; 6/19 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `experiments/superblock-sbport` | — | `8d3827a6e2ea` | 16/1200; 4/12 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `experiments/superblocks` | `601dd68b7cfb` | `601dd68b7cfb` | 33/1443; 32/0 | — (no matching head) | KEEP: protected workspace parent; E3; high |
| `experiments/wpctx` | `ef7fe06a4f67` | `ef7fe06a4f67` | 16/1077; 16/0 | — (no matching head) | Archive failed experiment; do not integrate; E2; high |
| `external-base` | `f1ab140de17c` | `f1ab140de17c` | 1/231; 1/0 | [#69](https://github.com/imazen/zqi/pull/69) closed | KEEP as explicitly deferred PR69 reference; E7; high |
| `final2` | — | `51f400e615c5` | 39/1144; 1/38 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `gh-pages` | `ebe8dc07b9ea` | `ebe8dc07b9ea` | 40/1887; 37/0 | — (no matching head) | KEEP historical deployments until independent retention review; W1; high |
| `imi-encode-feature` | `f54a2aceb11e` | `f54a2aceb11e` | 0/1; 0/0 | [#81](https://github.com/imazen/zqi/pull/81) merged | Delete candidate: merged, zero ahead; M1; high |
| `main` | `78e8a6e11a2a` | `78e8a6e11a2a` | 0/0; 0/0; G: 0/7; 0/0 | — (no matching head) | KEEP default; stale G is not remote main; M1; high |
| `preserve-pages-csum-composition-20261008` | `80387691dfa7` | — | 2/1577; 1/0 | — (no matching head) | Exact archive exists; retire alias after ownership check; P1; high |
| `preserve-profile-samples-before-capability-fix-20261008` | `e832a1864594` | — | 1/1570; 0/0 | — (no matching head) | Archived descendant exists; retire alias after ownership check; P1; high |
| `preserve-profile-samples-reviewed-20261008` | `2ed66d7b5ab1` | — | 2/1570; 1/0 | — (no matching head) | Exact archive exists; retire alias after ownership check; P1; high |
| `preserve-rdoq-original-20261008` | `d8c53e9b9789` | — | 1/1603; 1/0 | — (no matching head) | Exact archive exists; retire alias after ownership check; P1; high |
| `rebased` | — | `e9f30a22702b` | 30/1187; 9/21 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `review/dcdec-runtime-only-p1-20261012` | `0a2df8c38773` | — | 1/1016; 1/0 | — (no matching head) | Archived descendant exists; retain pending requalification; D1; medium |
| `review/dcdec-runtime-only-p2-20261012` | `e2a3992d2ae5` | — | 2/1016; 2/0 | — (no matching head) | Exact archive exists; retain pending requalification; D1; medium |
| `review/smallxf-before-main-rebase` | `d2ee51107e0d` | — | 40/1064; 10/30 | — (no matching head) | Exact archive exists; retire alias after ownership check; S1; high |
| `review/smallxf-integration-20261012` | `3ee9071076ca` | — | 1/965; 1/0 | — (no matching head) | KEEP: protected workspace parent; S1; high |
| `sbfinal` | — | `63b456790241` | 25/1200; 6/19 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `sbport` | — | `8d3827a6e2ea` | 16/1200; 4/12 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `sbport-review` | — | `64ef19df2e1e` | 46/1143; 0/46 | — (no matching head) | Archive full port chain once, then retire redundant alias; S2; high |
| `science-qscale-evidence-20261008` | `816e6fffb7ac` | — | 5/1597; 2/1 | — (no matching head) | Exact archive exists; retain evidence, retire alias when unowned; P1; high |
| `science-qscale-preserved-20261008` | `55d67b375019` | — | 3/1600; 2/0 | — (no matching head) | Archived descendant exists; retain evidence, retire alias when unowned; P1; high |
| `superblocks` | — | `81cf3e977af5` | 47/1143; 1/46 | — (no matching head) | Archive unique WIP overlay; do not integrate; S2; high |

## Substantive branch assessments

**C1 — superseded checksum designs.** PR1 closed in favor of PR2; PR2 closed in
favor of merged [PR4](https://github.com/imazen/zqi/pull/4), piecewise checksums
with optional verification. Their closing comments explicitly left branches in
place ([PR1 comment](https://github.com/imazen/zqi/pull/1#issuecomment-6044808242),
[PR2 comment](https://github.com/imazen/zqi/pull/2#issuecomment-6049364469)).
Do not transplant the obsolete whole-file grammar. Local checksum tips are
contained in the existing checksum archive; origin tips are different rebased
histories without an exact or descendant archive in this inventory. Both local
and remote v2 histories have 14 study paths absent from main, including
`benchmarks/csum_overhead_2026-10-07.md`. Archive the unmatched remote histories
before any deletion; equivalence is semantic supersession, not patch identity.

**D1 — nested-image schedules and controlled study.** `25b3afc2` includes the
`9c12de8f` measurement prefix and extra evidence/generator work; 43 changed paths
are absent from main, including controlled measurement and provenance records.
The branch's `benchmarks/dc_decode_controlled_2026-10-11/README.md` explicitly
says acceptance is pending and distinguishes historical from composed source.
`48e593c2` is an empty child with a tree identical to `f43a2200` (empty direct
`git diff`); its original code is preserved in the f43 archive. The empty wrapper itself
is not archive-reachable; preserve that tip too before retiring its bookmark
if exact history retention is required. `5efb37cf` is a
rebased schedule pair covered by the DC-study archive, but is also a workspace
parent. Runtime-only p1/p2 add NEON row state and exact-stripe producer scheduling;
they are not patch-equivalent to main. Preserve them, compare on current main,
then run the controlled byte/pixel/backend and 1/2/4/8-thread gates before any
integration. This review does not claim current-main performance benefit or
bug-fix necessity. Coordinator's separate decoder inventory remains separate.

**E1 — exact-block lab.** `5c03a48c` retains estimation/prototype development and
production-policy history. The delivered mechanism is represented by merged
[PR6](https://github.com/imazen/zqi/pull/6), current `imij/src/lossy/exact.rs`,
and `benchmarks/lossless_over_lossy_2026-10-07/README.md`. Nine branch-changed
files are byte-identical to main; the others evolved. Its 27 unmatched patches
are not proof of 27 missing production changes. Preserve the complete historical
tip under an archive, rather than merge the development stack.

**E2 — contexts rejected at gates.** Main retains the two study READMEs but
omits the four branch-only example/module paths (`wpctx.rs`, `wpctx_simd.rs`
and two examples). `benchmarks/wpctx_2026-10-11/README.md` reports rejection on
bytes and decode gates. `benchmarks/cheapctx_2026-10-11/README.md` names
`d9070780` and reports the early failed median gate with only 3/18 timing cells
covered. These are retained historical findings, not freshly reproduced results.
Keep both tips because cheapctx is a different prototype, not a substitute for
the original weighted-context implementation.

**E3 — early large-transform lab and kernel lane.** `c954e75f` is an ancestor of
`601dd68b` (the full lab stack's commit history includes it). Main's production
superblocks are a later port with different grammar. The original lab stack
contains 19 changed paths absent from main, including `dct16_lab.rs`, its selector,
probes, and the kernel report. That report at `601dd68b` records the gate as not
met, with incomplete architecture qualification; no new timing is inferred.
The full lab tip is a protected `dct16kernel` workspace parent. Preserve its
complete history; do not merge old lab machinery into the later production port.

**E4 — exact-unit option 1 rejected.** Main's
`benchmarks/exact_blocks_per_unit_2026-10-11/README.md` chooses option 2
(head locators), not the branch's nested-image-per-unit prototype. Two changed
study/harness files are identical to main; implementation files differ. Preserve
the measured losing implementation at `050fdd89`, without adopting its grammar.

**E5 — interslice.** `0032f635` is contained in archived descendant `0acd200f`.
Its branch study/test files are absent from main; uniform page references now
exist through the later unified sequence design. The reference-difference
prototype is not established as adopted by that fact. Its own README names the
old lab stripe discriminator and old container restrictions; retain the source
and evidence in the archive. Any revival needs a current sequence-format gap
analysis and byte/pixel/speed gates, rather than replaying the old page patches.

**E6 — count grammar.** Main's `benchmarks/nz_grammar_2026-10-07/REVIEW.md`
explicitly rejects RunLevelCount under the unchanged decode gate and points at
`84d20b96` for the retained code. Branch-only files include `nz_lab_oracle.rs`,
review tests and `nz_decode_pair.rs`. The WIP tip `5ddb1d3e` is a sibling overlay,
not an ancestor of the final reviewed tip: its four-file delta changes coefficient
and AVX-512/kernel-choice code. Preserve it separately; neither tip should be
silently discarded or integrated as a missing feature.

**E7 — external base intentionally deferred.** [PR69 closing comment](https://github.com/imazen/zqi/pull/69#issuecomment-6242134824)
explicitly retains `external-base`; main instead uses self-contained sequences.
`imij/src/frames/external.rs`, its tests and the draft benchmark are absent from
main. This is substantive unmerged wire/API work, not a stale merged branch.
Keep the reference; reopening requires decisions on identity, locator and
standalone extraction, plus current registry reassignment checks.

**S1 — small transforms.** The pre-rebase review contains 30 equivalent and ten
unmatched patches, with 27 changed files already identical to main; the old
study history is exactly archived. The follow-up `3ee90710` has source and tests
represented on main, including `production_intra_frames_and_pages_opt_in` at
`imij/tests/small_transforms.rs:776`, and the current integration guide.
It still has an unmatched patch because the code evolved; do not equate that
with a missing feature. Keep its protected workspace-parent bookmark.

**S2 — superblock production-port aliases.** Pairs `final2`/`experiments/superblock-final2`,
`rebased`/`experiments/superblock-rebased`, `sbfinal`/`experiments/superblock-sbfinal`,
`sbport`/`experiments/superblock-sbport`, and
`sbport-review`/`experiments/superblock-review` have exact matching targets.
The final review at `64ef19df` has 46/46 non-merge patches equivalent to main,
so its nonzero ancestry-ahead is rebase duplication, not missing functionality.
Earlier port tips mix equivalent and unmatched pre-review revisions; retain
all historical tip reachability when archiving (one later tip does not preserve
an earlier rebased sibling). The extra `superblocks` tip `81cf3e97` adds a unique
21-file WIP overlay on `64ef19df`, including old lab modules and retired entropy
copies. Its one unmatched patch is substantive historical material, not a
production fix. Archive that overlay, not just its reviewed parent. Main's
superblock source and tests are already implemented; do not resurrect old lab
modules because their files are absent.

**P1 — preservation aliases.** Pages/checksum composition, profile/sample
capability, original RDOQ source-recompute work and qscale effort evidence have
exact or descendant origin archives listed below. The pre-capability profile
commit is a merge (zero cherry patches, one ancestry-ahead), not an unimplemented
feature. Main has current profile reporting, checksum composition, and RDOQ
recomputation through the encoder row pipeline (`imij/src/lossy/rdoq_lab.rs:370`).
These old histories are not integration queues. Retain original evidence and
failed/unrun states; only their redundant unowned bookmark names are retirement
candidates. Current RDOQ/source and metric claims need their own gates if revived.

**W1 — gh-pages.** Disconnected deployment history contains immutable bundles;
current `docs/SITE_DEPLOY.md` and project hosting notes use complete Cloudflare
Pages stages. No source integration is needed. There is no archive covering this
tip in the snapshot. Keep it pending an explicit deployment-history retention
review; this audit does not authorize deletion of generated artifacts/history.

**M1 — main and PR81.** PR81 is merged, its tip `f54a2ace` is an ancestor of
main, zero ahead and one behind. The branch is the clearest deletion candidate
once the coordinator verifies no new work has appeared. Main stays protected.

## Existing archive coverage

An exact archive target proves identical preserved history; a descendant preserves
history but is not a byte-identical tip. `—` means no origin archive contains the
target, not proof the work is absent everywhere. See branch recommendations before
retiring any bookmark, especially protected workspace parents.

| Target | Exact origin archive | Descendant origin archive (if no exact) |
|---|---|---|
| `7c28b06b790f` | — | `archive/2026-10-07-host-uptime-and-load-for-every` |
| `b8ce06434101` | — | — |
| `5c724324b9c9` | `archive/2026-10-07-host-uptime-and-load-for-every` | — |
| `7f80ff75ca60` | — | — |
| `25b3afc22ab7` | `archive/2026-10-11-prepare-source-bound-geometry-fixture-generators` | — |
| `9c12de8fb75b` | — | `archive/2026-10-11-prepare-source-bound-geometry-fixture-generators` |
| `f43a22007049` | `archive/2026-10-11-prefetch-nested-exact-block-stripes-before` | — |
| `48e593c2c245` | — | — |
| `5efb37cf9467` | — | `archive/2026-10-12-dc-decode-study` |
| `5c03a48c103d` | — | — |
| `d9070780c0b0` | — | — |
| `c954e75fe101` | — | — |
| `050fdd891cf8` | — | — |
| `0032f635a579` | — | `archive/2026-10-11-distinguish-compiled-and-archived-experiment-exe` |
| `84d20b9694b9` | — | — |
| `5ddb1d3ea1be` | — | — |
| `51f400e615c5` | — | — |
| `e9f30a22702b` | — | — |
| `64ef19df2e1e` | — | — |
| `63b456790241` | — | — |
| `8d3827a6e2ea` | — | — |
| `601dd68b7cfb` | — | — |
| `ef7fe06a4f67` | — | — |
| `f1ab140de17c` | — | — |
| `ebe8dc07b9ea` | — | — |
| `f54a2aceb11e` | — | — |
| `98218a02ca49` | — | — |
| `78e8a6e11a2a` | — | — |
| `80387691dfa7` | `archive/2026-10-08-compose-mixed-pages-and-checksum-ordered` | — |
| `e832a1864594` | — | `archive/2026-10-08-retain-signed-sample-capability-in-the` |
| `2ed66d7b5ab1` | `archive/2026-10-08-retain-signed-sample-capability-in-the` | — |
| `d8c53e9b9789` | `archive/2026-10-08-rdoq-lab-source-recompute-through-the` | — |
| `0a2df8c38773` | — | `archive/2026-10-12-preserve-reviewed-exact-stripe-producer-scheduli` |
| `e2a3992d2ae5` | `archive/2026-10-12-preserve-reviewed-exact-stripe-producer-scheduli` | — |
| `d2ee51107e0d` | `archive/2026-10-11-readme-what-ships-arithmetic-selector-tuning` | — |
| `3ee9071076ca` | `archive/2026-10-12-qualify-opted-in-intra-frames-and` | — |
| `816e6fffb7ac` | `archive/2026-10-08-qscale-compatibility-with-final-production-trell` | — |
| `55d67b375019` | — | `archive/2026-10-08-qscale-compatibility-with-final-production-trell` |
| `81cf3e977af5` | — | — |

## All open issues

| Issue | Classification | Verified evidence | Concrete next action |
|---|---|---|---|
| [#73 Native corpus inventory](https://github.com/imazen/zqi/issues/73) | Actionable; not resolved | Source search across `imij/src` and tool Rust/manifests finds only the `Op::Inventory` error-name mapping at `imij/src/codec.rs:495`, no inventory adapter/tool. Existing corpus encoder and strict validator do not supply the requested structural accounting. No closure evidence for the explicit 4,300-file run. | Implement a private inventory adapter/executable against the shared contract; account for payloads, auxiliary layers and trailing bytes; malformed/bounded tests; run the explicit route list and retain a structural-only report. Do not register private IMI in the public registry merely to make dispatch succeed. |
| [#64 Lossless larger-than-PNG tail](https://github.com/imazen/zqi/issues/64) | Needs decision; investigated, unresolved | Latest comment and main `experiments/copy_inline_2026-10-18/README.md` agree the existing inter-copy experiment failed its speed gate; 160 RGB8 tested, 20 RGB16 not covered. `1621935a` is the retained study commit. This is evidence against that implementation, not proof the lossless tail is fixed. | Keep open. Owner decides whether a dedicated inline-copy kernel merits a new lab study; preserve the failed inter-copy route. If authorized, include all 180 cases and unchanged byte/pixel/decode-speed gates; no production format change before the gate and API/format decision. |
| [#14 Bounded encoding beyond lossless](https://github.com/imazen/zqi/issues/14) | Partially done; shared-trait integration actionable | PR68/71/72 are merged. `imij/src/stream_encode.rs:44` exports bounded session/config; `imij/tests/bounded_lossy.rs` covers parity, checksums, injected failures and latent ranges; `imij/tests/codec_bounded.rs` covers spool limits/failure/cancellation. But `imij/src/codec/decode.rs:266,383` still uses `BufferedPullDecoder`, calling complete `decode()` before returning, and no shared `dyn_incremental_decoder`/`push_decoder_from_source` implementation is present there. Latest issue comment explicitly preserves that integration gap. | Keep open with a narrower shared input/output contract checklist, or split a follow-up before closing the completed encoder milestone. Bind native row/feed/session work through current shared traits; make replay/scratch/backpressure/first-output contracts visible and qualify end-to-end exact pixels/metadata. Do not redo already merged bounded-lossy work. |
| [#13 Remote archive, pruning and references](https://github.com/imazen/zqi/issues/13) | Partially done; remaining choices and qualification | `imij-set/src/reader.rs:333,541,561,572` implements batched keys and dimensions/CICP/filename pruning. `next_matching` at line 982 remains a bounded sequential scan. `docs/ARCHIVE_SEEDS.md` is explicitly design-only for external seeds. PR69 is closed unmerged; PR70 is merged. Latest issue comments separate HTTP/1.1/native qualification from remaining HTTP/2/CDN, multi-object revision, publication and reservation gates. | Keep open. Decide typed/compound pruning and seed requirements; implement manifest conditional replacement only under an agreed contract. Qualify actual CDN/WAN workloads with matched objects/queries and retain request/byte/time evidence. Track reservation-return-size checks and final color/checksum publication separately from the completed multipart repair. |

Suggested closures now: **none**. No open issue is wholly obsolete or a duplicate
on the evidence inspected. #14 can become a completed bounded-encoder milestone
only if its remaining shared integration work is explicitly retained elsewhere;
that is a tracking decision, not an automatic closure recommendation.

The #13 HTTP fixes and native test evidence are in `4a406e58` / `d0119e8b`
and `docs/BROWSER_ENGINE_AND_FORMAT_FREEZE.md`; they do not establish cloud
throughput or browser memory bounds. The #14 resource evidence remains in
`benchmarks/bounded_lossy_2026-10-17.md`; no resource or speed number is claimed
as a fresh measurement by this audit. Historical docs that predate later work
were treated as historical, not edited during triage.

## Full target identities

These identify exactly the objects reviewed, including both versions of the
checksum refs and stale `main@git`; use freshly resolved refs for any later action.

| Prefix | Full commit |
|---|---|
| `0032f635a579` | `0032f635a57917c6f23ff8155878426dcc894776` |
| `050fdd891cf8` | `050fdd891cf81eb6a79b4c04736d07ab90228da3` |
| `0a2df8c38773` | `0a2df8c387739f1f38a9ae97032973513795b3d2` |
| `25b3afc22ab7` | `25b3afc22ab7416639a39e24a24f707ee3cc1523` |
| `2ed66d7b5ab1` | `2ed66d7b5ab1b1c09553e0afbaa173308124231c` |
| `3ee9071076ca` | `3ee9071076ca7af103aa070ed434fe277210d57d` |
| `48e593c2c245` | `48e593c2c245496781ed11f93546deaf195d3d6d` |
| `51f400e615c5` | `51f400e615c5d1b8e21f5d97dbbb531c81c523fb` |
| `55d67b375019` | `55d67b37501953fccabcf67441e128f54a08c180` |
| `5c03a48c103d` | `5c03a48c103d360a18fa5802e3f1e12cf53bd9e6` |
| `5c724324b9c9` | `5c724324b9c9d7254e6bdb20abd349c642ec857c` |
| `5ddb1d3ea1be` | `5ddb1d3ea1be0471f1968a998c35a62ded9197ce` |
| `5efb37cf9467` | `5efb37cf9467977ef5260b67e072a523ffe531d2` |
| `601dd68b7cfb` | `601dd68b7cfbebccdedb493c5faafcca1b313dd9` |
| `63b456790241` | `63b456790241a487de13aa3367c9744e063614c4` |
| `64ef19df2e1e` | `64ef19df2e1ed46e77c9a7209792d44d54302e09` |
| `78e8a6e11a2a` | `78e8a6e11a2a53133903a13dc89488b1c818b659` |
| `7c28b06b790f` | `7c28b06b790f4ac6b3518c1d0e14adfbb3d67a88` |
| `7f80ff75ca60` | `7f80ff75ca60975aaeda7f5af6f11e0f1159b643` |
| `80387691dfa7` | `80387691dfa78894afb9bae55ebe2cf490951530` |
| `816e6fffb7ac` | `816e6fffb7ac91f1d7d4b0125ecfb6b523e669d4` |
| `81cf3e977af5` | `81cf3e977af57313c198b037cd471203bca992d5` |
| `84d20b9694b9` | `84d20b9694b93f660db289100ae5a8d625c17018` |
| `8d3827a6e2ea` | `8d3827a6e2ea69bb1f265a96383ee7a31d522483` |
| `98218a02ca49` | `98218a02ca4994f2e0e03e3d71c035c427b7cd48` |
| `9c12de8fb75b` | `9c12de8fb75b9b3d0b37231f0b74819425c6cd36` |
| `b8ce06434101` | `b8ce06434101768560f90683c67c3ab1b63bdc1f` |
| `c954e75fe101` | `c954e75fe101c3792e17031086be6e157beb4386` |
| `d2ee51107e0d` | `d2ee51107e0d22134f8cf6c62a3433762af5cbbf` |
| `d8c53e9b9789` | `d8c53e9b97896c4424efd61aff1aa56f359659c3` |
| `d9070780c0b0` | `d9070780c0b0c51d85b3721ca8db2c0e89b848d1` |
| `e2a3992d2ae5` | `e2a3992d2ae54424f0070af50180fd459b82d1b9` |
| `e832a1864594` | `e832a1864594e59e26beaeb2c26821eff51b64dc` |
| `e9f30a22702b` | `e9f30a22702b1f323ed6fcdfe5c6fa6efa5fe252` |
| `ebe8dc07b9ea` | `ebe8dc07b9eacd308b2c70da5be8337fdbe32c63` |
| `ef7fe06a4f67` | `ef7fe06a4f672b39e760efbf2e946530f78b7469` |
| `f1ab140de17c` | `f1ab140de17c86fb82b3a4513b4367834f1f36af` |
| `f43a22007049` | `f43a220070492d6d5b4196b0a2b4d6b46e30828c` |
| `f54a2aceb11e` | `f54a2aceb11e24736196b100077f1bfa62658800` |

## Review and landing boundaries

The coordinator owns landing this audit; it is deliberately unpushed. No cleanup
recommendation has been executed. Before a later cleanup: re-fetch through jj,
re-resolve branch targets, active PRs, workspace parents and ownership; preserve
unarchived sibling histories; only then retire selected aliases. This prevents a
new push or live workspace from being mistaken for the reviewed historical tip.
The coordinator separately owns native-JPEG gating, v3/converter rejection,
monomorphization and decoder complexity work; those were not expanded here.

## Protected workspace snapshot

All tips below and their immediate parents are protected, including unnamed
changes. Refresh this metadata before any later cleanup.

| Workspace | Tip | Immediate parent |
|---|---|---|
| `branch-triage-20261027` | `39228da6d8c3cbd91f62b81b2bdefca561df76df` | `78e8a6e11a2a53133903a13dc89488b1c818b659` |
| `api` | `c14f98d3fe32d89b1ba9f8e6d22f44eee159ad63` | `78e8a6e11a2a53133903a13dc89488b1c818b659` |
| `root` | `050a82641943abfa0cd6e158ba3b7186c3731c58` | `5b396027aefd4e34f6ced036ce284155dbbfa270` |
| `default` | `3b3e29490ea848a1a96849024896b607bcc39148` | `3d251523f4572c5344044f3bd78c2305042a14d6` |
| `wasm-swe2-20261016` | `147dcb2d0eae0a0f66dcdf3f4c3078daf28bce90` | `f94c6ef44db4bb3800020774c84a6b8121b8e7a9` |
| `kbit-investigate` | `2ee8dfb35d843b0ac054dc624f90eab9aea34650` | `9259eb3436799e8b9d63892b90bcee9503245d1a` |
| `imij-devin-handoff` | `7d547f3a7857b3612bd97ef7d5f71c7910e09214` | `f00c0adbadc89ec4e43ed0c4f30cc5911abee286` |
| `smallxf` | `d05c9074853ad0fe94d00a883a6be9bbb19bf068` | `3ee9071076ca7af103aa070ed434fe277210d57d` |
| `zqi--jpegsize` | `9d855a852d33e633f9da98f4de7b26a299949aec` | `f25c8acb7f4873b49c14580186c8d9b9f0ef0e6f` |
| `dcdec` | `0245e04948ecc9fa8ee1d0aac5bd7bda05978006` | `17a3a34093f465f5b0f528be7fd4cf10c533afae` |
| `dcdec-review` | `67a383646ab2a89898304cdb5dc0ee52d283768f` | `5efb37cf9467977ef5260b67e072a523ffe531d2` |
| `interperf-is` | `e454dab7b4b2dc5413da485d966d1cfee9f4b5d4` | `0acd200f79de56064d1ae95a987128c0953a433b` |
| `freezefix` | `f1ebf3afe690c359195b6f0b70782fce5c629490` | `73ee6c85e89b7cc9efd084680d1c64e13ba3a915` |
| `kbitws` | `1a1eccc7176de15f8d165be1d242b6de42583aba` | `8344d4a83d4cee88a6f18ebf520b95f99a74763e` |
| `capmask` | `d2804cc97ba40691bd67f675527d1508be3b3226` | `aadedcf0bbe388fd44fc0a1618062a4b370232da` |
| `dct16kernel` | `54a97954d4ed675f344bb47bfb44e4f219555f26` | `601dd68b7cfbebccdedb493c5faafcca1b313dd9` |

## Executed adopted-branch cleanup — 2026-10-27

This section records later user-authorized actions. The original triage above is
a historical snapshot: its no-mutation/no-push statements and recommendations
precede this cleanup. Base for execution: `a07c379cd431fc65c5596bd0336d67d74e817699`
(merged PR82). There were no open PRs. All workspace tips and immediate parents
were re-resolved and excluded, as were unadopted experiments, ambiguous unique
work, and every nested-image decoder candidate still awaiting qualification.

**Executed:** removed seven pre-existing local jj bookmarks and twelve origin
branches, across seventeen distinct names. Ten of the origin branches were
remote-only; these were explicitly tracked, then their temporary local bookmarks
were deleted too. Thus `jj bookmark delete` ran for seventeen names, while the
net deletion of pre-existing local names is seven. Six of those seven already
had exported Git branch refs; `imi-decoder-footprint` had not been exported.

Four new local/origin archive refs were pushed and their exact GitHub targets
verified **before** retiring any branch. Existing archive refs were unchanged.
All before/after exact names, scopes and full hashes are in
[the cleanup ref audit](BRANCH_CLEANUP_2026-10-27.tsv). `=` in its After column
means the exact full Before SHA; this snapshot is after deletion and before the
report commit advances main. The later main advancement contains only this
executed record and its TSV, with no source changes.

| Retired name | Pre-existing scopes removed | Full original target | Adoption evidence / retained history |
|---|---|---|---|
| `imi-encode-feature` | local/origin | `f54a2aceb11e24736196b100077f1bfa62658800` | Merged PR81; `main@origin` |
| `imi-decoder-footprint` | local/origin | `c8337fc71857ed17c96663269636fd2ec387ce76` | Merged PR82; `main@origin` |
| `experiments/superblock-final2` | origin | `51f400e615c5d1b8e21f5d97dbbb531c81c523fb` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-final2` |
| `experiments/superblock-rebased` | origin | `e9f30a22702b1f323ed6fcdfe5c6fa6efa5fe252` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-rebased` |
| `experiments/superblock-review` | origin | `64ef19df2e1ed46e77c9a7209792d44d54302e09` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-review` |
| `experiments/superblock-sbfinal` | origin | `63b456790241a487de13aa3367c9744e063614c4` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-sbfinal` |
| `experiments/superblock-sbport` | origin | `8d3827a6e2ea69bb1f265a96383ee7a31d522483` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-sbfinal` |
| `final2` | origin | `51f400e615c5d1b8e21f5d97dbbb531c81c523fb` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-final2` |
| `rebased` | origin | `e9f30a22702b1f323ed6fcdfe5c6fa6efa5fe252` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-rebased` |
| `sbfinal` | origin | `63b456790241a487de13aa3367c9744e063614c4` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-sbfinal` |
| `sbport` | origin | `8d3827a6e2ea69bb1f265a96383ee7a31d522483` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-sbfinal` |
| `sbport-review` | origin | `64ef19df2e1ed46e77c9a7209792d44d54302e09` | Adopted production port; original review S2; `archive/2026-10-27-adopted-superblock-review` |
| `review/smallxf-before-main-rebase` | local | `d2ee51107e0d22134f8cf6c62a3433762af5cbbf` | Adopted small transforms; original review S1; `archive/2026-10-11-readme-what-ships-arithmetic-selector-tuning` |
| `preserve-pages-csum-composition-20261008` | local | `80387691dfa78894afb9bae55ebe2cf490951530` | Current mixed-pages checksum regression at imij/tests/mixed_pages.rs:859; `archive/2026-10-08-compose-mixed-pages-and-checksum-ordered` |
| `preserve-profile-samples-before-capability-fix-20261008` | local | `e832a1864594e59e26beaeb2c26821eff51b64dc` | Current capability/profile reporting and imij/tests/profiles.rs; later extended-tier policy retained; `archive/2026-10-08-retain-signed-sample-capability-in-the` |
| `preserve-profile-samples-reviewed-20261008` | local | `2ed66d7b5ab1b1c09553e0afbaa173308124231c` | Current capability/profile reporting and imij/tests/profiles.rs; later extended-tier policy retained; `archive/2026-10-08-retain-signed-sample-capability-in-the` |
| `preserve-rdoq-original-20261008` | local | `d8c53e9b97896c4424efd61aff1aa56f359659c3` | Current RDOQ shared row pipeline at imij/src/lossy/rdoq_lab.rs:440; `archive/2026-10-08-rdoq-lab-source-recompute-through-the` |

The final superblock review remains 46/46 patch-equivalent to current main.
Earlier port revisions contain pre-review differences; the prior source audit
establishes adoption of the delivered mechanism, and their original histories
remain archived rather than discarded. No old lab overlay was included.
Four separate refs were necessary because the rebased histories are siblings;
one final tip does not contain all of them. The `sbport` tip is an ancestor of
`sbfinal`, verified with `git merge-base --is-ancestor`.

| New preservation ref | Exact remote target |
|---|---|
| `archive/2026-10-27-adopted-superblock-final2` | `51f400e615c5d1b8e21f5d97dbbb531c81c523fb` |
| `archive/2026-10-27-adopted-superblock-rebased` | `e9f30a22702b1f323ed6fcdfe5c6fa6efa5fe252` |
| `archive/2026-10-27-adopted-superblock-review` | `64ef19df2e1ed46e77c9a7209792d44d54302e09` |
| `archive/2026-10-27-adopted-superblock-sbfinal` | `63b456790241a487de13aa3367c9744e063614c4` |

Validation after deletion: fresh `jj git fetch` and bookmark inventory, exported
Git `refs/heads`, and paginated live GitHub branch API all agree. Every retired
name is absent. Every retained local/origin target kept its exact before hash.
`jj git export --ignore-working-copy` synchronized the previously stale
`main@git` from `98218a02` to execution-base main `a07c379c`; this changed a ref,
not any other workspace's files. Counts including archives: local 82 → 79
(seven removed, four created), origin 78 → 70 (twelve removed, four created).
The report push intentionally advances main afterward.

Preserved exceptions include `main`, every pre-existing `archive/*`, protected
workspace-associated refs, all `dcdec-*` and runtime-only decoder candidates,
`exact-blocks-lab` (prototype history), `csum-checksum` / `csum-checksum-v2`
(divergent superseded grammars), both count-grammar experiments, cheap/weighted
contexts, exact-unit option 1, interslice, early DCT16/superblock labs,
`superblocks` (unique WIP overlay), `external-base`, qscale evidence aliases and
`gh-pages`. Archival availability alone was not treated as adoption permission.

No source files, tests, issue state or other workspace files were changed. No
builds ran. Only this continuation's `branch-cleanup-20261027` workspace is
scheduled for removal after the report is verified on origin/main; that final
lifecycle check is reported in the original chat, avoiding a self-referential
post-cleanup report edit.

## Remaining remote branches and diagonal DCT follow-up — 2026-10-27

Live GitHub inventory after the executed cleanup: fifteen remote names outside
`archive/`. This follow-up is triage only; no additional archive or deletion
operation was performed. Main was `61ba82e0dbc8088e97adac281cad4450497e6049`.

| Remote branch | Status | Recommended next action |
|---|---|---|
| `main` | Current adopted implementation | Keep. |
| `csum-checksum` | PR1 superseded by PR2 and then merged PR4; obsolete whole-file design, not a missing checksum implementation | Archive both divergent local/remote histories before retiring names; don't merge old grammar. |
| `csum-checksum-v2` | PR2 superseded by piecewise checksums; local history archived, remote rebased history differs | Preserve exact remote history in an archive before retiring; retain timing evidence. |
| `exact-blocks-lab` | Delivered exact-block mechanism adopted via PR6; branch also contains prototype/estimation history | Archive complete original tip; do not treat all unmatched prototype patches as missing production work. |
| `experiments/cheapctx` | Above-row/shared-context experiment stopped at failed runtime gate; recorded timing coverage 3/18 | Archive failed candidate and evidence; no integration recommendation. |
| `experiments/dct16-merged` | First larger-DCT study rejected as built; later improved superblock port adopted | Archive as the superseded early study, preserving distinct original source and measurements. |
| `experiments/exactunit-opt1` | Nested exact image per unit rejected; head-locator option 2 adopted instead | Archive losing implementation and measurements; don't integrate option 1. |
| `experiments/interslice` | Unique lab reference-difference stripe mode; uniform-page references later have a production route | Keep pending focused source/evidence review. Separate already-adopted page-reference functionality from unadopted difference coding. |
| `experiments/nz-count-grammar` | RunLevelCount rejected at unchanged native decode gate; oracle/test source absent from main by design | Archive final reviewed failed experiment intact. |
| `experiments/nz-count-grammar-devin-wip` | Separate coefficient/kernel WIP, not ancestor of final reviewed count-grammar tip | Preserve separately before retiring; do not collapse into final experiment history. |
| `experiments/superblocks` | Original lab study plus kernel/glue follow-up, not the production-port branch; protected workspace parent | Keep while workspace remains associated; eventual archive must preserve the entire lab stack. Production superblocks are already adopted. |
| `experiments/wpctx` | Weighted-predictor contexts rejected on recorded size and decode gates | Archive failed experiment and optimized reference implementation. |
| `external-base` | PR69 intentionally closed unmerged, with explicit instruction to keep branch; cross-file resolver/extraction/identity decisions deferred | Keep as deferred reference; production sequences cover the self-contained use case. |
| `gh-pages` | Legacy immutable deployment bundles, disconnected history; site now uses Cloudflare Pages | Keep pending deployment-history retention decision; no source integration work. |
| `superblocks` | Adopted production-review parent plus unique 21-file WIP lab overlay | Archive complete WIP tip before retiring the name; no merge of retired entropy copies/old lab modules. |

The key unresolved integration candidate is interslice, not the failed context
or count-grammar experiments. Its archived README reports useful compression
results and ARM timings, but its rule evaluation describes x86 as matching
existing inter prediction rather than demonstrating the stated intra-relative
ceiling. Main has no `DIFF_STRIPE` implementation or reference-difference stripe
mode. The later general page-reference interface alone does not establish that
this lab coding was adopted. Requalify the source-bound binaries, both coding
choices, exact pixels, and 1/2/4/8-thread costs before an adoption decision.
No historical branch performance claim was rerun during this review.

### Diagonal DCT: no recorded transform evaluation found

Searched all seventy current origin branch heads, including archives (seventy
unique target commits), for diagonal/directional/rotated DCT names, DDCT, AFV and
Hornuss, across Rust, Python, Markdown, text and TOML. There were no matches.
Also searched all reachable commit subjects for diagonal/directional/rotated DCT
or transform terms, and inspected the transform and experiment ledgers. This is
negative repository evidence, not proof an unrecorded experiment never happened.

There **was** an evaluation of diagonal coefficient grouping in packed entropy
coding: `imij-bench/src/lossy.rs::groups_diag` groups existing DCT coefficients
by zigzag anti-diagonals. `docs/LOSSY.md`, E8, records that fixed groups of four
beat diagonal, 2x2 and half-row grouping. That changes entropy grouping, not the
spatial transform. A second false lead, `lift_scales` in the same benchmark
source, checks that lifting-DCT scale conversion is diagonal in the ordinary
DCT basis; it is not a diagonal-direction transform. The small-transform Haar
“diagonal detail” is likewise not a diagonal DCT.

Confirmed transform studies are the ordinary separable 8x8 DCT, four 4x4 tiles
with a 2x2 Haar DC combination, and merged 16x16/16x8/8x16 DCTs. A spatially
rotated/diagonal DCT therefore has no identified implementation, RD study or
decode-speed result in the inspected repository evidence. It has not been shown
to pass or fail an IMI adoption gate.

## Executed disposition renames — 2026-10-27

User-authorized renames of the fourteen remaining non-main, non-archive origin
branches, following the remaining-branch triage above. `to-review/` marks work
requiring technical review; `deferred/` marks an intentionally retained proposal,
workspace-associated lab or pending retention decision; `archive/` marks rejected,
superseded or preserved historical code. Main is unchanged by the rename operation.
Local-only decoder/qscale/review bookmarks outside that remote inventory were not
renamed. Existing archive names were retained.

All sixteen replacement refs were created, pushed and verified on GitHub at the
original full commit IDs before any original name was retired. Fourteen origin
names and their thirteen corresponding pre-existing local names were retired;
the remote-only `superblocks` name was explicitly tracked for deletion. Two
checksum branches had divergent local tips: these got separate `-local` archive
names, also pushed, preserving both exact histories. This is name cleanup, not
adoption or rejection of additional code.

| Original name | Replacement | Scope represented | Exact preserved commit |
|---|---|---|---|
| `csum-checksum` | `archive/csum-checksum` | origin | `b8ce06434101768560f90683c67c3ab1b63bdc1f` |
| `csum-checksum` | `archive/csum-checksum-local` | divergent local, preserved on origin too | `7c28b06b790f4ac6b3518c1d0e14adfbb3d67a88` |
| `csum-checksum-v2` | `archive/csum-checksum-v2` | origin | `7f80ff75ca60975aaeda7f5af6f11e0f1159b643` |
| `csum-checksum-v2` | `archive/csum-checksum-v2-local` | divergent local, preserved on origin too | `5c724324b9c9d7254e6bdb20abd349c642ec857c` |
| `exact-blocks-lab` | `archive/exact-blocks-lab` | origin + local | `5c03a48c103d360a18fa5802e3f1e12cf53bd9e6` |
| `experiments/cheapctx` | `archive/cheapctx` | origin + local | `d9070780c0b0c51d85b3721ca8db2c0e89b848d1` |
| `experiments/dct16-merged` | `archive/dct16-merged` | origin + local | `c954e75fe101c3792e17031086be6e157beb4386` |
| `experiments/exactunit-opt1` | `archive/exactunit-opt1` | origin + local | `050fdd891cf81eb6a79b4c04736d07ab90228da3` |
| `experiments/interslice` | `to-review/interslice` | origin + local | `0032f635a57917c6f23ff8155878426dcc894776` |
| `experiments/nz-count-grammar` | `archive/nz-count-grammar` | origin + local | `84d20b9694b93f660db289100ae5a8d625c17018` |
| `experiments/nz-count-grammar-devin-wip` | `archive/nz-count-grammar-devin-wip` | origin + local | `5ddb1d3ea1be0471f1968a998c35a62ded9197ce` |
| `experiments/superblocks` | `deferred/superblocks-lab` | origin + local | `601dd68b7cfbebccdedb493c5faafcca1b313dd9` |
| `experiments/wpctx` | `archive/wpctx` | origin + local | `ef7fe06a4f672b39e760efbf2e946530f78b7469` |
| `external-base` | `deferred/external-base` | origin + local | `f1ab140de17c86fb82b3a4513b4367834f1f36af` |
| `gh-pages` | `deferred/gh-pages` | origin + local | `ebe8dc07b9eacd308b2c70da5be8337fdbe32c63` |
| `superblocks` | `archive/superblocks-wip` | origin | `81cf3e977af57313c198b037cd471203bca992d5` |

Independent verification after fresh jj fetch: all fourteen old names absent
from origin and exported Git branches; all sixteen replacements resolve to the
expected exact commits in jj, Git and live paginated GitHub branch inventory.
Every retained bookmark target was unchanged except synchronization of stale
`main@git` to origin/main. All workspace working-copy commits and immediate
parents remained exactly unchanged, including the lab workspace whose branch
is now `deferred/superblocks-lab`. No workspace was removed.

The unarchived remote work queue is now `to-review/interslice`,
`deferred/external-base`, `deferred/gh-pages`, `deferred/superblocks-lab`, plus
`main`. Previous sections and dated study documents retain historical branch
names; this table is their resolving map. No source changes, builds or issue
mutations were performed. The report commit advances main after the ref checks.
