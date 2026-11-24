"""Independent check of a tiled Deflate+predictor RGBA TIFF import (stdlib zlib + PIL tag reader + numpy).
Decodes sampled tiles from the source with Python's zlib, undoes horizontal prediction with numpy, composites over
black as documented, and compares with the raster written by gallery_import. Tiles are chosen across the image.
usage: verify-gallery-cog.py SOURCE.tif LEVEL0.rgb WIDTH HEIGHT [TILES=40]
"""
import sys
import zlib
import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None
src, raw, w, h = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
count = int(sys.argv[5]) if len(sys.argv) > 5 else 40
tags = Image.open(src).tag_v2
tw, th, spp = tags[322], tags[323], tags[277]
assert tags[259] in (8, 32946) and tags[317] == 2 and tuple(tags[258]) == (8,) * spp
offsets, counts = tags[324], tags[325]
across, down = -(-w // tw), -(-h // th)
rng = np.random.default_rng(20261017)
picks = sorted(set(int(i) for i in rng.integers(0, across * down, count)) | {0, across - 1, across * down - 1, (down - 1) * across})
bad = n = worst = 0
with open(src, 'rb') as s, open(raw, 'rb') as r:
    for i in picks:
        ty, tx = divmod(i, across)
        s.seek(offsets[i])
        t = np.frombuffer(zlib.decompress(s.read(counts[i])), np.uint8).reshape(th, tw, spp).astype(np.uint16)
        t = np.cumsum(t, axis=1, dtype=np.uint16) & 0xFF  # horizontal predictor 2, modulo 256
        rows, cols = min(th, h - ty * th), min(tw, w - tx * tw)
        t = t[:rows, :cols]
        if spp == 4:
            expected = ((t[..., :3] * t[..., 3:4] + 127) // 255).astype(np.uint8)
        else:
            expected = t.astype(np.uint8)
        for y in range(rows):
            r.seek(((ty * th + y) * w + tx * tw) * 3)
            ours = np.frombuffer(r.read(cols * 3), np.uint8).reshape(cols, 3).astype(int)
            diff = np.abs(ours - expected[y].astype(int))
            worst = max(worst, int(diff.max()))
            bad += int((diff > 0).sum())
            n += diff.size
print({'tiles': len(picks), 'samples': n, 'max_abs_diff': worst, 'differing_samples': bad})
