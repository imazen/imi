# Format version 2: bytes per file before and after (2026-10-10)

What format version 2 (SPEC.md, appendix V2) costs or saves per file, measured, not predicted. Decoded pixels
did not change in any measured cell.

## Method

- **Revisions:** before = `24d002bf` (format version 1); after = the format version 2 tree `9e4bc514`. The
  commits that land (`79a1b899` and the review fixes on top of it, rebased onto `6227d907`) encode
  identically: a byte-gate of `6227d907` against `79a1b899` (same program, default and `lab` builds) produced
  lists byte-identical to those of `24d002bf` and `9e4bc514`, and the review fixes change no encoder output
  (byte-gate `79a1b899` against the final commit). Encoder code paths are the same; only the container and
  payload grammar differ.
- **Tools:** `tools/byte-gate/run.sh` builds a program against each revision (`git archive`), and
  `tools/byte-gate/compare.py` joins the two lists cell by cell, checks that every cell decodes to the same
  pixels (FNV-1a 64 of the decoded buffer), and sums the size change per group.
  - Bytes report: `GATE_PROGRAM=tools/byte-gate/sizes.rs GATE_FEATURES=default GATE_RAW=<inputs>`.
    Per image: lossless (`ll`), lossless with metadata (`ll-meta`), lossless RGBA (`ll-rgba`), lossy XYB at
    q10/30/60/90, lossy q60 with metadata, YCoCg q60 and RGBA q60. Metadata: a 1 KiB ICC stand-in, a 64-byte
    Exif block, a 114-byte XMP packet and orientation 6.
  - Inputs (`tools/byte-gate/prepare_sizes.py`): 64 px = the 28 byte-gate images (1024 px crops of the RD
    corpus) downscaled to 64 x 64 with Lanczos; 4096 px = the centre 4096 x 4096 crops of the first 6 photos
    (name order) at least 4096 px on both sides in a private photo set; their SHA-256 are below.
  - Byte-gate grid (`main.rs`, default and `lab` builds): 28 images at 1024 px, 386 encodes each (360 lossy, 26 lossless).
- **Host:** r5900xt (encoded bytes do not depend on the host).

## Results

### Bytes report (64 px and 4096 px)

`compare.py` over `sizes.rs` lists, 340 cells (28 images at 64 px and 6 at 4096 px, 10 encodes each). Decoded
pixels differ in **0** of 340 cells. Mean bytes per file, before and after; delta per file (every image of a
group moves by the same few bytes, so the median, min and max show the spread):

| cells | n | mean bytes before | mean bytes after | median delta | min | max | total % |
|---|---:|---:|---:|---:|---:|---:|---:|
| 4096x4096 ll | 6 | 7107837.3 | 7108090.3 | +253 | +253 | +253 | +0.004 |
| 4096x4096 ll-meta | 6 | 7109060.3 | 7109313.3 | +253 | +253 | +253 | +0.004 |
| 4096x4096 ll-rgba | 6 | 9025748.5 | 9026001.5 | +253 | +253 | +253 | +0.003 |
| 4096x4096 lossy-q10 | 6 | 300858.3 | 301688.0 | +841 | +789 | +857 | +0.276 |
| 4096x4096 lossy-q30 | 6 | 682525.0 | 683381.7 | +857 | +856 | +857 | +0.126 |
| 4096x4096 lossy-q60 | 6 | 1233441.3 | 1234298.0 | +857 | +856 | +857 | +0.069 |
| 4096x4096 lossy-q60-meta | 6 | 1234664.3 | 1235521.0 | +857 | +856 | +857 | +0.069 |
| 4096x4096 lossy-q60-rgba | 6 | 5786296.7 | 5788083.8 | +1763.5 | +1712 | +1879 | +0.031 |
| 4096x4096 lossy-q60-ycocg | 6 | 1080172.2 | 1081028.8 | +857 | +856 | +857 | +0.079 |
| 4096x4096 lossy-q90 | 6 | 3253187.8 | 3254098.7 | +899 | +863 | +982 | +0.028 |
| 64x64 ll | 28 | 6747.1 | 6747.1 | +0 | +0 | +0 | +0.000 |
| 64x64 ll-meta | 28 | 7966.1 | 7966.1 | +0 | +0 | +0 | +0.000 |
| 64x64 ll-rgba | 28 | 8988.0 | 8988.0 | +0 | +0 | +0 | +0.000 |
| 64x64 lossy-q10 | 28 | 461.8 | 453.8 | -8 | -8 | -8 | -1.732 |
| 64x64 lossy-q30 | 28 | 981.4 | 973.4 | -8 | -8 | -8 | -0.815 |
| 64x64 lossy-q60 | 28 | 1560.0 | 1552.0 | -8 | -8 | -8 | -0.513 |
| 64x64 lossy-q60-meta | 28 | 2779.0 | 2771.0 | -8 | -8 | -8 | -0.288 |
| 64x64 lossy-q60-rgba | 28 | 3835.1 | 3827.1 | -8 | -8 | -8 | -0.209 |
| 64x64 lossy-q60-ycocg | 28 | 1182.4 | 1174.4 | -8 | -8 | -8 | -0.677 |
| 64x64 lossy-q90 | 28 | 3211.0 | 3203.0 | -8 | -8 | -8 | -0.249 |

