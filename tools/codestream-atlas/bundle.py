#!/usr/bin/env python3
"""Packs codestream traces, decoded RGBA8 pixels and V2/V3/V4 specification sections into
data.js for the Codestream Atlas viewer."""
import base64, glob, json, os, re, sys
traces_dir, spec_md, out = sys.argv[1], sys.argv[2], sys.argv[3]
# Optional encode sources (PNG files) for the in-browser encoder.
source_pngs = sys.argv[4:]
# Native decode timing (profile_fixtures.sh): NAME.prof<THREADS>.json per
# file and profile-meta.json for the host.
meta_path = os.path.join(traces_dir, 'profile-meta.json')
profile_meta = json.load(open(meta_path)) if os.path.exists(meta_path) else None
files = []
for f in sorted(glob.glob(os.path.join(traces_dir, '*.json'))):
    if '.prof' in os.path.basename(f) or os.path.basename(f) == 'profile-meta.json':
        continue
    t = json.load(open(f))
    stem = os.path.basename(f)[:-5]
    runs = {}
    for p in glob.glob(os.path.join(traces_dir, stem + '.prof*.json')):
        runs[p[:-5].rsplit('.prof', 1)[1]] = json.load(open(p))
    if runs:
        t['timing'] = {'native': dict(profile_meta or {}, runs=runs)}
    raw = os.path.join(traces_dir, stem + '.rgba')
    if os.path.exists(raw):
        t['rgba'] = base64.b64encode(open(raw, 'rb').read()).decode()
    name = stem.split('_')[0]
    kind = stem.split('-', 1)[1] if '-' in stem else stem
    t['name'] = f"{name} · {kind}"
    files.append(t)
# Spec sections: "## 2. DATA chunk" / "### 2.3 Stripes and lanes" /
# "### 1.4.1 Decoded bitmap checksum" -> id "2" / "2.3" / "1.4.1"; deeper
# headings ("#### 2.8.1 ...") stay inside their section's text.
sections = {}
for prefix, path in [('', spec_md), ('V4:', os.path.join(os.path.dirname(spec_md), 'SPEC_V4.md'))]:
    cur, buf = None, []
    with open(path) as spec_input:
        for line in spec_input:
            m = re.match(r'^(#{2,3}) (\d+(?:\.\d+)*)\.? (.*)$', line.rstrip())
            if m:
                if cur: sections[cur]['text'] = ''.join(buf).strip()
                cur = prefix + m.group(2); buf = []
                sections[cur] = {'title': m.group(3)}
            elif re.match(r'^#{1,3} ', line):
                if cur: sections[cur]['text'] = ''.join(buf).strip()
                cur = None; buf = []
            elif cur:
                buf.append(line)
        if cur: sections[cur]['text'] = ''.join(buf).strip()
# Lossy unit records: SPEC 2.8 gives the lossy head and defers the unit
# record to docs/LOSSY.md.
sections['LOSSY'] = {'title': 'Lossy unit records (docs/LOSSY.md)',
                     'text': 'SPEC 2.8 specifies the lossy payload head; the lossy unit record (DC, alpha and AC records) and its semantics are specified in docs/LOSSY.md. This view is generated from the decoder that reads them.'}
sources = [{'name': os.path.basename(p)[:-4], 'png': base64.b64encode(open(p, 'rb').read()).decode()}
           for p in source_pngs]
open(out, 'w').write('window.ATLAS=' + json.dumps({'files': files, 'spec': sections, 'sources': sources}) + ';\n')
print(len(files), 'files', len(sections), 'sections', len(sources), 'sources', os.path.getsize(out), 'bytes')
