"""Oracle for a jpeg-grid import: PIL/libjpeg-turbo decodes of each part must equal that part's placed region,
and seam continuity (mean |difference| of adjacent columns/rows across each part boundary versus nearby interior pairs)
is reported so a misplaced or cropped part would show.
usage: verify-gallery-grid.py IMPORT.json LEVEL0.rgb
"""
import json
import sys
import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None
rec = json.load(open(sys.argv[1]))
cols, rows = rec['grid']
W, H = rec['width'], rec['height']
raw = np.memmap(sys.argv[2], dtype=np.uint8, mode='r', shape=(H, W, 3))
y0 = 0
parts = rec['parts']
result = {'parts': [], 'seams': []}
heights = []
for r in range(rows):
    x0 = 0
    for c in range(cols):
        im = Image.open(parts[r * cols + c]['file'])
        im.load()
        a = np.asarray(im)
        h, w = a.shape[:2]
        ours = raw[y0:y0 + h, x0:x0 + w]
        diff = np.abs(ours.astype(int) - a.astype(int))
        result['parts'].append({'part': r * cols + c + 1, 'size': [w, h], 'max_abs_diff': int(diff.max()), 'differing_samples': int((diff > 0).sum())})
        x0 += w
    heights.append(h)
    y0 += h


def mad(a, b):
    return float(np.abs(a.astype(int) - b.astype(int)).mean())


x = 0
for c in range(cols - 1):
    x += json.loads(json.dumps(result['parts'][c]))['size'][0]
    strip = slice(0, min(H, 2000))
    seam = mad(raw[strip, x - 1], raw[strip, x])
    inner = mad(raw[strip, x - 6], raw[strip, x - 5])
    result['seams'].append({'vertical_boundary_x': x, 'seam_mad': round(seam, 3), 'interior_mad_nearby': round(inner, 3)})
y = 0
for r in range(rows - 1):
    y += heights[r]
    seam = mad(raw[y - 1, :4000], raw[y, :4000])
    inner = mad(raw[y - 6, :4000], raw[y - 5, :4000])
    result['seams'].append({'horizontal_boundary_y': y, 'seam_mad': round(seam, 3), 'interior_mad_nearby': round(inner, 3)})
print(json.dumps(result, indent=1))