- **Small lossy files get smaller**: 8 bytes at 64 px (0.2-1.7%), net of what went (the seed tables' 5-byte
  profile record, now one feature-mask byte; the lossy version and color bytes; the envelope's length and
  always-written entropy bytes) and what came (record ids and head-record framing). The itemized split was
  not measured.
- **Small lossless files are unchanged** (net 0 bytes in all 84 cells).
- **Large files pay for the record framing**: +253 bytes per 4096 px lossless file (+0.004%), about +857 per
  lossy file (+0.03% to +0.28%, more at low quality where the file is small), about +1.8 KB with alpha.
  Every unit record now carries an `id` (design decision 3) and its own length, and the unit table hint
  repeats each span's length so a region reader can find a record from one fetch (design decision 5): before, the
  table held the only copy of the lengths.
- Metadata adds the same bytes before and after (the chunks are unchanged; orientation moved to presence
  bit 4).

### Byte-gate grid (1 MP)

`compare.py --group 3` over `main.rs` lists: 10,808 cells per build (28 images x 386 encodes, efforts
0-10, q3-q90), default and `lab` builds. Decoded pixels differ in **0** cells in either build; the two builds'
lists are identical to each other on both revisions. Per configuration, mean bytes per file and the change:

| cells | n | mean bytes before | mean bytes after | median delta | min | max | total % |
|---|---:|---:|---:|---:|---:|---:|---:|
| aq | 840 | 133286.5 | 133376.6 | +90 | +76 | +106 | +0.068 |
| balanced | 840 | 141653.4 | 141741.8 | +88 | +67 | +105 | +0.062 |
| compact | 840 | 129160.0 | 129230.0 | +64 | +46 | +118 | +0.054 |
| exact | 840 | 129855.7 | 129948.8 | +88 | +67 | +146 | +0.072 |
| fast | 840 | 144294.3 | 144426.9 | +136 | +99 | +170 | +0.092 |
| fastest | 840 | 112980.6 | 113112.4 | +136 | +100 | +170 | +0.117 |
| layouts | 840 | 173022.4 | 173115.0 | +90 | +81 | +106 | +0.054 |
| ll | 140 | 1059297.8 | 1059355.5 | +60 | +44 | +60 | +0.005 |
| ll-csum | 140 | 1059395.2 | 1059452.9 | +60 | +44 | +60 | +0.005 |
| ll-near2 | 140 | 603125.0 | 603181.8 | +60 | +44 | +60 | +0.009 |
| ll-nopal-cg64 | 140 | 1136253.7 | 1137010.1 | +765 | +573 | +765 | +0.067 |
| ll-rgba-t4 | 140 | 1416976.7 | 1417034.4 | +60 | +44 | +60 | +0.004 |
| ll-stripes16 | 28 | 1051239.3 | 1051460.9 | +223.5 | +162 | +253 | +0.021 |
| rgba-t4 | 840 | 501094.7 | 501241.5 | +150 | +115 | +168 | +0.029 |
| xyb-rl | 840 | 136975.6 | 137063.7 | +88 | +67 | +104 | +0.064 |
| xyb-shuf | 840 | 149831.6 | 149922.5 | +89 | +81 | +105 | +0.061 |
| ycocg-rl | 840 | 109566.9 | 109654.3 | +88 | +72 | +104 | +0.080 |
| ycocg-shuf | 840 | 122509.9 | 122599.8 | +89 | +81 | +105 | +0.073 |

