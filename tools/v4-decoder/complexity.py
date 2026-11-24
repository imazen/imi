#!/usr/bin/env python3
"""Reproducible structural complexity metrics, never a weighted difficulty score.

Counts come from AST syntax, explicit finite witnesses, and the reviewed rule
inventory below. Inventory grouping is a modeling choice, not a measurement of
all possible format interactions. No entropy/pixels/performance are measured.
"""
import argparse
import ast
from collections import Counter
import itertools
import json
from importlib.metadata import version
from pathlib import Path
import re
import sys

import xxhash  # Same diagnostic dependency as repository terminology checks.
import decoder


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
FEATURES = ('settings', 'transforms', 'block_control', 'coverage', 'models',
            'banks', 'coder', 'index', 'exact_packing', 'numeric_interpretation')

# One entry per identified joint rule. Transitive dependencies are not edges.
# Source anchors and explanations are part of the reviewable input model.
RULES = [
    ('R1', ('settings', 'transforms'), 'specified', 'SPEC_V4.md#3-one-settings-grammar',
     'A selected partition must fit the actual unit edges.'),
    ('R2', ('settings', 'transforms'), 'specified', 'SPEC_V4.md#3-one-settings-grammar',
     'Merged parents must use the ordinary plane plan.'),
    ('R3', ('settings', 'transforms', 'block_control'), 'specified', 'SPEC.md#29-superblocks-feature-superblocks',
     'Merged parents need equal effective AQ and must not be exact; resolve scoped override first.'),
    ('R4', ('settings', 'block_control'), 'specified', 'SPEC_V4.md#3-one-settings-grammar',
     'Exact/keep bypass precedes AC override; code zero restores numeric AQ.'),
    ('R5', ('models', 'numeric_interpretation', 'block_control'), 'specified',
     'SPEC_V4.md#4-one-model-definition-and-one-model-id-at-use',
     'Numeric definitions must preserve the role/sample contract, including raw control slots.'),
    ('R6', ('models', 'banks', 'numeric_interpretation'), 'specified',
     'SPEC_V4.md#4-one-model-definition-and-one-model-id-at-use',
     'Bank-relative tables must match the numeric interpretation and plane seed role.'),
    ('R7', ('models', 'coder'), 'specified', 'SPEC_V4.md#4-one-model-definition-and-one-model-id-at-use',
     'AC definition alphabet and restart syntax depend on ANS versus Huffman.'),
    ('R8', ('models', 'index'), 'proof-required', 'SPEC_V4.md#11-executable-decoder-audit-2026-11-12',
     'An indexed definition must bind to an actual preceding record, with legal lifetime.'),
    ('R9', ('coverage', 'index'), 'specified', 'SPEC_V4.md#5-one-authority-for-dc-geometry-and-unit-identity',
     'DC unit identity and owner rectangle derive from span starts, not AC stripe identity.'),
    ('R10', ('exact_packing', 'block_control'), 'gap', 'SPEC_V4.md#11-executable-decoder-audit-2026-11-12',
     'Exact counts/packing must agree with decoded selection; packing stripe cardinality is missing.'),
    ('R11', ('exact_packing', 'models'), 'gap', 'tools/v4-decoder/README.md#f1--packed-exact-geometry-is-missing-implementation-blocker',
     'A shared packed stripe must use its owning record model, possibly from an earlier unit.'),
    ('R12', ('exact_packing', 'index'), 'specified', 'SPEC.md#28-lossy-payload-head',
     'ROI exact pixels may need stripe bytes located in an earlier owner record.'),
    ('R13', ('coverage', 'block_control'), 'specified', 'SPEC_V4.md#5-one-authority-for-dc-geometry-and-unit-identity',
     'Current AC controls come from the correct rows of a possibly shared DC rectangle.'),
]


