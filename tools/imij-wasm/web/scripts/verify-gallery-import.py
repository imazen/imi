"""Third-party oracle (PIL / libjpeg-turbo) for a single-JPEG import: four 64-row strips of the
raster written by gallery_import must equal the independent decode exactly.
usage: verify-gallery-import.py SOURCE.jpg LEVEL0.rgb WIDTH HEIGHT
"""
import sys
import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None
src, raw, w, h = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
im = Image.open(src)
im.load()
assert im.size == (w, h) and im.mode == 'RGB'
worst = bad = n = 0
with open(raw, 'rb') as f:
    for y in [0, h // 3, h // 2, h - 64]:
        f.seek(y * w * 3)
        ours = np.frombuffer(f.read(64 * w * 3), np.uint8).astype(int)
        theirs = np.frombuffer(im.crop((0, y, w, y + 64)).tobytes(), np.uint8).astype(int)
        assert len(ours) == len(theirs)
        diff = np.abs(ours - theirs)
        worst = max(worst, int(diff.max()))
        bad += int((diff > 0).sum())
        n += len(ours)
print({'strips': 4, 'samples': n, 'max_abs_diff': worst, 'differing_samples': bad})