1 MP files grow by 0.004-0.12%: lossless +44 to +60 bytes (+573 to +765 with 64-column groups, which
multiply the unit records), lossy +46 to +170 bytes.

### Presence-bit write frequencies (design decision 1)

`tools/byte-gate/presence.rs` over the byte-gate grid (default build) and `tools/byte-gate/presence.py` over
the version 2 decode-parity fixtures: how often the reference encoder writes each header field, feature and
lossy parameter.

These tables are history: they were measured with the lossy head's first version 2 layout, which put the color
scales at presence bit 6 and the quantizer floor at bit 7. The swap this section proposes landed the same day
(2026-10-10); in the current grammar (SPEC 2.8) the floor is bit 6 and the scales bit 7.

#### Byte-gate grid: 10808 files (10080 lossy)

| field | bit | written | % |
|---|---:|---:|---:|
| header coding | 0 | 10080 | 93.3 |
| header sample | 1 | 0 | 0.0 |
| header features | 2 | 9973 | 92.3 |
| header channels | 3 | 980 | 9.1 |
| header orientation | 4 | 0 | 0.0 |
| header decode_speed | 5 | 9240 | 85.5 |
| header cicp | 6 | 0 | 0.0 |
| header sig_bits | 7 | 0 | 0.0 |
| header alpha_mode | 8 | 0 | 0.0 |
| feature seed_bank_1 | 0 | 9809 | 90.8 |
| feature exact_blocks | 1 | 252 | 2.3 |
| feature near_lossless | 4 | 135 | 1.2 |
| lossy seed | 0 | 9809 | 97.3 of lossy |
| lossy entropy | 1 | 2100 | 20.8 of lossy |
| lossy matrix | 2 | 0 | 0.0 of lossy |
| lossy dc | 3 | 0 | 0.0 of lossy |
| lossy records | 4 | 10080 | 100.0 of lossy |
| lossy full_states | 5 | 7563 | 75.0 of lossy |
| lossy scale | 6 | 0 | 0.0 of lossy |
| lossy floor | 7 | 0 | 0.0 of lossy |
| lossy mul | 8 | 0 | 0.0 of lossy |

Headers whose `present` needs two bytes: 0 of 10808.

#### Decode-parity fixtures, base set: 4893 files (2304 lossy)

| field | bit | written | % |
|---|---:|---:|---:|
| header coding | 0 | 2304 | 47.1 |
| header sample | 1 | 3882 | 79.3 |
| header features | 2 | 2704 | 55.3 |
| header channels | 3 | 2919 | 59.7 |
| header orientation | 4 | 0 | 0.0 |
| header decode_speed | 5 | 1152 | 23.5 |
| header cicp | 6 | 0 | 0.0 |
| header sig_bits | 7 | 0 | 0.0 |
| header alpha_mode | 8 | 0 | 0.0 |
| feature seed_bank_1 | 0 | 1794 | 36.7 |
| feature offset_map | 11 | 1160 | 23.7 |
| lossy seed | 0 | 1794 | 77.9 of lossy |
| lossy entropy | 1 | 1728 | 75.0 of lossy |
| lossy matrix | 2 | 0 | 0.0 of lossy |
| lossy dc | 3 | 0 | 0.0 of lossy |
| lossy records | 4 | 2304 | 100.0 of lossy |
| lossy full_states | 5 | 1696 | 73.6 of lossy |
| lossy scale | 6 | 0 | 0.0 of lossy |
| lossy floor | 7 | 768 | 33.3 of lossy |
| lossy mul | 8 | 0 | 0.0 of lossy |