class Decisions(ast.NodeVisitor):
    """Explicit syntax convention; not an asserted compiler CFG/McCabe score."""
    def __init__(self):
        self.counts = Counter()

    def visit_FunctionDef(self, node):
        pass  # Nested functions are measured separately.

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_ClassDef(self, node):
        pass

    def visit_If(self, node):
        self.counts['if'] += 1
        self.generic_visit(node)

    def visit_IfExp(self, node):
        self.counts['conditional_expression'] += 1
        self.generic_visit(node)

    def visit_For(self, node):
        self.counts['loop'] += 1
        self.generic_visit(node)

    visit_While = visit_For

    def visit_ExceptHandler(self, node):
        self.counts['exception_handler'] += 1
        self.generic_visit(node)

    def visit_BoolOp(self, node):
        self.counts['short_circuit_link'] += len(node.values) - 1
        self.generic_visit(node)

    def visit_comprehension(self, node):
        self.counts['comprehension_generator'] += 1
        self.counts['comprehension_filter'] += len(node.ifs)
        self.generic_visit(node)


def functions(body, prefix=''):
    for node in body:
        if isinstance(node, ast.ClassDef):
            yield from functions(node.body, prefix + node.name + '.')
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            yield prefix + node.name, node
            yield from functions(node.body, prefix + node.name + '.')


def group(name):
    root = name.split('.')[0]
    if root == 'Reader':
        return 'primitives'
    if root in ('read_delta', 'apply', 'read_settings', 'select', 'resolve_tiles', 'effective_scale'):
        return 'settings'
    if root in ('Setup', 'read_coverage'):
        return 'geometry'
    if root == 'read_setup':
        return 'setup'
    if root in ('read_definition', 'Models'):
        return 'models'
    if root == 'walk_data':
        return 'traversal'
    if root in ('read_index', 'validated_model_lookup'):
        return 'index'
    if root == 'probe_selected_model':
        return 'audit_only_probe'
    raise ValueError('classify newly introduced function: ' + name)


def syntax_inventory():
    sample = ast.parse('def f(xs,a,b):\n if a and b:\n  for x in xs: pass\n return [x for x in xs if x]\n')
    visitor = Decisions()
    for statement in sample.body[0].body:
        visitor.visit(statement)
    assert sum(visitor.counts.values()) == 5
    result = {}
    for name, node in functions(ast.parse((HERE / 'decoder.py').read_text()).body):
        visitor = Decisions()
        for statement in node.body:
            visitor.visit(statement)
        result[name] = dict(group=group(name), decisions=sum(visitor.counts.values()),
                            kinds=dict(sorted(visitor.counts.items())))
    groups = {}
    for key in sorted({r['group'] for r in result.values()}):
        selected = {n: r for n, r in result.items() if r['group'] == key}
        largest = max(selected, key=lambda n: selected[n]['decisions'])
        groups[key] = dict(functions=len(selected), decisions=sum(r['decisions'] for r in selected.values()),
                           max_function=largest, max_decisions=selected[largest]['decisions'])
    core = [r for r in result.values() if r['group'] != 'audit_only_probe']
    return dict(groups=groups, functions=result,
                totals=dict(core_functions=len(core), core_decisions=sum(r['decisions'] for r in core)),
                missing='No entropy/model expansion, reconstruction, exact packing, checksums or outer container.')


def rule_inventory():
    neighbors = {key: set() for key in FEATURES}
    incidence = Counter()
    for key, nodes, status, source, reason in RULES:
        assert len(set(nodes)) == len(nodes) and set(nodes) <= set(FEATURES), key
        assert (ROOT / source.split('#')[0]).is_file(), source
        incidence.update(nodes)
        for a, b in itertools.combinations(nodes, 2):
            neighbors[a].add(b)
            neighbors[b].add(a)
    return dict(mechanisms=len(FEATURES), joint_rules=len(RULES),
                arity=dict(sorted(Counter(len(r[1]) for r in RULES).items())),
                statuses=dict(sorted(Counter(r[2] for r in RULES).items())),
                projected_pairs=sum(map(len, neighbors.values())) // 2,
                per_mechanism={key: dict(joint_rules=incidence[key], degree=len(neighbors[key]),
                                        neighbors=sorted(neighbors[key])) for key in FEATURES},
                rules=[dict(id=r[0], mechanisms=r[1], status=r[2], source=r[3], reason=r[4]) for r in RULES])


