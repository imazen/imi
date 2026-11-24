#!/usr/bin/env python3
"""Check tracked paths and text without repeating restricted external product names."""
import xxhash
from pathlib import Path
import re
import subprocess
import unicodedata

# Fingerprints permit enforcing the naming policy without storing the name here.
RESTRICTED = {6: {'54cafb6a57e806392932da4d47ad6584'}}


def restricted_lines(lines, rules=RESTRICTED):
    carry = ''
    maximum = max(rules)
    for number, line in enumerate(lines, 1):
        normalized = re.sub('[^a-z0-9]', '', unicodedata.normalize('NFKC', line).casefold())
        joined = carry + normalized
        for length, fingerprints in rules.items():
            for start in range(max(0, len(carry) - length + 1), len(joined) - length + 1):
                if xxhash.xxh3_128(joined[start:start + length].encode()).hexdigest() in fingerprints:
                    yield number
                    break
            else:
                continue
            break
        carry = joined[-(maximum - 1):] if maximum > 1 else ''


def main():
    git_root = subprocess.run(['git', 'rev-parse', '--show-toplevel'],
                              capture_output=True, text=True)
    if git_root.returncode == 0:
        root = Path(git_root.stdout.strip())
        files = subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z'])
    else:
        root = Path(subprocess.check_output(['jj', 'root'], text=True).strip())
        files = subprocess.check_output(['jj', 'file', 'list', '-T', r'path ++ "\0"'], cwd=root)
    paths = files.decode().split('\0')
    failures = []
    checked = 0
    for relative in paths:
        if not relative:
            continue
        restricted_path = bool(list(restricted_lines([relative])))
        display = '<restricted filename>' if restricted_path else relative
        if restricted_path:
            failures.append('<restricted filename>')
        path = root / relative
        if not path.is_file():
            raise RuntimeError('tracked input missing: ' + display)
        data = path.read_bytes()
        checked += 1
        for number in restricted_lines(data.decode('utf-8', errors='replace').splitlines()):
            failures.append(f'{display}:{number}')
    for failure in failures:
        print(f'naming policy violation: {failure}')
    print(f'Checked {checked} tracked files; {len(failures)} naming policy violations.')
    return bool(failures)


if __name__ == '__main__':
    raise SystemExit(main())
