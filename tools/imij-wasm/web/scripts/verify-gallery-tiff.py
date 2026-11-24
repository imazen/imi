"""Third-party oracle (PIL tag reader + littleCMS via ImageCms) for a TIFF import with an embedded ICC profile.
Reads source strips directly (uncompressed chunky RGB8, single strip), converts them to sRGB with littleCMS
(relative colorimetric) and compares with the raster written by gallery_import (MoxCms). Two different CMS
implementations may differ by a code value or two: the maximum and the count of differing samples are reported.
usage: verify-gallery-tiff.py SOURCE.tif LEVEL0.rgb WIDTH HEIGHT
"""
import io
import sys
import numpy as np
from PIL import Image, ImageCms

Image.MAX_IMAGE_PIXELS = None
src, raw, w, h = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
im = Image.open(src)
assert im.size == (w, h) and im.mode == 'RGB'
tags = im.tag_v2
offset, = tags[273]
assert tags[259] == 1 and tags[277] == 3 and tuple(tags[258]) == (8, 8, 8) and tags[278] >= h
icc = tags[34675]
srgb = ImageCms.createProfile('sRGB')
transform = ImageCms.buildTransform(ImageCms.ImageCmsProfile(io.BytesIO(icc)), srgb, 'RGB', 'RGB',
                                    renderingIntent=ImageCms.Intent.RELATIVE_COLORIMETRIC)
worst = bad = n = 0
hist = np.zeros(256, dtype=np.int64)
with open(src, 'rb') as s, open(raw, 'rb') as r:
    for y in [0, h // 4, h // 2, 3 * h // 4, h - 64]:
        s.seek(offset + y * w * 3)
        strip = Image.frombytes('RGB', (w, 64), s.read(64 * w * 3))
        theirs = np.asarray(ImageCms.applyTransform(strip, transform)).astype(int).ravel()
        r.seek(y * w * 3)
        ours = np.frombuffer(r.read(64 * w * 3), np.uint8).astype(int)
        diff = np.abs(ours - theirs)
        worst = max(worst, int(diff.max()))
        bad += int((diff > 0).sum())
        n += len(ours)
        hist += np.bincount(diff, minlength=256)
print({'strips': 5, 'samples': n, 'max_abs_diff': worst, 'differing_samples': bad,
       'diff_histogram_0_to_4': [int(v) for v in hist[:5]]})
