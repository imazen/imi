#!/usr/bin/env python3
"""Byte census of a native-validated baseline numeric V4 image at offset zero.

Accepts its one-entry archive as well. This counts existing syntax; it is not
an integrity verifier, pixel decoder or general outer-container parser.
"""
import argparse
import json
from collections import Counter
from pathlib import Path

class Cursor:
    def __init__(self, data):
        self.data, self.at = data, 0
    def take(self, count):
        if count < 0 or self.at + count > len(self.data):
            raise ValueError('truncated census input')
        result = self.data[self.at:self.at+count]
        self.at += count
        return result
    def varint(self):
        value = 0
        for i in range(10):
            b = self.take(1)[0]
            if i == 9 and b > 1:
                raise ValueError('varint overflow')
            value |= (b & 127) << (7*i)
            if not b & 128:
                if i and value < 1 << (7*i):
                    raise ValueError('noncanonical varint')
                return value
        raise ValueError('unterminated varint')

def census(data):
    outer = Cursor(data)
    assert outer.take(8) == b'IMIJ\r\n\x87\n'
    header = Cursor(outer.take(outer.varint()))
    assert header.take(1) == b'\x04', 'V4 required'
    assert header.varint() == 0, 'census is limited to the baseline numeric witness'
    width, height = header.varint(), header.varint()
    assert header.at == len(header.data)
    header_bytes = outer.at
    assert outer.take(4) == b'DATA', 'standalone witness without optional chunks required'
    payload = Cursor(outer.take(outer.varint()-1))
    data_framing_bytes = outer.at - header_bytes - len(payload.data)
    setup = payload.take(payload.varint())
    setup_bytes = payload.at
    semantic = Cursor(setup)
    tile_cols, tile_rows, order, mode = [semantic.varint() for _ in range(4)]
    assert mode in (0, 1), 'aggregate-tile census is not implemented'
    assert payload.varint() == 2, 'front directory required'
    capacity = payload.varint()
    slot = Cursor(payload.take(capacity))
    directory = Cursor(slot.take(slot.varint()))
    padding = slot.take(len(slot.data)-slot.at)
    assert not any(padding)
    directory_prefix_bytes = payload.at - setup_bytes
    count = directory.varint()
    entries_start = directory.at
    entries = []
    directory_classes = Counter()
    for _ in range(count):
        before = directory.at
        entry = [directory.varint() for _ in range(4)]
        entries.append(entry)
        directory_classes[entry[0]] += directory.at-before
    entries_bytes = directory.at-entries_start
    objects_len = int.from_bytes(directory.take(8), 'little')
    assert directory.at == len(directory.data)
    assert objects_len == sum(entry[3] for entry in entries)
    parts = Counter(setup=setup_bytes, directory_prefix=directory_prefix_bytes)
    kinds = Counter()
    for kind, role, identifier, length in entries:
        assert role == 0
        record = Cursor(payload.take(length))
        tag, body_len = record.varint(), record.varint()
        parts['object_framing'] += record.at
        trailer = 16 if mode else 0
        body = Cursor(record.take(body_len-trailer))
        record.take(trailer)
        parts['integrity_trailers'] += trailer
        assert record.at == len(record.data)
        assert body.varint() == identifier
        kinds[tag] += 1
        if tag == 59:
            assert kind == 0
            parts['unit_identifiers'] += body.at
            parts['numeric_entropy'] += len(body.data)-body.at
        else:
            assert (tag,kind) in [(65,2),(67,3)]
            parts['tile_controls'] += len(body.data)
    assert payload.at == len(payload.data)
    assert sum(parts.values()) == len(payload.data)
    return dict(scope='byte census only; input requires separate native validation',
                width=width,height=height,tile_columns=tile_cols,tile_rows=tile_rows,order=order,
                image_bytes_through_DATA=outer.at,header_bytes=header_bytes,
                data_chunk_framing_bytes=data_framing_bytes,data_payload_bytes=len(payload.data),
                active_directory_bytes=len(directory.data),directory_entries=count,
                directory_entry_bytes=entries_bytes,directory_entry_class_bytes=dict(directory_classes),
                directory_padding_bytes=len(padding),
                object_counts=dict(kinds),payload_components=dict(parts))

if __name__ == '__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('input',type=Path)
    p.add_argument('output',type=Path)
    a=p.parse_args()
    result=census(a.input.read_bytes())
    with a.output.open('x') as f:json.dump(result,f,indent=2);f.write('\n')
    print(json.dumps(result))
