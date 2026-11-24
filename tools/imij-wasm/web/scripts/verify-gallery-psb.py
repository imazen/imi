"""Third-party oracle (littleCMS via PIL ImageCms) for an uncompressed merged RGB8 PSB import.
Reads the planar channels of five 64-row strips straight from the source, converts them with the
embedded ICC profile to sRGB (relative colorimetric) and compares with the raster written by psb_srgb (MoxCms).
Different CMS implementations may differ by a code value or two: maximum and histogram are reported.
usage: verify-gallery-psb.py SOURCE.psb LEVEL0.rgb EMBEDDED.icc WIDTH HEIGHT PIXEL_DATA_OFFSET
"""
import io
import sys
import numpy as np
from PIL import Image, ImageCms

src, raw, icc_path, w, h, offset = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]), int(sys.argv[6])
icc = open(icc_path, 'rb').read()
transform = ImageCms.buildTransform(ImageCms.ImageCmsProfile(io.BytesIO(icc)), ImageCms.createProfile('sRGB'), 'RGB', 'RGB',
                                    renderingIntent=ImageCms.Intent.RELATIVE_COLORIMETRIC)
worst = bad = n = 0
hist = np.zeros(256, dtype=np.int64)
with open(src, 'rb') as s, open(raw, 'rb') as r:
    for y in [0, h // 4, h // 2, 3 * h // 4, h - 64]:
        planes = []
        for c in range(3):
            s.seek(offset + c * w * h + y * w)
            planes.append(np.frombuffer(s.read(64 * w), np.uint8).reshape(64, w))
        strip = Image.fromarray(np.stack(planes, axis=-1), 'RGB')
        theirs = np.asarray(ImageCms.applyTransform(strip, transform)).astype(int).ravel()
        r.seek(y * w * 3)
        ours = np.frombuffer(r.read(64 * w * 3), np.uint8).astype(int)
        diff = np.abs(ours - theirs)
        worst = max(worst, int(diff.max()))
        bad += int((diff > 0).sum())
        n += len(ours)
        hist += np.bincount(diff, minlength=256)
print({'strips': 5, 'samples': n, 'max_abs_diff': worst, 'differing_samples': bad, 'diff_histogram_0_to_4': [int(v) for v in hist[:5]]})
