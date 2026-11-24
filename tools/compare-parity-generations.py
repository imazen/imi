#!/usr/bin/env python3
"""Compare encoded files and pixels between immutable parity generations."""
import argparse
import hashlib
import json
from pathlib import Path

from decode_parity import fixture_hashes


def verify_seal(directory):
    manifest = directory / 'sha256.json'
    expected = json.loads(manifest.read_text())['files']
    actual = fixture_hashes(directory)
    if actual != expected:
        raise ValueError(f'{directory}: fixture changed after sealing')
    names = {name[:-4] for name in actual if name.endswith('.imi')}
    if not names or names != {p.stem for p in directory.glob('*.imi')}:
        raise ValueError(f'{directory}: empty domain or unsealed encoded fixtures')
    if names != {p.stem for p in directory.glob('*.raw')}:
        raise ValueError(f'{directory}: unsealed or missing raw fixtures')
    return names, hashlib.sha256(manifest.read_bytes()).hexdigest()


def compare(old_root, new_root, strict):
    changes = []
    domains = {}
    for group in ('base', 'extra', 'sb'):
        old, new = old_root / group, new_root / group
        names_old, seal_old = verify_seal(old)
        names_new, seal_new = verify_seal(new)
        matched = sorted(names_old & names_new)
        added, removed = sorted(names_new - names_old), sorted(names_old - names_new)
        domains[group] = {
            'old_directory': str(old.resolve()), 'new_directory': str(new.resolve()),
            'old_seal_sha256': seal_old, 'new_seal_sha256': seal_new,
            'old_ids': sorted(names_old), 'new_ids': sorted(names_new),
            'matched_ids': matched, 'added_ids': added, 'removed_ids': removed,
        }
        for name in matched:
            for suffix in ('.imi', '.raw'):
                filename = name + suffix
                before, after = (old / filename).read_bytes(), (new / filename).read_bytes()
                if before != after:
                    changes.append({
                        'set': group, 'file': filename,
                        'old_bytes': len(before), 'new_bytes': len(after),
                        'old_sha256': hashlib.sha256(before).hexdigest(),
                        'new_sha256': hashlib.sha256(after).hexdigest(),
                    })
        print(group, 'matched', len(matched), 'added', len(added), 'removed', len(removed))
        for suffix in ('.imi', '.raw'):
            selected = [c for c in changes if c['set'] == group and c['file'].endswith(suffix)]
            print(group, suffix, 'changed', len(selected), 'byte deltas',
                  sorted({c['new_bytes'] - c['old_bytes'] for c in selected}))
    failures = []
    if any(change['file'].endswith('.raw') for change in changes):
        failures.append('decoded fixture pixels changed')
    if strict and any(d['added_ids'] or d['removed_ids'] for d in domains.values()):
        failures.append('inherited fixture domain changed')
    if any(d['removed_ids'] for d in domains.values()):
        failures.append('fixtures removed')
    return {'strict_domain': strict, 'domains': domains, 'changes': changes,
            'failures': failures}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('old', type=Path)
    parser.add_argument('new', type=Path)
    parser.add_argument('report', type=Path)
    parser.add_argument('--canonical', type=Path,
                        help='also compare matched pixels and inventory domain changes against this seal')
    args = parser.parse_args()
    if args.report.exists():
        parser.error('use a new report path')
    result = {'inherited': compare(args.old, args.new, strict=True)}
    if args.canonical:
        result['canonical'] = compare(args.canonical, args.new, strict=False)
    with args.report.open('x') as report:
        json.dump(result, report, indent=2)
        report.write('\n')
    failures = [failure for comparison in result.values() for failure in comparison['failures']]
    if failures:
        raise SystemExit('; '.join(failures))


if __name__ == '__main__':
    main()
