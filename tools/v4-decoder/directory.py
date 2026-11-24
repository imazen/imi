"""Current four-varint directory control audit; no object or pixel proof.

The caller supplies admitted tile order and a group-to-tile mapping. This
checks namespace, tile framing and next-main bindings. Coverage geometry,
integrity, model expansion and entropy remain with the production reader.
"""
from decoder import Reader, IndexEntry, Corrupt, Unsupported


def read_directory(raw, tile_ids, group_tile):
    r = Reader(raw)
    count = r.uint()
    if len(raw) - r.pos < 8 or count > (len(raw) - r.pos - 8) // 4:
        raise Corrupt('directory count')
    models, units = [None] * 4, [None] * 4
    pending, tile, offset, ordinal, side = None, None, 0, 0, 0
    begun, awaiting = False, False
    entries, bindings = [], {}
    for _ in range(count):
        kind, role, key, size = (r.uint() for _ in range(4))
        if kind > 5:
            raise Unsupported('directory kind')
        if role > 3 or size < 2 or offset + size > (1 << 64) - 1:
            raise Corrupt('directory role/extent')
        entry = IndexEntry(kind, role, key, offset, offset + size)
        if kind == 2:
            if tile is not None or role or ordinal >= len(tile_ids) or key != tile_ids[ordinal]:
                raise Corrupt('tile begin')
            tile, units = key, [None] * 4
        elif kind == 3:
            if tile is None or role or key != tile:
                raise Corrupt('tile end')
            tile, ordinal = None, ordinal + 1
        elif kind in (1, 5):
            if key == 0 and (begun or kind == 5):
                raise Corrupt('root placement/lifetime')
            if (models[role] is None and key != 0) or (models[role] is not None and key <= models[role]):
                raise Corrupt('model identity/reuse')
            if kind == 5:
                if pending is not None:
                    raise Corrupt('two pending models')
                pending = entry
            models[role], awaiting = key, True
        elif kind == 0:
            if tile is None or (role and models[role] is None):
                raise Corrupt('unit before tile/root')
            if units[role] is not None and key <= units[role]:
                raise Corrupt('unit order')
            if role != 3 and group_tile(key) != tile:
                raise Corrupt('unit geometry')
            if role == 3 and key != (0 if units[role] is None else units[role] + 1):
                raise Corrupt('exact stripe order')
            if pending is not None:
                if pending.role != role:
                    raise Corrupt('next-model role')
                bindings[offset] = pending
            pending, begun, awaiting = None, True, False
            units[role] = key
        else:
            if role or key != side:
                raise Corrupt('side ordinal')
            side += 1
        entries.append(entry)
        offset += size
    objects_len = int.from_bytes(r.take(8), 'little')
    r.done()
    if tile is not None or ordinal != len(tile_ids) or offset != objects_len or awaiting or pending:
        raise Corrupt('directory coverage/unfinished model')
    return tuple(entries), bindings


def bind_model(entry, definition, target=None):
    if entry.kind not in (1, 5) or (entry.role, entry.key) != (definition.role, definition.key):
        raise Corrupt('model identity')
    if entry.kind != (5 if definition.lifetime else 1):
        raise Corrupt('model lifetime')
    if entry.kind == 5 and (target is None or (target.role, target.key) != (definition.role, definition.target)):
        raise Corrupt('model target')


def bind_unit(entry, model_id, bindings):
    pending = bindings.get(entry.start)
    if pending is not None and (pending.role, pending.key) != (entry.role, model_id):
        raise Corrupt('next-model selection')
