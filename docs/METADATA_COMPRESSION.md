# Seeded metadata compression

`compress_metadata(file, max_block_bytes)` explicitly remuxes ICC, EXIF and XMP
chunks. Existing encoders keep their bytes unless this helper is called.
Compression is selected only when the complete replacement chunk is smaller;
image payload bytes remain identical. The operation is idempotent.

`mCMP` carries the original four-byte identity (`iCCP`, `eXIf` or `xMPp`), then a
pack record. The same pack format compresses plane-set indexes:

| Mode | Record |
|---|---|
| 0 | byte 0, canonical decoded-length ULEB128, exact raw bytes |
| 1 | byte 1, seed ID 1, canonical decoded-length ULEB128, LZ4 block |

The immutable seed is the 1,163-byte literal in `imij/src/metadata_pack.rs`, SHA256
`3136211e1ae05153d1b854e5bed190c95d0cadf7a2d56504e35b889a3a4c3dde`.
It contains common metadata vocabulary. The LZ4 history begins with those seed
bytes and continues with decoded bytes, including legal overlapping matches.
Seed IDs are versioned; decoders reject unknown IDs. This is an LZ4 block with
explicit external history, not a standalone LZ4 frame.
The seed is a fixed handwritten vocabulary, not a corpus-trained dictionary;
its bytes are compiled into the codec and are not repeated in each file.

The encoder uses a bounded 8 KiB match table and bounded candidate search. There
is no new dependency, runtime training or dictionary negotiation. Decoding first
validates token lengths, offsets, overlap, terminal-block constraints and exact
output size without allocating expanded storage, then expands into one buffer.
Unknown modes, overlong varints, invalid history, trailing data and truncated
records fail. Tests include independent seeded/overlap vectors, every prefix
truncation, mutations, random inputs and window-boundary cases.
The final sequence contains only literals and has a zero match nibble. It has
at least five literals except for a sole short sequence; the last match begins
at least twelve decoded bytes before EOF. These constraints are checked before
the expanded buffer is allocated. The pack record carries no checksum.

Compressed metadata expansion is capped at 16 MiB per identity; the remux caller
may choose a lower cap. Streaming readers also reject serialized compressed
records above `2 × 16 MiB + 64` before reading their bodies. Duplicate original
identities are rejected before expansion. This does not introduce an aggregate
limit for all existing raw or unknown metadata in the ordinary container API.

The structured metadata record `sMDT` ([SPEC.md 1.5](../SPEC.md)) embeds the
same pack record directly rather than going through `mCMP`: decoders reject
an `mCMP` whose inner identity is not `iCCP`, `eXIf` or `xMPp` as corrupt,
so wrapping a new identity in `mCMP` would make existing decoders fail the
whole file.

Compress a primary **before** building its plane set. Remuxing a file with
`pDAT`, `pIDX` or `sLOC` is rejected because moving bytes would invalidate its
absolute offsets. Build a fresh plane set to rewrite such a file. Container
framing is checked by the remuxer; closed native image entropy is not decoded or
validated by this metadata operation.
