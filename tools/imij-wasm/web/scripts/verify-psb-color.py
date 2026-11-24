"""Independent LittleCMS oracle for the bounded PSB importer's RGB8 conversion."""
from pathlib import Path
import ctypes as C
import ctypes.util
import hashlib
import json
import random
import struct
import subprocess
import sys

binary, profile, output = map(Path, sys.argv[1:])
output.mkdir(exist_ok=False)
icc = profile.read_bytes()
rng = random.Random(1242)
pixels = bytes(range(256)) * 3 + bytes(rng.randrange(256) for _ in range(4096 * 3))
width = len(pixels) // 3
resource = b'8BIM' + struct.pack('>H', 1039) + b'\0\0' + struct.pack('>I', len(icc)) + icc + b'\0' * (len(icc) % 2)
header = b'8BPS' + struct.pack('>H', 2) + bytes(6) + struct.pack('>HIIHH', 3, 1, width, 8, 3)
psb = header + struct.pack('>II', 0, len(resource)) + resource + bytes(8) + bytes(2)
psb += b''.join(pixels[channel::3] for channel in range(3))
source = output / 'fixture.psb'
source.write_bytes(psb)
subprocess.run([str(binary), str(source), str(output / 'actual.rgb'), str(output / 'copied.icc')], check=True)
if (output / 'copied.icc').read_bytes() != icc:
    raise ValueError('source ICC changed')
lib = C.CDLL(ctypes.util.find_library('lcms2'))
lib.cmsOpenProfileFromMem.argtypes = [C.c_void_p, C.c_uint32]
lib.cmsOpenProfileFromMem.restype = C.c_void_p
lib.cmsCreate_sRGBProfile.restype = C.c_void_p
lib.cmsCreateTransform.argtypes = [C.c_void_p, C.c_uint32, C.c_void_p, C.c_uint32, C.c_uint32, C.c_uint32]
lib.cmsCreateTransform.restype = C.c_void_p
lib.cmsDoTransform.argtypes = [C.c_void_p, C.c_void_p, C.c_void_p, C.c_uint32]
lib.cmsDeleteTransform.argtypes = [C.c_void_p]
lib.cmsCloseProfile.argtypes = [C.c_void_p]
profile_buffer = C.create_string_buffer(icc)
src = lib.cmsOpenProfileFromMem(profile_buffer, len(icc))
dst = lib.cmsCreate_sRGBProfile()
transform = lib.cmsCreateTransform(src, 262169, dst, 262169, 1, 0)
if not src or not dst or not transform:
    raise ValueError('LittleCMS transform creation')
reference = C.create_string_buffer(len(pixels))
lib.cmsDoTransform(transform, C.create_string_buffer(pixels), reference, width)
lib.cmsDeleteTransform(transform)
lib.cmsCloseProfile(src)
lib.cmsCloseProfile(dst)
actual = (output / 'actual.rgb').read_bytes()
if len(actual) != len(pixels):
    raise ValueError('converted extent')
deltas = [abs(a-b) for a, b in zip(actual, reference.raw)]
result = {'pixels': width, 'source_icc_sha256': hashlib.sha256(icc).hexdigest(),
          'actual_sha256': hashlib.sha256(actual).hexdigest(),
          'reference_sha256': hashlib.sha256(reference.raw).hexdigest(),
          'max_channel_difference': max(deltas), 'differing_channels': sum(d != 0 for d in deltas),
          'intent': 'relative colorimetric', 'oracle': 'LittleCMS2 RGB8; separate CMS rounding, not codec parity'}
(output / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result))
if max(deltas) > 2:
    raise ValueError('CMS disagreement exceeds two RGB8 code values')