def settings_cases():
    # Homogeneous plane plans only. This is a stated finite subset, not all V4.
    plans = tuple(p for p in range(14) if p != 10)
    domains = (tuple(itertools.product((1, 2, 3), repeat=2)), tuple(range(8)), plans)
    legal, rejected = [], Counter()
    for geometry, partition, plan in itertools.product(*domains):
        try:
            decoder.resolve_tiles(b'', {}, decoder.State(partition, (plan,) * 12),
                                  *geometry, {'superblocks', 'small_transforms', 'channel_layouts'})
            legal.append((geometry, partition, plan))
        except decoder.Corrupt as error:
            rejected[str(error)] += 1
    pairs = set()
    for case in legal:
        pairs.update((a, case[a], b, case[b]) for a, b in itertools.combinations(range(3), 2))
    compatible_but_illegal = [case for case in itertools.product(*domains)
                             if all((a, case[a], b, case[b]) in pairs
                                    for a, b in itertools.combinations(range(3), 2)) and case not in legal]
    return dict(domain_sizes=list(map(len, domains)), attempted=math_product(map(len, domains)),
                accepted=len(legal), rejected=sum(rejected.values()), rejection_first_reason=dict(rejected),
                feasible_pair_assignments=len(pairs), pairwise_compatible_but_illegal=len(compatible_but_illegal),
                scope='Unit-local parent widths/heights 1/2/3; 8 partitions; 13 homogeneous plane plans. No pixels.')


def math_product(values):
    result = 1
    for value in values:
        result *= value
    return result


def aq_three_way():
    legal, rejected = [], []
    for merged, override, varying in itertools.product((False, True), (0, 64), (False, True)):
        aq = (32, 64, 32, 64) if varying else (64,) * 4
        state = decoder.State(scales=(override,) * 4)
        effective = tuple(decoder.effective_scale(state, p, aq[p]) for p in range(4))
        case = (merged, override, varying)
        (rejected if merged and len(set(effective)) != 1 else legal).append(case)
    pairs = {(a, case[a], b, case[b]) for case in legal for a, b in itertools.combinations(range(3), 2)}
    invisible = [case for case in rejected if all((a, case[a], b, case[b]) in pairs
                                                 for a, b in itertools.combinations(range(3), 2))]
    assert rejected == [(True, 0, True)] and invisible == rejected
    return dict(attempted=8, accepted=len(legal), rejected=rejected,
                pairwise_invisible_rejections=invisible,
                rule='V3 2.9.3 equal merged-parent multipliers after V4 scoped override resolution.')


