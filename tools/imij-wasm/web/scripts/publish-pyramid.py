"""Publish prepared half-step levels to immutable public object keys; retain the prior catalog."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("root", type=Path)
parser.add_argument("prior", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--scene")
parser.add_argument("--reference-inputs", type=Path)
parser.add_argument("--max-prepared", action="store_true", help="Publish the complete effort10 preparation layout")
parser.add_argument("--lossy-only", action="store_true")
parser.add_argument("--quality-key", default="lossy")
parser.add_argument("--encode-effort", type=float, default=0)
parser.add_argument("--quality-target", type=float, default=80)
parser.add_argument("--include-full", action="store_true")
parser.add_argument("--raw", type=Path)
parser.add_argument("--pyramid-subdir", default="")
args = parser.parse_args()
root, prior, output = args.root, args.prior, args.output
if output.exists():
    raise FileExistsError(output)
catalog = json.loads(prior.read_text())
endpoint = 'https://' + os.environ['R2_ACCOUNT_ID'] + '.r2.cloudflarestorage.com'
os.environ['AWS_ACCESS_KEY_ID'] = os.environ['R2_ACCESS_KEY_ID']
os.environ['AWS_SECRET_ACCESS_KEY'] = os.environ['R2_SECRET_ACCESS_KEY']
os.environ.pop('AWS_SESSION_TOKEN', None)
records = []


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def publish(path, existing=None):
    sha = digest(path)
    size = path.stat().st_size
    if existing and existing['sha256'] == sha:
        expected = existing.get('encodedBytes', existing.get('bytes'))
        if expected != size:
            raise ValueError('existing object extent')
        value = dict(existing)
        value['encodedBytes'] = size
        records.append({'file': str(path.relative_to(root)), 'reused': True, **value})
        return value
    key = 'imij/nasa/' + sha + '/' + path.name
    command = ['aws', '--endpoint-url', endpoint, 's3api', 'head-object', '--bucket', 'codec-corpus', '--key', key]
    before = subprocess.run(command, capture_output=True, text=True)
    if before.returncode:
        if '(404)' not in before.stderr and 'Not Found' not in before.stderr:
            raise RuntimeError(before.stderr)
        subprocess.run(['aws', '--endpoint-url', endpoint, 's3', 'cp', str(path), 's3://codec-corpus/' + key,
                        '--no-progress', '--only-show-errors', '--content-type', 'application/octet-stream',
                        '--cache-control', 'public,max-age=31536000,immutable', '--metadata', 'sha256=' + sha], check=True)
        head = json.loads(subprocess.check_output(command))
    else:
        head = json.loads(before.stdout)
    if head['ContentLength'] != size or head['Metadata']['sha256'] != sha:
        raise ValueError('published object identity')
    value = {'url': 'https://codec-corpus.r2.imazen.org/' + key, 'sha256': sha, 'encodedBytes': size,
             'etag': head['ETag'], 'multipart': False}
    records.append({'file': str(path.relative_to(root)), **value})
    print(path.relative_to(root), size, sha, flush=True)
    return value


def rois(path, w, h):
    rw, rh = min(w, 32), min(h, 32)
    if path.stat().st_size != w * h * 3:
        raise ValueError('raw raster extent')
    result = []
    with path.open('rb') as stream:
        for x, y in [(min(w // 3, w-rw), min(h // 3, h-rh)), (min(w // 2, w-rw), min(h // 2, h-rh)), (w-rw, h-rh)]:
            pixels = bytearray()
            for yy in range(y, y+rh):
                stream.seek((yy*w+x)*3)
                pixels.extend(stream.read(rw*3))
            if len(pixels) != rw*rh*3:
                raise ValueError('raw ROI extent')
            result.append({'x': x, 'y': y, 'width': rw, 'height': rh, 'sha256': hashlib.sha256(pixels).hexdigest()})
    return result


scenes = catalog['datasets'] if args.scene is None else [s for s in catalog['datasets'] if s['id'] == args.scene]
if args.scene is not None and len(scenes) != 1:
    raise ValueError('scene identity')
if args.max_prepared:
    if args.reference_inputs is None:raise ValueError('sealed reference input records required')
    reference_inputs={(item['scene'],item['level']):item['inputs'] for item in json.loads(args.reference_inputs.read_text())}
    complete=json.loads((root/'complete.json').read_text())
    prepared=json.loads((root/'preparation.json').read_text())
    if complete['count']!=len(prepared):raise ValueError('incomplete maximum-effort preparation')
    expected={(scene['id'],level['key'],quality) for scene in catalog['datasets'] for level in scene['levels'] for quality in ('lossless','lossy','webmap')}
    actual={(item['scene'],item['level'],item['quality']) for item in prepared}
    if expected!=actual or len(actual)!=len(prepared):raise ValueError('maximum-effort domain')
    for item in prepared:
        if item['effort']!=10 or item['rc']!=0 or item['binarySha256']!=complete['binarySha256']:raise ValueError('encoder provenance')
        reference=reference_inputs[(item['scene'],item['level'])]
        if [(v['bytes'],v['sha256']) for v in item['inputs']]!=[(v['bytes'],v['sha256']) for v in reference]:raise ValueError('original source pixels changed')
        if item['target']!={'lossless':None,'lossy':80,'webmap':70}[item['quality']]:raise ValueError('quality target')
        scene=next(scene for scene in catalog['datasets'] if scene['id']==item['scene'])
        old=scene['representations'][item['quality']][item['level']]
        if (old['width'],old['height'])!=(item['width'],item['height']):raise ValueError('maximum-effort geometry')
        path=root/item['file']
        if digest(path)!=item['sha256'] or digest(path.with_suffix('.bin'))!=item['indexSha256']:raise ValueError('prepared identity')
        archive=publish(path,old)
        index=publish(path.with_suffix('.bin'),old.get('index'))
        archive.update(kind='tiles',width=item['width'],height=item['height'],divisor=item['divisor'],effort=10,
                       index={'url':index['url'],'sha256':index['sha256'],'bytes':index['encodedBytes']},
                       quality='IMI lossless effort10; ordinary 512x256 cells' if item['quality']=='lossless' else f"IMI JpegQuality target{item['target']}, Fast decode, effort10; ordinary 512x256 cells")
        if item['quality']=='lossless':archive['rois']=scene['representations']['lossless'][item['level']].get('rois',[])
        else:archive['qualityTarget']=item['target']
        scene['representations'][item['quality']][item['level']]=archive
    output.write_text(json.dumps(catalog,indent=2)+'\n')
    output.with_name(output.stem + '-objects.json').write_text(json.dumps(records,indent=2)+'\n')
    raise SystemExit(0)
for scene in scenes:
    directory = root / scene['id']
    pyramid = directory / args.pyramid_subdir
    levels = json.loads((pyramid / 'levels.json').read_text())
    scene['representations'].setdefault(args.quality_key, {})
    if args.include_full:
        w, h = scene['width'], scene['height']
        if not args.lossy_only:
            image = publish(directory / 'full-lossless.imi')
            cache = publish(directory / 'full-lossless-cache.bin')
            image.update(kind='indexed', width=w, height=h, divisor=1,
                         bootstrap={'url': cache['url'], 'sha256': cache['sha256'], 'bytes': cache['encodedBytes']},
                         rois=rois(args.raw if args.raw else directory / 'source.rgb', w, h))
            scene['representations']['lossless']['full'] = image
        archive = publish(directory / 'full-lossy.imijset')
        index = publish(directory / 'full-lossy.bin')
        archive.update(kind='tiles', width=w, height=h, divisor=1,
                       index={'url': index['url'], 'sha256': index['sha256'], 'bytes': index['encodedBytes']},
                       quality=f'IMI JpegQuality target{args.quality_target:g}, Fast decode, effort{args.encode_effort:g}; ordinary 512x256 cells', qualityTarget=args.quality_target,effort=args.encode_effort)
        scene['representations'][args.quality_key]['full'] = archive
    divisor = 2
    scene['levels'] = [{'key': 'full', 'divisor': 1}]
    for level in levels:
        if level['divisor'] != divisor:
            raise ValueError('nonconsecutive pyramid level')
        w, h = level['width'], level['height']
        if (w, h) != ((scene['width']+divisor-1)//divisor, (scene['height']+divisor-1)//divisor):
            raise ValueError('pyramid geometry')
        key = {2: 'half', 4: 'quarter'}.get(divisor, 'level-' + str(divisor))
        scene['levels'].append({'key': key, 'divisor': divisor})
        cell = pyramid / ('level-' + str(divisor))
        if not args.lossy_only:
            old = scene['representations']['lossless'].get(key)
            image = publish(cell / 'lossless.imi', old)
            cache = publish(cell / 'lossless-cache.bin', old.get('bootstrap') if old else None)
            image.update(kind='indexed', width=w, height=h, divisor=divisor,
                         bootstrap={'url': cache['url'], 'sha256': cache['sha256'], 'bytes': cache['encodedBytes']},
                         rois=rois(cell / 'source.rgb', w, h))
            scene['representations']['lossless'][key] = image
        old = scene['representations'][args.quality_key].get(key)
        archive = publish(cell / 'lossy.imijset', old)
        index = publish(cell / 'lossy.bin', old.get('index') if old else None)
        archive.update(kind='tiles', width=w, height=h, divisor=divisor,
                       index={'url': index['url'], 'sha256': index['sha256'], 'bytes': index['encodedBytes']},
                       quality=f'IMI JpegQuality target{args.quality_target:g}, Fast decode, effort{args.encode_effort:g}; ordinary 512x256 cells', qualityTarget=args.quality_target,effort=args.encode_effort)
        scene['representations'][args.quality_key][key] = archive
        divisor *= 2
    if not levels or max(levels[-1]['width'], levels[-1]['height']) > 512:
        raise ValueError('missing overview level')
output.write_text(json.dumps(catalog, indent=2) + '\n')
output.with_name(output.stem + '-objects.json').write_text(json.dumps(records, indent=2) + '\n')
