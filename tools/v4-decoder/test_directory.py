import unittest
from dataclasses import replace
from decoder import Reader, Corrupt, read_definition
from directory import read_directory, bind_model, bind_unit
from test_decoder import fields, definition


class CurrentDirectoryTests(unittest.TestCase):
    def body(self, entries):
        return fields(len(entries), *(value for entry in entries for value in entry)) + sum(e[3] for e in entries).to_bytes(8, 'little')

    def entries(self):
        return [(1, 1, 0, 10), (2, 0, 0, 3), (5, 1, (1 << 64) - 1, 20),
                (4, 0, 0, 2), (0, 1, 0, 7), (3, 0, 0, 5)]

    def test_lifetime_has_the_same_four_varint_size(self):
        entries = self.entries()
        parsed, bindings = read_directory(self.body(entries), [0], lambda _: 0)
        self.assertEqual(bindings[parsed[4].start], parsed[2])
        payload = entries.copy()
        payload[2] = (1, *payload[2][1:])
        self.assertEqual(len(self.body(entries)), len(self.body(payload)))
        model = Reader(definition(1, (1 << 64) - 1, lifetime=1, target=0))
        _, body, start, end = model.record()
        d = read_definition(body, start, end)
        bind_model(parsed[2], d, parsed[4])
        bind_unit(parsed[4], d.key, bindings)
        for wrong in [replace(d, lifetime=0), replace(d, target=1)]:
            with self.assertRaises(Corrupt):
                bind_model(parsed[2], wrong, parsed[4])
        with self.assertRaisesRegex(Corrupt, 'selection'):
            bind_unit(parsed[4], 0, bindings)

    def test_pending_roots_roles_expiry_and_reuse_refuse(self):
        for kind in ('root', 'role', 'two', 'unfinished', 'late-root', 'reuse'):
            entries = self.entries()
            if kind == 'root': entries[2] = (5, 1, 0, 20)
            if kind == 'role': entries[4] = (0, 0, 0, 7)
            if kind == 'two': entries.insert(2, (5, 1, 1, 20))
            if kind == 'unfinished': entries.pop(4)
            if kind == 'late-root': entries.insert(5, (1, 2, 0, 10))
            if kind == 'reuse': entries[2] = (1, 1, 0, 20)
            with self.subTest(kind=kind), self.assertRaises(Corrupt):
                read_directory(self.body(entries), [0], lambda _: 0)


if __name__ == '__main__':
    unittest.main()