def exact_dependency_witness():
    # Existing exact.rs::the_head_places_every_stripe: [0,10,2,8], B=4,H=16.
    source = (ROOT / 'imij/src/lossy/exact.rs').read_text()
    fixture = source.split('fn the_head_places_every_stripe()', 1)[1].split('    #[test]', 1)[0]
    values = re.search(r'head\(&\[([^\]]+)\],\s*(\d+),', fixture)
    counts = tuple(int(value.strip()) for value in values.group(1).split(','))
    row_blocks = int(values.group(2))
    sizes = re.search(r'assert_eq!\(\(e.per_stripe\(\), e.nested_stripes\(\)\), \((\d+), (\d+)\)\)', fixture)
    capacity, source_stripes = map(int, sizes.groups())
    assert capacity % row_blocks == 0
    units = [u for u, count in enumerate(counts) for _ in range(count)]
    consumers = [set(units[a:a + capacity]) for a in range(0, len(units), capacity)]
    owners = [units[a] for a in range(0, len(units), capacity)]
    foreign = [(owner, consumer) for owner, readers in zip(owners, consumers)
               for consumer in readers if consumer != owner]
    retained = [sum(owner <= boundary < max(readers) for owner, readers in zip(owners, consumers))
                for boundary in range(len(counts))]
    local = [(u, {u}) for u, count in enumerate(counts) if count
             for _ in range(0, count, min(count, row_blocks) * (capacity // row_blocks))]
    local_foreign = [(owner, reader) for owner, readers in local for reader in readers if reader != owner]
    local_retained = [sum(owner <= boundary < max(readers) for owner, readers in local)
                      for boundary in range(len(counts))]
    assert len(owners) == source_stripes
    assert owners == [1, 1, 3] and set(foreign) == {(1, 2), (1, 3)} and retained == [0, 1, 1, 0]
    return dict(source='imij/src/lossy/exact.rs::the_head_places_every_stripe', counts=counts,
                packed_blocks_per_stripe=capacity,
                shared=dict(owners=owners, cross_unit_owner_consumer_edges=len(foreign),
                            packed_stripes=len(owners),
                            max_consumer_units_per_stripe=max(map(len, consumers)),
                            retained_stripes_after_unit=retained),
                unit_local=dict(cross_unit_owner_consumer_edges=len(local_foreign),
                                packed_stripes=len(local), max_consumer_units_per_payload=max(len(r) for _, r in local),
                                cross_unit_retention_obligations=max(local_retained)),
                limit='Geometric witness, not a corpus result, memory/RSS estimate or timing result.')


def report():
    rust = (ROOT / 'imij/src/lossy/model_refs.rs').read_text()
    enum = re.search(r'enum Selector[^\{]*\{(.*?)\n\}', rust, re.S).group(1)
    variants = re.findall(r'^\s+([A-Z]\w*)[,(]', enum, re.M)
    assert variants == ['Root', 'Catalog', 'Inline', 'Define', 'Earlier']
    inputs = {HERE / 'decoder.py', Path(__file__).resolve(), ROOT / 'SPEC_V4.md',
              ROOT / 'imij/src/lossy/model_refs.rs', ROOT / 'imij/src/lossy/exact.rs'}
    inputs.update(ROOT / rule[3].split('#')[0] for rule in RULES)
    return dict(schema=1,
                runtime=dict(python=sys.version.split()[0], xxhash=version('xxhash')),
                inputs_xxh3_128={str(p.relative_to(ROOT)): xxhash.xxh3_128(p.read_bytes()).hexdigest()
                                 for p in sorted(inputs)},
                syntax=syntax_inventory(), coupling_inventory=rule_inventory(),
                settings_enumeration=settings_cases(), aq_three_way=aq_three_way(),
                exact_geometry_witness=exact_dependency_witness(),
                protocol_comparison=dict(v3_model_use_forms=variants, v4_model_use_encoding='one unsigned ID',
                                         v3_model_selectable_roles=int(re.search(r'if role > (\d+)', rust).group(1)) + 1,
                                         v4_draft_model_selectable_roles=len(decoder.KINDS),
                                         v3_inline_cross_record_pending_slots=0,
                                         v4_transient_cross_record_pending_slots=1,
                                         v4_definition_lifetimes=2, v4_settings_scope_depth=3,
                                         model_reference_chain_depth=1,
                                         v3_index_entry_encodings=2, v4_index_entry_encodings=1,
                                         index_placement_choices_both=2,
                                         v3_reservation_tag_values=3, v4_reservation_tag_values=0),
                interpretation='A vector of counts, not a weighted difficulty/performance score. Manual inventory is incomplete by construction.')


def markdown(r):
    lines = ['# Decoder structural complexity counts', '',
             'Missing pixel kernels are unmeasured, not zero. See docs/DECODER_COMPLEXITY_METRICS.md for scope.', '',
             '| Python structural area | Syntactic decisions | Largest function |', '|---|---:|---|']
    for key, value in r['syntax']['groups'].items():
        lines.append(f"| {key} | {value['decisions']} | {value['max_function']} ({value['max_decisions']}) |")
    lines += ['', '| Mechanism | Joint rules in reviewed inventory | Neighbors |', '|---|---:|---:|']
    for key, value in r['coupling_inventory']['per_mechanism'].items():
        lines.append(f"| {key} | {value['joint_rules']} | {value['degree']} |")
    lines += ['', '```json', json.dumps({k: r[k] for k in ['settings_enumeration', 'aq_three_way',
                                                         'exact_geometry_witness', 'protocol_comparison']}, indent=2), '```']
    return '\n'.join(lines) + '\n'


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args()
    result = report()
    print(json.dumps(result, indent=2) if args.json else markdown(result), end='\n' if args.json else '')
