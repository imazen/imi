"""Structural witnesses with opaque, deliberately NON-codec model/entropy blobs.

These do not encode/decode pixels. Expected geometry/order is specified here
independently of the decoder's schedule. Negative witnesses exercise the draft's
proof obligations, not relaxed product tests.
"""
import struct
import unittest

from decoder import (Corrupt, Image, IndexEntry, LimitExceeded, Reader, SpecGap, State,
                     effective_scale, probe_selected_model, read_index,
                     read_setup, resolve_tiles, validated_model_lookup, walk_data)


def u(value):
    out = bytearray()
    while value >= 128:
        out.append((value & 127) | 128)
        value >>= 7
    return bytes(out + bytes([value]))


def fields(*values):
    return b''.join(u(v) for v in values)


def blob(raw):
    return u(len(raw)) + raw


def record(kind, raw):
    return u(kind) + blob(raw)


def setup(rows=1, cols=0, coder=0, coverage=b'\x00\x01', control=0,
          numeric=((0, 16),), settings=b'\x00\x00', extensions=b''):
    quantizer = b'\x00' + struct.pack('<f', 1)
    body = (fields(rows, cols, coder) + blob(quantizer) + fields(control, 0)
            + blob(settings) + blob(coverage) + fields(0, len(numeric))
            + b''.join(fields(*plane) for plane in numeric) + blob(extensions))
    return blob(body)


def definition(role, key=0, lifetime=0, target=0, raw=b'OPAQUE-MODEL'):
    return record(63, fields(role, key, lifetime)
                  + (u(target) if lifetime else b'') + u(0) + blob(raw))


def unit(role, identity, model=0, coder=0, settings=b'', entropy=b'OPAQUE-ENTROPY'):
    prefix = u(identity) + (blob(settings) if role == 1 else b'') + u(model)
    if role == 1 and coder == 0:
        prefix += u(0)
    return record({0: 51, 1: 55, 2: 53, 3: 57}[role], prefix + entropy)


def roots(alpha=False):
    return definition(0) + definition(1) + (definition(2) if alpha else b'')


