#!/usr/bin/env python3
"""Executable V4 structural decoder; not an entropy or pixel decoder.

Input is DATA plus an already admitted image description. Opaque model/entropy
bytes are returned explicitly, never treated as decoded samples. See README.md
for missing arithmetic and for the intentionally incomplete index proof probe.
Only the standard library is used.
"""
from dataclasses import dataclass
import math
import struct


class Corrupt(ValueError):
    pass


class Unsupported(ValueError):
    pass


class SpecGap(ValueError):
    pass


class LimitExceeded(ValueError):
    pass


class Reader:
    def __init__(self, data, base=0):
        self.data = memoryview(data)
        self.pos = 0
        self.base = base

    def take(self, size):
        if size < 0 or size > len(self.data) - self.pos:
            raise Corrupt("truncated field")
        result = self.data[self.pos:self.pos + size]
        self.pos += size
        return result

    def uint(self):
        value = 0
        for i in range(10):
            byte = self.take(1)[0]
            if i == 9 and byte > 1:
                raise Corrupt("u64 overflow")
            value |= (byte & 127) << (7 * i)
            if byte < 128:
                if i and byte == 0:
                    raise Corrupt("nonminimal varint")
                return value
        raise Corrupt("u64 overflow")

    def count(self, minimum_bytes=1):
        count = self.uint()
        if count > (len(self.data) - self.pos) // minimum_bytes:
            raise Corrupt("count exceeds enclosing bytes")
        return count

    def blob(self):
        size = self.uint()
        base = self.base + self.pos
        return Reader(self.take(size), base)

    def done(self):
        if self.pos != len(self.data):
            raise Corrupt("trailing bytes")

    def record(self):
        start = self.base + self.pos
        kind = self.uint()
        body = self.blob()
        return kind, body, start, self.base + self.pos

    def f32(self):
        value = struct.unpack('<f', self.take(4))[0]
        if not math.isfinite(value):
            raise Corrupt("nonfinite quantizer parameter")
        return value


@dataclass(frozen=True)
class Image:
    width: int
    height: int
    alpha: bool = False
    # Outer-header admission, color/precision and reference admission are inputs.
    features: frozenset = frozenset()


@dataclass(frozen=True)
class State:
    partition: int = 0
    plans: tuple = (0,) * 12
    scales: tuple = (0,) * 4


def read_delta(r):
    mask = r.uint()
    if mask & ~7:
        raise Unsupported("settings field")
    delta = {}
    if mask & 1:
        value = r.uint()
        if value > 7:
            raise Corrupt("partition")
        delta['partition'] = value
    if mask & 2:
        edits = r.uint()
        if edits > 4095:
            raise Corrupt("plane-plan mask")
        plans = {}
        for i in range(12):
            if edits & (1 << i):
                plan = r.uint()
                if plan > 13 or plan == 10:
                    raise Unsupported("plane plan")
                plans[i] = plan
        delta['plans'] = plans
    if mask & 4:
        value = r.uint()
        if value <= 255:
            scales = (value,) * 4
        elif value == 256:
            scales = tuple(r.uint() for _ in range(4))
            if any(v > 255 for v in scales):
                raise Corrupt("parent AC multiplier")
        else:
            raise Unsupported("AC multiplier form")
        delta['scales'] = scales
    return delta


def apply(state, delta):
    plans = list(state.plans)
    for i, value in delta.get('plans', {}).items():
        plans[i] = value
    return State(delta.get('partition', state.partition), tuple(plans),
                 delta.get('scales', state.scales))


def read_settings(r):
    sets = {}
    previous = 0
    for _ in range(r.count(2)):
        key = r.uint()
        if key <= previous:
            raise Corrupt("settings IDs not increasing/nonzero")
        previous = key
        sets[key] = read_delta(r)
    image = apply(State(), read_delta(r))
    r.done()
    return sets, image


def select(r, parent, sets):
    kind = r.uint()
    if kind == 0:
        return parent
    if kind == 1:
        return apply(parent, read_delta(r))
    if kind == 2:
        key = r.uint()
        if key not in sets:
            raise Corrupt("missing settings ID")
        return apply(parent, sets[key])
    raise Unsupported("settings selector")