Headers whose `present` needs two bytes: 0 of 4893.

#### Decode-parity fixtures, extra set (frames and pages: top-level headers): 582 files (128 lossy)

| field | bit | written | % |
|---|---:|---:|---:|
| header coding | 0 | 128 | 22.0 |
| header sample | 1 | 476 | 81.8 |
| header features | 2 | 356 | 61.2 |
| header channels | 3 | 436 | 74.9 |
| header orientation | 4 | 0 | 0.0 |
| header decode_speed | 5 | 64 | 11.0 |
| header cicp | 6 | 0 | 0.0 |
| header sig_bits | 7 | 0 | 0.0 |
| header alpha_mode | 8 | 0 | 0.0 |
| feature seed_bank_1 | 0 | 128 | 22.0 |
| feature offset_map | 11 | 228 | 39.2 |
| lossy seed | 0 | 128 | 100.0 of lossy |
| lossy entropy | 1 | 96 | 75.0 of lossy |
| lossy matrix | 2 | 0 | 0.0 of lossy |
| lossy dc | 3 | 0 | 0.0 of lossy |
| lossy records | 4 | 0 | 0.0 of lossy |
| lossy full_states | 5 | 128 | 100.0 of lossy |
| lossy scale | 6 | 0 | 0.0 of lossy |
| lossy floor | 7 | 0 | 0.0 of lossy |
| lossy mul | 8 | 0 | 0.0 of lossy |

Headers whose `present` needs two bytes: 0 of 582.

- **Header:** no file in either set needs a second `present` byte: every written field is in bits 0-5. The
  order inside the first byte differs between the sets (coding, features and decode speed lead in the grid;
  sample type and channels in the parity set, which covers every sample type), which costs no bytes.
- **Lossy head:** the grid writes nothing at bit 7 or above. In the parity set the quantizer floor (bit 7) is
  written in a third of the lossy files, each paying a second `present` byte, while the color scales (bit 6)
  are never written. Swapping the two bits would save that byte; it is left for the presence re-sort before
  the format freeze.

The per-cell lists (before, after) and the 34 input images are kept outside the repository (about 290 MB);
`tools/byte-gate/prepare_sizes.py` regenerates the 64 px inputs from the byte-gate images.

## Inputs

The six 4096 x 4096 crops (raw RGB8), by SHA-256:

| file | SHA-256 |
|---|---|
| `00b851348ab37a95-4096_4096_4096.rgb8` | `5cf65abd8faa031c96f2d0e2f86046c178ed1f2408ef47e20e3c0d67447008e9` |
| `012d03fa1de7403d-4096_4096_4096.rgb8` | `9eaf928faec7a853d2d6a7f8553e56f5b7cae6d2df112af4ab351a178c4996e3` |
| `03484c8be395c40d-4096_4096_4096.rgb8` | `f2bd7ea2c4bb012ed3bae7719d28ced2316addd79b024166e0e0fe036c7d2a08` |
| `03a84e2c51c807b8-4096_4096_4096.rgb8` | `412cbf951456c4d4d5c9b7de556119305e03154d9d71f489a800e83628049325` |
| `0415bcad3f0c2039-4096_4096_4096.rgb8` | `5b8b59e9bf52161b7f31ede7d80db425e2ab24942af5b7a8300f82e12ffd7f5f` |
| `05a772d7118cef2c-4096_4096_4096.rgb8` | `319684e75c6884250903a4dcfd030d10c3c51342755a291c8f8270f858d3f67f` |