class DecoderWitnesses(unittest.TestCase):
    def test_range_settings_are_local_strict_and_allow_sparse_overrides(self):
        # The first merge bit selects the upper interval; all three tree
        # bits are zero. The lower endpoint is 0x7ffff800.
        encoded = fields(3, 0) + blob(bytes.fromhex("7f ff f8 00"))
        state = State(0, (0,) * 12, (32,) * 4)
        features = {'superblocks'}
        self.assertEqual(resolve_tiles(encoded + fields(0), {}, state, 2, 2, features),
                         (State(1, (0,) * 12, (32,) * 4),))
        override = fields(1, 0, 1, 4, 64)
        self.assertEqual(resolve_tiles(encoded + override, {}, state, 2, 2, features),
                         (State(1, (0,) * 12, (64,) * 4),))
        self.assertEqual(resolve_tiles(b'', {}, state, 2, 2, features), (state,))
        for raw, width, admitted in (
                (encoded + fields(0), 1, features),
                (encoded + fields(0), 2, set()),
                (encoded[:-1], 2, features),
                (fields(3, 0) + blob(bytes.fromhex("7f ff f8 01")) + fields(0), 2, features),
                (fields(3, 0) + blob(bytes.fromhex("7f ff f8 00 00")) + fields(0), 2, features)):
            with self.assertRaises(Corrupt):
                resolve_tiles(raw, {}, state, width, 2, admitted)

    def test_both_coders_and_alpha_preserve_work_order(self):
        for coder in (0, 3):
            data = (setup(coder=coder, numeric=((0, 16), (2, 8))) + roots(True)
                    + unit(0, 0) + unit(2, 0) + unit(1, 0, coder=coder))
            work = list(walk_data(data, Image(8, 8, alpha=True)))
            self.assertEqual([(w.role, w.unit, w.restart) for w in work],
                             [(0, 0, None), (2, 0, None), (1, 0, 0 if coder == 0 else None)])

    def test_independent_column_dc_spans_and_odd_edges(self):
        # Two columns, three AC stripes. Left [1,2], right [2,1].
        data = setup(rows=2, cols=16, coverage=fields(1, 2, 2, 1, 2, 2, 2, 1)) + roots()
        expected = [(0, 0), (1, 0), (0, 1), (1, 1), (0, 2), (1, 2),
                    (1, 3), (1, 4), (0, 5), (1, 5)]
        data += b''.join(unit(role, identity) for role, identity in expected)
        work = list(walk_data(data, Image(25, 33)))
        self.assertEqual([(w.role, w.unit) for w in work], expected)
        self.assertEqual([w.span for w in work if w.role == 0],
                         [(0, 1, 0), (0, 2, 1), (1, 3, 0), (2, 3, 1)])

    def test_transient_expiry_and_no_id_reuse(self):
        initial = (setup() + roots() + definition(0, 9, 1, 0)
                   + unit(0, 0, 9) + unit(1, 0))
        self.assertEqual(len(list(walk_data(initial, Image(8, 8)))), 2)
        for suffix in [unit(0, 1, 9) + unit(1, 1),
                       definition(0, 9) + unit(0, 1, 9) + unit(1, 1)]:
            with self.assertRaises(Corrupt):
                list(walk_data(initial + suffix, Image(8, 16)))

    def test_two_transients_cannot_be_batched_before_a_group(self):
        data = (setup() + roots() + definition(0, 1, 1, 0)
                + definition(1, 1, 1, 0) + unit(0, 0, 1) + unit(1, 0, 1))
        with self.assertRaisesRegex(Corrupt, 'two transient'):
            list(walk_data(data, Image(8, 8)))

    def test_transient_requires_next_main_role_target_and_use(self):
        for definition_bytes, use in [(definition(1, 1, 1, 0), unit(0, 0)),
                                      (definition(0, 1, 1, 1), unit(0, 0, 1)),
                                      (definition(0, 1, 1, 0), unit(0, 0, 0))]:
            data = setup() + roots() + definition_bytes + use + unit(1, 0)
            with self.assertRaisesRegex(Corrupt, 'transient next-main'):
                list(walk_data(data, Image(8, 8)))

    def test_roots_before_pixels_and_no_forward_references(self):
        cases = [setup() + definition(0) + unit(0, 0) + definition(1) + unit(1, 0),
                 setup() + roots() + unit(0, 0, 4) + definition(0, 4) + unit(1, 0),
                 setup() + roots() + unit(0, 1) + unit(1, 0)]
        for data in cases:
            with self.assertRaises(Corrupt):
                list(walk_data(data, Image(8, 8)))

    def test_exact_descriptor_stops_at_spec_gap(self):
        data = setup(control=2, numeric=((0, 16), (3, 16)))
        with self.assertRaisesRegex(SpecGap, 'F1'):
            read_setup(Reader(data).blob(), Image(16, 16))
        # Four packed blocks, two per row: height 16. Removed stripe height
        # could be 8 (two byte lengths) or 16 (one); B/counts do not choose it.
        stripe_counts = [(16 + hs - 1) // hs for hs in (8, 16)]
        self.assertEqual(stripe_counts, [2, 1])

    def test_extension_and_side_rules_are_not_silently_guessed(self):
        with self.assertRaisesRegex(SpecGap, 'F2'):
            read_setup(Reader(setup(extensions=record(21, b''))).blob(), Image(8, 8))
        data = setup() + roots() + record(70, b'') + unit(0, 0) + unit(1, 0)
        with self.assertRaisesRegex(SpecGap, 'F3'):
            list(walk_data(data, Image(8, 8)))

    def test_indexed_local_check_does_not_prove_definition_uniqueness(self):
        prefix = setup() + roots() + definition(1, 7, raw=b'FIRST')
        selected = definition(1, 7, raw=b'SECOND')
        data = prefix + selected + unit(0, 0) + unit(1, 0, 7)
        entry = IndexEntry(1, 1, 7, len(prefix), len(prefix) + len(selected))
        # Deliberately incomplete probe accepts its local ID and extent.
        self.assertEqual(bytes(probe_selected_model(data, entry).raw), b'SECOND')
        # The actual stream grammar rejects that same DATA before success.
        with self.assertRaisesRegex(Corrupt, 'redefinition'):
            list(walk_data(data, Image(8, 8)))
        with self.assertRaises(Corrupt):
            validated_model_lookup(data, Image(8, 8), entry)

    def test_index_cannot_reinterpret_entropy_bytes_as_a_definition(self):
        prefix = setup() + roots()
        real = definition(1, 7, raw=b'REAL')
        real_entry = IndexEntry(1, 1, 7, len(prefix), len(prefix) + len(real))
        prefix += real + unit(0, 0)
        fake = definition(1, 7, raw=b'INSIDE-OPAQUE-ENTROPY')
        data = prefix + unit(1, 0, 7, entropy=fake)
        # It is an embedded byte sequence, not a record in the DATA traversal.
        at = len(data) - len(fake)
        fake_entry = IndexEntry(1, 1, 7, at, len(data))
        self.assertEqual(len(list(walk_data(data, Image(8, 8)))), 2)
        self.assertEqual(bytes(probe_selected_model(data, fake_entry).raw),
                         b'INSIDE-OPAQUE-ENTROPY')
        with self.assertRaisesRegex(Corrupt, 'actual definition boundary'):
            validated_model_lookup(data, Image(8, 8), fake_entry)
        self.assertEqual(bytes(validated_model_lookup(data, Image(8, 8), real_entry).raw),
                         b'REAL')

    def test_index_entries_and_selected_identity_are_checked(self):
        raw = fields(1, 1, 1, 7, 9, 5) + (20).to_bytes(8, 'little')
        entries, size = read_index(raw)
        self.assertEqual((entries[0].start, entries[0].end, size), (9, 14, 20))
        for invalid in [fields(2, 1, 1, 7, 0, 3, 1, 1, 7, 0, 3)
                        + (6).to_bytes(8, 'little'),
                        fields(1, 1, 1, 7, 9, 5) + (13).to_bytes(8, 'little')]:
            with self.assertRaises(Corrupt):
                read_index(invalid)
        model = definition(0, 7)
        with self.assertRaisesRegex(Corrupt, 'identity'):
            probe_selected_model(model, IndexEntry(1, 1, 7, 0, len(model)))

    def test_header_default_and_odd_tile_edge_override(self):
        merged = State(partition=1)
        with self.assertRaisesRegex(Corrupt, 'unit edge'):
            resolve_tiles(b'', {}, merged, 3, 2, {'superblocks'})
        # Inherit group; override tile 1 to partition 0. Tile 0 stays 16x16.
        states = resolve_tiles(bytes.fromhex('00 01 01 01 01 00'), {}, merged,
                               3, 2, {'superblocks'})
        self.assertEqual([s.partition for s in states], [1, 0])

    def test_default_restore_and_bypass_need_decoded_dc(self):
        state = State(scales=(128, 0, 64, 0))
        self.assertEqual([effective_scale(state, p, 32) for p in range(4)],
                         [128, 32, 64, 32])
        self.assertIsNone(effective_scale(state, 0, 32, bypass=True))

    def test_sparse_full_width_ids_and_varint_admission(self):
        data = (setup() + roots() + definition(1, (1 << 64) - 1)
                + unit(0, 0) + unit(1, 0, (1 << 64) - 1))
        self.assertEqual(list(walk_data(data, Image(8, 8)))[1].model.key, (1 << 64) - 1)
        for malformed in [b'\x80\x00', b'\x80' * 10, b'\xff' * 9 + b'\x02']:
            with self.assertRaises(Corrupt):
                Reader(malformed).uint()
        with self.assertRaisesRegex(LimitExceeded, 'unit-count budget'):
            read_setup(Reader(setup(cols=8)).blob(), Image(40000, 8))

    def test_truncation_wrong_role_and_trailing_records_fail(self):
        data = setup() + roots() + unit(0, 0) + unit(1, 0)
        for bad in [data[:-1], data + definition(1, 2),
                    setup() + roots() + unit(1, 0) + unit(0, 0)]:
            with self.assertRaises(Corrupt):
                list(walk_data(bad, Image(8, 8)))


if __name__ == '__main__':
    unittest.main()
