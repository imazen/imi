# Checksum offset review (2026-10-08)

The piecewise checksum grammar permits piece sizes from 2^10 through
2^40 bytes. A small file with a single piece of size 2^32 is valid on a
32-bit reader too; the piece size is not an allocation request.

The review witness `large_piece_sizes_on_every_path` failed on i686 before
the correction: the chunked `DecodeSession::decode_reader` returned
`TooLarge` at k=32. `Check::cover` narrowed the remaining piece length
before clamping it to the input slice. At the beginning of a large piece
that produced zero and repeatedly finished empty pieces without consuming
input. It now clamps in u64 before converting to usize.

The plane reader had the same narrowing order in its initial header read
and in the scratch size and read length for a checksum range. These now
clamp before narrowing. Its checksum-table allocation checks that the
whole table length fits usize. The sparse positional-reader witness
`wide_plane_offsets_keep_bounded_reads_nonempty` checks the first bounded
read at 2^32-byte offsets and for 2^32, 2^33 and 2^40-byte pieces, without
allocating or hashing those entire ranges.

Other reviewed conversions have existing bounds: parsed digest count is
checked against usize and the actual table payload, piece ranges in a
slice-backed file are bounded by that slice's covered length, and reader
read-ahead is limited to 4 MiB. The plane verified-piece bitset indexes are
bounded by the parsed digest count. Coverage still excludes only the cSUM
chunk; digest comparison, placement rules, prefix parsing and verification
off behavior are unchanged. No public API was added.

Validation: `just check-checksum-review` (fmt, 20 checksum integration
tests, wide-offset witness, lab and minimal-feature clippy) and
`just check-checksum-i686` (the same 20 integration tests and wide-offset
witness) passed. The checksum tests include missing/truncated tables,
unknown algorithms, open DATA, stream rejection before bad-piece rows,
independent XXH3 digests, and optional verification.


## Header extension composition

`finish_file` now asks the strict header parser for the insertion point of
extension2 and preserves all higher extension records verbatim. Appending2
after a pre-existing higher id was invalid under the existing ordering rule.
The regression includes profiles1 and opaque4/6/128, zero-length and multi-byte
payload lengths, and growth of the header-length varint. It checks decoded
pixels, exact header bytes, refinish after removing only the checksum chunk,
and unchanged rejection of deliberately unordered extensions.

The append merge with seed-study main a53c143d passed full integration-core
in a fresh dedicated target (default/lab/minimal tests, clippy, fmt and wasm).
Mixed-page extension3 requires a combined pages/checksum integration snapshot;
this extension4 regression does not claim that separate composition gate.

## Piece ends near 2^64 (follow-up)

`Table::piece` computed the end of piece `i` as `i * 2^k + 2^k` before
clamping it to `covered_len`. Slice-backed files bound `covered_len` by the
slice, but the plane reader takes it from the source's reported length
minus the table gap, and a `ReadAt` may report any length up to `u64::MAX`
(a remote size, for instance). For the last piece of such a source the sum
passed 2^64: debug builds panicked with an overflow, and release builds
produced a range ending before it starts, so round-out hashed nothing and
reported a mismatch without reading the piece. The end now saturates before
the clamp. Witnesses: `checksum::tests::last_piece_ends_at_a_huge_covered_length`
(k = 10, 32, 40 at `covered_len = u64::MAX`) and
`planes::decode::tests::round_out_reads_the_last_piece_of_a_huge_source`
(the first round-out read starts at the last piece's offset); both failed
before the change in debug and release builds.

Reviewed and unchanged:
- The streamed reader's piece counter indexes the digest table through
  `checked as usize`. It cannot pass the table's digest count (at most 2^30
  entries on a 32-bit target) without failing first: past the table it is a
  mismatch, and before the table arrives the pending digests stop at a
  `TooLarge` allocation. So it never truncates.
- A prefix parse (`previews`) checks only the pieces wholly before `DATA` and
  bounds `covered_len` from below by `DATA`'s covered offset, not by a file
  length it does not have. Its callers do not reuse those pieces; any future
  caller must not verify past `DATA` with them.
- Header extension insertion: the library's header writer emits only
  extension 1, and the two remux paths (`compress_metadata`,
  `PlaneSetBuilder`) patch an existing record in place, so today's writers
  never meet a higher record; the ordered insertion above covers a header
  that carries one.
