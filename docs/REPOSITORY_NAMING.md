# Repository naming

Use **reference codec** in prose and `reference_codec` in paths and technical
identifiers. This applies to every branch, commit message and repository-hosted
review or issue. External sources and private measurement archives retain their
original identities. Resolve external dependency package names from the supplied
source manifest and use a Cargo or rustc alias in comparison tools.

Run `just check-terminology` before publishing. Its fingerprint-based check covers
tracked paths and text, including case, punctuation and whitespace variants.
Lookup fingerprints use XXH3-128, matching the hosted-review guard. This
non-cryptographic lookup does not store the restricted name in the repository.
The check supports colocated Git checkouts and isolated JJ workspaces; the latter
use the JJ working snapshot to list files.
Hosted review metadata and reachable history need separate checks when rewriting
history; a working-tree check alone does not cover those surfaces.

The 2026-11-09 consistency pass preserved all 82 branch names and all seven open
PR relationships. The original and transformed graphs contain 3,251 commits.
Every historical file was checked against the deterministic text/path transform;
authors, timestamps and JJ change identities were retained. The `imij` and
`imij-set` diffs in all seven PRs remain byte-identical. Rewriting an affected
signed commit invalidates its old signature; original objects remain in the
private backup.

The neutral adapter was built against the unchanged, hash-verified frozen
external product. All 16 old/new controls produced identical complete streams
and decoded pixels: packed and padded odd layouts, both profiles and quantizers,
1/8 encode workers and 1/2/4/8 decode workers. This is a correctness check, with
no new speed or rate-distortion claim. The naming gate's four fixture tests pass.

The before/after PR snapshots, commit mapping, original bundle, external-link
index and object audit are archived externally under
`naming_consistency_20261109/`. These records are outside the
hosted repository. Original benchmark binaries, source archives and scalar
measurements are unchanged. Neutral filesystem aliases resolve existing archive
pointers without modifying sealed data. The original source commit IDs quoted
by measurements continue to identify the measured source, rather than a rewritten
commit that was never benchmarked.

For older private tables, the selection and C-oracle tools accept an explicit
`--reference-label`. `uhd_summary.py --cref PATH` accepts the retained oracle's
original filename. These options change input identification only; hashes,
required cells and pixel assertions remain mandatory. Source-freezing checks
also validate the actual external product key present in the supplied manifest.

GitHub can retain old commit objects and read-only PR refs after branch updates.
A clean reachable branch history does not establish that those cached objects
have been purged. Keep that hosting limitation separate from the verified local
object and PR graph checks.

Hosted readback on 2026-11-09 verified all 82 branch heads and all seven open PRs:
numbers, open/draft state, branch names, assignments, requested reviewers and
review records are preserved. All 34 existing conversation comments are
byte-identical. The seven open PR head refs point to the new commits.

There are still 65 closed PR head refs pointing at old, rewritten commits among
89 hosted PR refs. These are a verified remaining limit, not a completed purge.
GitHub's [PR namespace is read-only](https://docs.github.com/en/pull-requests/how-tos/review-pull-requests/checking-out-pull-requests-locally#error-failed-to-push-some-refs).
Removing hosting-side references can make closed PR diffs unavailable; see
[GitHub's history-removal effects](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/removing-sensitive-data-from-a-repository#side-effects-of-rewriting-history).
No PR was deleted, closed, reopened or recreated by this pass. A hosting-side
removal has not been requested.

Older local clones must migrate unpublished work onto the rewritten graph before
pushing. Do not merge the old ancestry back into a branch. Keep source/binary
provenance from earlier measurements in the private archive.

## Review continuity

Capture a baseline before updating history or review branches:
`just check-pr-continuity imazen/zqi ABSOLUTE_SNAPSHOT_PATH`.
After publication run
`just compare-pr-continuity imazen/zqi BASELINE_PATH NEW_SNAPSHOT_PATH`.
Snapshots are retained exclusively outside git and never overwritten. The
collector uses the authenticated `gh` CLI and performs read-only API requests.

The check preserves PR numbers/identities, titles, open/draft state, branch
names, repositories, assignments, reviewer requests and existing conversation,
inline-review and submitted-review records. It also checks thread resolution,
current hosted head refs and forward-only head movement. An authorized history
rewrite may supply its explicit old-to-new JSON mapping with the script's
`--commit-map` option. A PR description may be refreshed and new discussion
added without treating either as lost review history. Intentional changes to
other review state require inspection and a new baseline.

GitHub's raw `mergeable: null` means pending calculation; it is not a known
conflict. A previously mergeable PR becoming conflicting or pending fails the
comparison. Recheck pending state before declaring preservation verified.
This is a continuity guard, not an approval or source-correctness review.

The2026-11-09 follow-up found one newly introduced conflict in the streaming
PR's naming document. A normal merge resolved it while retaining both product
crate trees byte for byte; hosted readback again reports PR91 mergeable.
PR84 and PR85 already conflicted with the live main branch before maintenance:
their respective conflicting-file sets are unchanged. The earlier cached PR
base SHAs were older than that live branch and did not establish mergeability.
The original/live merge checks and comparison snapshots are retained in the
private evidence root above. All seven PR identities and stack bases survive;
the nine continuity regression controls pass.