# Parent positions are TL, TR, BL, BR. These footprints come from SPEC 2.9.1.
MERGES = ((), ((0, 1, 2, 3),), ((0, 1),), ((2, 3),),
          ((0, 1), (2, 3)), ((0, 2),), ((1, 3),), ((0, 2), (1, 3)))


def range_partitions(r, block_width, block_rows):
    """Strict V4 partition map, with the historical adaptive symbol model."""
    code = int.from_bytes(r.take(4), 'big')
    interval = 0xffffffff
    merged = [2048] * 9
    tree = [[2048] * 9 for _ in range(8)]

    def bit(probabilities, ctx):
        nonlocal code, interval
        p = probabilities[ctx]
        bound = (interval >> 12) * p
        one = code >= bound
        if one:
            code -= bound
            interval -= bound
            probabilities[ctx] -= p >> 5
        else:
            interval = bound
            probabilities[ctx] += (4096 - p) >> 5
        while interval < 1 << 24:
            interval <<= 8
            code = ((code << 8) | r.take(1)[0]) & 0xffffffff
        return int(one)

    grid = []
    columns = (block_width + 1) // 2
    for y in range(0, block_rows, 2):
        for x in range(0, block_width, 2):
            left = grid[-1] if x else 0
            above = grid[-columns] if y else 0
            ctx = min(left, 2) * 3 + min(above, 2)
            width, height = block_width - x, block_rows - y
            symbol = 0
            if (width > 1 or height > 1) and bit(merged, ctx):
                node = 1
                for _ in range(3):
                    node = 2 * node + bit(tree[node], ctx)
                symbol = node - 7
                if symbol == 8:
                    raise Corrupt("invalid range partition")
            if any(p % 2 >= width or p // 2 >= height
                   for block in MERGES[symbol] for p in block):
                raise Corrupt("range partition crosses unit edge")
            grid.append(symbol)
    r.done()
    if code or not any(grid):
        raise Corrupt("range partition termination")
    return grid


