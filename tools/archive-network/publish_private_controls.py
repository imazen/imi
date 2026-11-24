#!/usr/bin/env python3
"""Publish immutable matched controls to a new private R2 prefix, never a catalog.

Parent credentials stay local. Mint one-hour prefix-scoped temporary credentials;
put signed read URLs outside receipts. Preserve actual object HEAD metadata.
"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.request
import xxhash

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('config', type=Path)
p.add_argument('output', type=Path)
p.add_argument('access', type=Path)
a = p.parse_args()
c = json.loads(a.config.read_text())
a.output.mkdir(exist_ok=False)
a.access.mkdir(exist_ok=False, mode=0o700)
assert c['prefix'].startswith('validation/') and c['prefix'].endswith('/')
assert sum(Path(v).stat().st_size for v in c['archives'].values()) <= 128 * 1024**2
payload = dict(bucket=c['bucket'], permission='object-read-write', ttlSeconds=3600,
               prefixes=[c['prefix']], parentAccessKeyId=os.environ['R2_ACCESS_KEY_ID'],
               parentSecretAccessKey=os.environ['R2_SECRET_ACCESS_KEY'])
request = urllib.request.Request(
    f'https://api.cloudflare.com/client/v4/accounts/{os.environ["R2_ACCOUNT_ID"]}/r2/temp-access-credentials',
    data=json.dumps(payload).encode(), headers={'Authorization': 'Bearer '+os.environ['R2_API_TOKEN'],
                                               'Content-Type': 'application/json'})
with urllib.request.urlopen(request, timeout=30) as response:
    minted = json.load(response)
if not minted.get('success'):
    raise RuntimeError('scoped temporary credential mint failed')
credential = minted['result']
environment = dict(os.environ, AWS_ACCESS_KEY_ID=credential['accessKeyId'],
                   AWS_SECRET_ACCESS_KEY=credential['secretAccessKey'],
                   AWS_SESSION_TOKEN=credential['sessionToken'])
aws_config = a.access / 'aws-config'
aws_config.write_text('[default]\nregion = auto\ns3 =\n    signature_version = s3v4\n')
aws_config.chmod(0o600)
environment['AWS_CONFIG_FILE'] = str(aws_config)
aws = ['aws', '--endpoint-url', f'https://{os.environ["R2_ACCOUNT_ID"]}.r2.cloudflarestorage.com', '--region', 'auto']
receipts = []
for label, name in c['archives'].items():
    assert re.fullmatch('[a-z0-9-]+', label)
    path = Path(name)
    digest = xxhash.xxh3_128()
    with path.open('rb') as f:
        while block := f.read(1024 * 1024):
            digest.update(block)
    identity = digest.hexdigest()
    key = c['prefix'] + label + '-' + identity + '.bin'
    before = subprocess.run(aws + ['s3api', 'head-object', '--bucket', c['bucket'], '--key', key],
                            env=environment, text=True, capture_output=True)
    if before.returncode:
        assert '(404)' in before.stderr or 'Not Found' in before.stderr, 'HEAD failed before private upload'
        subprocess.run(aws + ['s3', 'cp', str(path), 's3://'+c['bucket']+'/'+key,
                              '--only-show-errors', '--no-progress', '--content-type', 'application/octet-stream',
                              '--cache-control', 'private,max-age=0,no-store', '--metadata', 'xxh3-128='+identity],
                       env=environment, check=True)
    head = json.loads(subprocess.check_output(aws + ['s3api', 'head-object', '--bucket', c['bucket'], '--key', key], env=environment))
    assert head['ContentLength'] == path.stat().st_size and head['Metadata']['xxh3-128'] == identity
    reader = subprocess.Popen(aws + ['s3', 'cp', 's3://'+c['bucket']+'/'+key, '-', '--only-show-errors', '--no-progress'],
                              env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    readback = xxhash.xxh3_128()
    readback_bytes = 0
    while block := reader.stdout.read(1024 * 1024):
        readback.update(block)
        readback_bytes += len(block)
    error = reader.stderr.read()
    assert reader.wait() == 0 and not error, 'private object readback failed'
    assert readback_bytes == path.stat().st_size and readback.hexdigest() == identity
    url = subprocess.check_output(aws + ['s3', 'presign', 's3://'+c['bucket']+'/'+key, '--expires-in', '3600'], env=environment, text=True)
    secret = a.access / (label + '.url')
    with secret.open('x') as f:
        f.write(url)
    secret.chmod(0o600)
    receipts.append(dict(label=label, bucket=c['bucket'], key=key, bytes=path.stat().st_size,
                         xxh3_128=identity, readback_verified=True, head=head, url_file=str(secret)))
    (a.output / 'OBJECTS.json').write_text(json.dumps(receipts, indent=2) + '\n')
    print(json.dumps({k: v for k, v in receipts[-1].items() if k not in ('head', 'url_file')}), flush=True)
