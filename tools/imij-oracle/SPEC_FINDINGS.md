# Specification findings from the independent oracle

Written while implementing `imij-oracle` from `SPEC.md` alone. Each finding
names the SPEC section, what the text says, what the reference wire does, and
the smallest stream that shows it. The oracle follows the reference where the
text allows it; where the text does not, the mismatch is recorded here for a
SPEC (or reference) fix.

These findings are queued for a SPEC-fix lane (root, 2026-10-10); this lane
does not edit SPEC.md. The oracle already follows the landed SPEC 0.3 error
classes (map modes and CICP `full_range` are closed fields, so unknown
values there are `Corrupt`).

## 1. Every slot model carries the flag bit (SPEC 3.3)

SPEC 3.3 lists the model fields as `k` (5 bits), `m` (3 bits), `cross`
(1 bit, "only slots 1 and 2 of RGB/RGBA"), `ctx_shift` (5 bits when the slot
depth is over 16), `ctx_map`, then the tables.

The reference writes the 1-bit flag for **every** slot's model, including
gray, alpha and the lo words of 64-bit samples, where its value is 0 and it
is unused. Reading it only for cross-capable slots desyncs the models bit
stream by one bit per non-cross slot.

Smallest stream: `geometry-0-c0-s0-k0.imi` (1x1 gray). Its models bytes are
`40 00 00 00 00 00 60`; a parse with the unconditional bit consumes exactly
56 bits, a parse with a conditional bit leaves 5 extra bits and reads a
nonsense table. With the unconditional bit, the whole fixtures set (3,043
lossless conformance streams) decodes to the sealed digests, including
`geometry-10-c0-s0-k0.imi`, whose single full-image run token decodes to
symbol 34 (run length 764) with the table ending exactly on the last bit of
the record.

Suggested fix: state that the bit is always written and is reserved (0)
outside slots 1 and 2 of RGB/RGBA. **Resolved** (2026-10-22, `5f554443`):
SPEC 3.3 now says "written in every model; may be 1 only in slots 1 and 2"
with validity and padding sections.

## 2. The models bit stream is padded to a byte boundary

SPEC 2 says `models_len` bounds the models bit stream; it does not say the
stream ends on a byte boundary. Reference writers pad the stream with zero
bits; after the last table of the last model (or palette model) there are
0-7 such bits, and `models_len` counts them. A decoder that insists the bit
stream end exactly after the last table rejects every file.

Smallest stream: `geometry-0-c0-s0-k0.imi` has one spare zero bit after the
last table; `geometry-10-c0-s0-k0.imi` has seven. Decoders should skip the
remaining bits without interpreting them. **Resolved** (2026-10-22, `5f554443`):
SPEC 2 now says "writers pad the last byte with zero bits, decoders ignore
what follows the last model" and SPEC 3.3 has a Padding paragraph.

## 3. The inter-reference "reduce" step is undefined (SPEC section 4)

Inter frames (`fRAM` presence bit 0) may use predictors 7-9. SPEC section 4
defines the reference value as `R = reduce(order(sample)) mod 2^n` "with this
slot's channel map", but section 2.1 defines only the forward expansions;
no inverse (`reduce`) is specified for any map mode. The oracle implements
the obvious inverse (identity for Lsb, `>> (D - bits)` for Shift and
Replicate, the round-half-up inverse for Scale, `(o - value) mod 2^32` for
Offset, `>> (D - bits)` for FloatShift) and rejects maps 5 and 6 there, but
the wire behavior is only pinned down by the reference implementation. No
sealed fixture uses predictors 7-9 (the animation fixtures use 0-4), so this
path is unverified by the oracle's gate. Suggested fix: define `reduce` per
mode, and say what an encoder may pick for a channel whose map has no exact
inverse for the reference sample. **Resolved** (2026-10-22, `5f554443`):
SPEC 4 now defines `reduce(o)` for every map with a full table including
F16InF32, UnormF32 and non-representable values. The oracle still rejects
maps 5 (F16InF32) and 6 (UnormF32) there.

## 4. Const values are order-domain values (SPEC 2.1, section 4)

SPEC 2.1's table row says `Const` expands to `x = value`, and section 4 says
to "apply the order map inverse" after the map. For an f16/f32 channel the
two readings differ: the wire stores the value in the order domain (so a
`Const` f16 of `0x8000` on the wire decodes to the raw half `0xFFFF`).
Smallest stream: `geometry-0-c1-s2-k1.imi` (1x1 f16 GrayAlpha, alpha Const
`0x8000`; expected alpha bits `FF FF`). The oracle applies the order inverse;
the text is consistent with this but the table row alone is not. A
clarifying half-sentence would help implementers. **Resolved** (2026-10-22,
`5f554443`): SPEC 2.1 now says "an order-domain value like every expansion
here" and adds a Validity block explaining the f16 example.

## 5. SPEC rules added after the oracle was written (2026-10-11)

The format-freeze fixes made SPEC say more than this oracle checks:

- SPEC 3.2's run cap: a model that gives weight to a run token whose runs
  exceed 2^15 positions is `Corrupt` when read. The oracle reads such a
  model and decodes the token (its scalar path uses the token's real
  extra-bit width); it does not reject the model yet.
- SPEC 3.1's lane rule (a run may not reach past its lane's last position
  in the unit) matches what the oracle already does: it reports "zero run
  past the end of lane" after the unit's last row.
- SPEC 4's `reduce` for maps 5 and 6 (finding 3): the oracle still rejects
  them.

None of these occurs in the sealed sets, so `just oracle-check` is
unaffected.

## 6. Smaller notes (no action required)

- The unit table hint's span entries and the "every decoder checks it" rule
  are implementable exactly as written.
- Palette stripes, the stream-length-then-streams order, the lane and
  context rules, and the reconstruction formulas matched the reference on
  the first implementation that followed the text.
- The edge rule "at `x = 0` (row > 0): `W = NW = N`" matters for every
  predictor that reads `W` and is easy to miss; the text is correct.