def resolve_tiles(raw, sets, image_state, block_width, block_rows, features):
    r = Reader(raw)
    group = image_state
    overrides = {}
    partitions = None
    count = ((block_width + 1) // 2) * ((block_rows + 1) // 2)

    def inherited(i):
        if partitions is None:
            return group
        return State(partitions[i], group.plans, group.scales)

    if len(raw):
        dense = r.data[0] == 3
        if dense:
            r.take(1)
        group = select(r, image_state, sets)
        if dense:
            if 'superblocks' not in features:
                raise Corrupt("range map without capability")
            part = r.blob()
            if not 4 <= len(part.data) <= 4 * count + 4:
                raise Corrupt("range map length")
            partitions = range_partitions(part, block_width, block_rows)
        previous = -1
        for _ in range(r.count(2)):
            tile = r.uint() + previous + 1
            if tile >= count:
                raise Corrupt("tile outside unit")
            overrides[tile] = select(r, inherited(tile), sets)
            previous = tile
        r.done()
    result = []
    for y in range(0, block_rows, 2):
        for x in range(0, block_width, 2):
            state = overrides.get(len(result), inherited(len(result)))
            present = {i for i, (dx, dy) in enumerate(((0, 0), (1, 0), (0, 1), (1, 1)))
                       if x + dx < block_width and y + dy < block_rows}
            if state.partition and 'superblocks' not in features:
                raise Corrupt("merge without capability")
            for merged in MERGES[state.partition]:
                if not set(merged) <= present:
                    raise Corrupt("merge crosses unit edge")
                if any(state.plans[p * 3 + c] for p in merged for c in range(3)):
                    raise Corrupt("merged parent has another plane plan")
            for p in present:
                for plan in state.plans[p * 3:p * 3 + 3]:
                    if plan == 1 and 'small_transforms' not in features:
                        raise Corrupt("small transform without capability")
                    if plan >= 2 and 'channel_layouts' not in features:
                        raise Corrupt("channel layout without capability")
            result.append(state)
    return tuple(result)


def effective_scale(state, parent, numeric_aq, bypass=False):
    """Post-DC operation. None means exact/keep, never a quantizer value."""
    if bypass:
        return None
    code = state.scales[parent]
    return code if code else max(0, min(255, numeric_aq))


@dataclass
class Setup:
    image: Image
    ac_rows: int
    cols: int
    coder: int
    control: int
    storage: int
    sets: dict
    state: State
    spans: tuple
    banks: frozenset
    roles: frozenset
    quantizer: dict
    lanes: dict

    @property
    def ng(self):
        return (self.image.width + self.cols - 1) // self.cols

    @property
    def stripes(self):
        return ((self.image.height + 7) // 8 + self.ac_rows - 1) // self.ac_rows

    def schedule(self):
        for s in range(self.stripes):
            for g in range(self.ng):
                unit = s * self.ng + g
                a, b = next((a, b) for a, b in self.spans[g] if a <= s < b)
                if a == s:
                    yield 0, unit, (a, b, g)
                if 2 in self.roles:
                    yield 2, unit, (s, s + 1, g)
                yield 1, unit, (s, s + 1, g)
                if 3 in self.roles:
                    yield 3, unit, (s, s + 1, g)


def read_coverage(r, stripes, ng):
    form = r.uint()
    if form == 0:
        length = r.uint()
        if not length:
            raise Corrupt("zero DC coverage")
        lists = [[(a, min(a + length, stripes)) for a in range(0, stripes, length)]]
    elif form == 1:
        n = r.count()
        if n not in (1, ng):
            raise Corrupt("DC column-list count")
        lists = []
        for _ in range(n):
            spans = []
            a = 0
            count = r.count()
            if count > stripes:
                raise Corrupt("too many DC spans")
            for _ in range(count):
                length = r.uint()
                if not length or a + length > stripes:
                    raise Corrupt("DC coverage extent")
                spans.append((a, a + length))
                a += length
            if a != stripes:
                raise Corrupt("incomplete DC coverage")
            lists.append(spans)
    else:
        raise Unsupported("coverage form")
    r.done()
    return tuple(tuple(lists[0] if len(lists) == 1 else lists[g]) for g in range(ng))


def read_setup(r, image, known_banks=(), max_units=4096):
    if image.width <= 0 or image.height <= 0:
        raise Corrupt("image geometry")
    rows, cols, coder = r.uint(), r.uint(), r.uint()
    if not rows or (cols and (cols % 8 or cols >= image.width)):
        raise Corrupt("AC unit geometry")
    if coder not in (0, 3):
        raise Unsupported("AC coder")
    cols = cols or image.width
    q = r.blob()
    mask, scale = q.uint(), q.f32()
    if mask & ~31:
        raise Unsupported("quantizer field")
    quantizer = {'quant_scale': scale}
    if mask & 1:
        quantizer['matrix'] = q.uint()
        if quantizer['matrix'] not in (0, 1, 2, 3, 4, 5, 63):
            raise Unsupported("quantization matrix")
    if mask & 2:
        quantizer['dc'] = tuple(q.uint() for _ in range(3))
        if any(not 1 <= v <= 255 for v in quantizer['dc']):
            raise Corrupt("DC step code")
    for bit, name, n in [(2, 'floor', 1), (3, 'scale', 3), (4, 'mul', 3)]:
        if mask & (1 << bit):
            quantizer[name] = tuple(q.f32() for _ in range(n))
    q.done()  # Normative quantizer math/bounds are an explicit arithmetic hook.
    control, storage = r.uint(), r.uint()
    if control > 4 or storage > 1:
        raise Unsupported("block control or DC storage")
    sets, state = read_settings(r.blob())
    stripes = ((image.height + 7) // 8 + rows - 1) // rows
    ng = (image.width + cols - 1) // cols
    if stripes * ng > max_units:
        raise LimitExceeded("audit unit-count budget")
    spans = read_coverage(r.blob(), stripes, ng)
    banks = []
    for _ in range(r.count()):
        bank = r.uint()
        if bank <= (banks[-1] if banks else 0):
            raise Corrupt("bank IDs")
        if bank not in known_banks:
            raise Unsupported("bank")
        banks.append(bank)
    lanes = {}
    expected_numeric = {0} | ({2} if image.alpha else set()) | ({3} if control == 2 else set())
    previous = -1
    for _ in range(r.count(2)):
        role, width = r.uint(), r.uint()
        if role <= previous or role not in expected_numeric or not 1 <= width <= 16:
            raise Corrupt("numeric plane descriptor")
        lanes[role], previous = width, role
        if role == 3:
            raise SpecGap('F1: packed exact stripe height/cardinality is missing')
    if set(lanes) != expected_numeric:
        raise Corrupt("numeric planes missing")
    extensions = r.blob()
    if len(extensions.data):
        raise SpecGap('F2: retained extension whitelist and interpretation need a table')
    r.done()
    return Setup(image, rows, cols, coder, control, storage, sets, state, spans,
                 frozenset(banks), frozenset(expected_numeric | {1}), quantizer, lanes)


@dataclass(frozen=True)
class Definition:
    role: int
    key: int
    lifetime: int
    target: int | None
    bank: int
    raw: memoryview
    start: int
    end: int


def read_definition(r, start, end):
    role, key, lifetime = r.uint(), r.uint(), r.uint()
    if role > 3 or lifetime > 1:
        raise Unsupported("definition role/lifetime")
    target = r.uint() if lifetime else None
    bank, body = r.uint(), r.blob()
    if not len(body.data):
        raise Corrupt("empty model body")
    r.done()
    return Definition(role, key, lifetime, target, bank, body.data, start, end)


class Models:
    def __init__(self, setup, limit):
        self.setup, self.limit = setup, limit
        self.entries, self.highwater = {}, {}
        self.roots = set()
        self.pending = None
        self.count = self.bytes = 0

    def define(self, d, before_first):
        if d.role not in self.setup.roles or (d.bank and d.bank not in self.setup.banks):
            raise Corrupt("definition role/bank not declared")
        if d.bank and d.role == 3:
            raise Unsupported("exact seed role")
        if d.key <= self.highwater.get(d.role, -1):
            raise Corrupt("model ID redefinition/nonmonotonic ID")
        if d.key == 0:
            if not before_first or d.lifetime:
                raise Corrupt("root placement/lifetime")
            self.roots.add(d.role)
        elif d.role not in self.roots:
            raise Corrupt("nonroot before its root")
        if self.count + 1 > self.limit:
            raise LimitExceeded("audit definition-count budget")
        if d.lifetime:
            if self.pending is not None:
                raise Corrupt("two transient definitions require the same next main unit")
            self.pending = d
        self.count += 1
        self.bytes += len(d.raw)  # Compressed bytes only, NOT expanded/RSS admission.
        self.highwater[d.role] = d.key
        self.entries[d.role, d.key] = d

    def use(self, role, key, unit, position):
        if self.roots != set(self.setup.roles):
            raise Corrupt("roots missing before first main unit")
        if self.pending is not None:
            d = self.pending
            if (role, unit, key) != (d.role, d.target, d.key):
                raise Corrupt("transient next-main target/use mismatch")
        d = self.entries.get((role, key))
        if d is None or d.start >= position:
            raise Corrupt("undefined, expired or forward model")
        return d

    def finish_unit(self):
        if self.pending is not None:
            self.entries.pop((self.pending.role, self.pending.key))
            self.pending = None


@dataclass(frozen=True)
class Work:
    role: int
    unit: int
    span: tuple
    model: Definition
    tiles: tuple
    restart: int | None
    entropy: memoryview


KINDS = {0: 51, 1: 55, 2: 53, 3: 57}


def walk_data(data, image, known_banks=(), max_bytes=16_000_000, max_models=4096,
              max_units=4096, max_parents=1_000_000, definition_sink=None):
    """Yield structural work, NOT pixels or verified model tables.

    Iteration may fail after earlier work was yielded. Only exhaustion proves
    this structural subset completed. Opaque bodies still need the kernel hooks.
    """
    if len(data) > max_bytes:
        raise LimitExceeded("audit input-byte budget")
    if ((image.width + 7) // 8) * ((image.height + 7) // 8) > max_parents:
        raise LimitExceeded("audit parent-count budget")
    r = Reader(data)
    setup = read_setup(r.blob(), image, known_banks, max_units)
    models = Models(setup, max_models)
    first = True
    for role, unit, span in setup.schedule():
        while True:
            kind, body, start, end = r.record()
            if kind != 63:
                break
            definition = read_definition(body, start, end)
            models.define(definition, first)
            if definition_sink is not None:
                definition_sink(definition)
        if kind % 2 == 0:
            raise SpecGap('F3: DATA side-record placement rule is not carried over explicitly')
        if kind != KINDS[role] or body.uint() != unit:
            raise Corrupt("main unit role/order/identity")
        tiles = ()
        if role == 1:
            s, _, g = span
            bw = (min(setup.cols, image.width - g * setup.cols) + 7) // 8
            bh = min(setup.ac_rows, (image.height + 7) // 8 - s * setup.ac_rows)
            tiles = resolve_tiles(body.blob().data, setup.sets, setup.state,
                                  bw, bh, image.features)
        model = models.use(role, body.uint(), unit, start)
        restart = body.uint() if role == 1 and setup.coder == 0 else None
        if restart not in (None, 0, 1):
            raise Corrupt("AC restart")
        first = False
        # Pixel pseudocode: decode DC first, THEN resolve AQ/exact/keep legality.
        yield Work(role, unit, span, model, tiles, restart,
                   body.take(len(body.data) - body.pos))
        models.finish_unit()
    r.done()


@dataclass(frozen=True)
class IndexEntry:
    kind: int
    role: int
    key: int
    start: int
    end: int


def read_index(raw):
    """Parse the common body, without assuming it proves DATA framing."""
    r = Reader(raw)
    entries, keys, end = [], set(), 0
    for _ in range(r.count(5)):
        kind, role, key = r.uint(), r.uint(), r.uint()
        start = end + r.uint()
        size = r.uint()
        if kind > 1 or role > 3:
            raise Unsupported("index kind/role")
        identity = kind, role, key
        if identity in keys or size < 2 or start + size > (1 << 64) - 1:
            raise Corrupt("index identity/extent")
        keys.add(identity)
        end = start + size
        entries.append(IndexEntry(kind, role, key, start, end))
    data_len = int.from_bytes(r.take(8), 'little')
    if end > data_len:
        raise Corrupt("index exceeds DATA")
    r.done()
    return tuple(entries), data_len


def probe_selected_model(data, entry):
    """Deliberately incomplete audit probe, NEVER a validated ROI decoder.

    Demonstrates that a matching selected ID/extent does not prove unselected
    gaps contain only ignorable records or that the definition is unique.
    """
    r = Reader(memoryview(data)[entry.start:entry.end], entry.start)
    kind, body, start, end = r.record()
    r.done()
    if entry.kind != 1 or kind != 63:
        raise Corrupt("index definition kind")
    d = read_definition(body, start, end)
    if (d.role, d.key) != (entry.role, entry.key):
        raise Corrupt("index definition identity")
    return d


def validated_model_lookup(data, image, entry):
    """Slow structural reference: scan every record before trusting a lookup.

    No entropy decoding is performed. A real sparse implementation still needs
    a specified equivalent framing proof; a selected-entry check is insufficient.
    """
    observed = {}
    def remember(d):
        observed[d.role, d.key] = d
    for _ in walk_data(data, image, definition_sink=remember):
        pass
    d = observed.get((entry.role, entry.key))
    if entry.kind != 1 or d is None or (d.start, d.end) != (entry.start, entry.end):
        raise Corrupt("index does not name an actual definition boundary")
    return d
