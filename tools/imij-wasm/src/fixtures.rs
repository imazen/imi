//! `emit-fixtures OUTDIR [--hdr-png PATH]` — writes the parity/demo fixture
//! set (small PNGs via a dependency-free stored-block writer, plus raw
//! buffers for float/extended formats) and a FIXTURES.tsv manifest.
//!
//! The RGB16 PQ fixture is a crop of the admitted HDR corpus source
//! (experiments/hdr_precision/sources.json) when `--hdr-png` supplies it;
//! otherwise a synthetic PQ-coded gradient with provenance marked in the
//! manifest.

use std::io::Write;

use crate::sha256::sha256_hex;

struct Fixture {
    name: &'static str,
    data: Vec<u8>,
    /// Format descriptor for the manifest / `--raw` args, e.g. "png-rgb16".
    format: String,
    origin: String,
    /// Suggested encode args exercising this fixture.
    args: String,
}

pub fn emit(argv: &[String]) -> Result<(), String> {
    let outdir = argv
        .first()
        .ok_or("emit-fixtures OUTDIR [--hdr-png PATH]")?;
    let mut hdr_png = None;
    let mut it = argv[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--hdr-png" => {
                hdr_png = Some(it.next().ok_or("missing value for --hdr-png")?.to_string());
            }
            other => return Err(format!("unknown emit-fixtures option {other:?}")),
        }
    }
    std::fs::create_dir_all(outdir).map_err(|e| format!("{outdir}: {e}"))?;

    // Synthetic 8-bit set: deterministic gradients + blocks + noise-free
    // dithering (LCG), kept small enough to commit.
    let mut fixtures = vec![
        Fixture {
            name: "gray8.png",
            data: png(64, 48, 0, 8, &synth(64, 48, 1, false, 0)),
            format: "png-gray8".into(),
            origin: "synthetic gradient+blocks".into(),
            args: "--lossless".into(),
        },
        Fixture {
            name: "ga8.png",
            data: png(64, 48, 4, 8, &synth(64, 48, 2, true, 1)),
            format: "png-ga8".into(),
            origin: "synthetic gradient + alpha wedge".into(),
            args: "--lossless".into(),
        },
        Fixture {
            name: "rgb8.png",
            data: png(96, 64, 2, 8, &synth(96, 64, 3, false, 2)),
            format: "png-rgb8".into(),
            origin: "synthetic color gradient".into(),
            args: "--lossy --quality 90".into(),
        },
        Fixture {
            name: "rgba8.png",
            data: png(64, 48, 6, 8, &synth(64, 48, 4, true, 3)),
            format: "png-rgba8".into(),
            origin: "synthetic color gradient + real alpha ramp".into(),
            args: "--lossy --quality 90".into(),
        },
        Fixture {
            name: "odd61x37.png",
            data: png(61, 37, 2, 8, &synth(61, 37, 3, false, 4)),
            format: "png-rgb8".into(),
            origin: "synthetic odd-size gradient".into(),
            args: "--lossy --quality 90".into(),
        },
        Fixture {
            name: "gray16.png",
            data: png(64, 48, 0, 16, &synth16(64, 48, 1, 5)),
            format: "png-gray16".into(),
            origin: "synthetic 16-bit gradient".into(),
            args: "--lossless".into(),
        },
        Fixture {
            name: "dither16.png",
            data: png(96, 64, 2, 8, &synth_dither16(96, 64)),
            format: "png-rgb8".into(),
            origin: "synthetic 16-colour ordered dither (template palette stripes)".into(),
            args: "--lossless --effort 10".into(),
        },
        Fixture {
            name: "sparse16.png",
            data: png(96, 64, 2, 16, &synth_sparse16(96, 64)),
            format: "png-rgb16".into(),
            origin: "synthetic 16-bit image of 120 scattered values per channel (value tables)"
                .into(),
            args: "--lossless".into(),
        },
        Fixture {
            name: "bilevel.png",
            data: png(96, 64, 0, 8, &synth_bilevel(96, 64)),
            format: "png-gray8".into(),
            origin: "synthetic two-value ordered dither (bilevel stripes with palettes off)".into(),
            args: "--lossless --palette off".into(),
        },
    ];

    // RGB16 PQ crop: real HDR source when provided, synthetic PQ ramp else.
    let (pq_data, pq_origin) = match &hdr_png {
        Some(path) => match load_crop(path) {
            Ok(v) => (v, format!("crop of {path} (admitted hdr_precision source)")),
            Err(e) => {
                eprintln!("warning: --hdr-png {path}: {e}; emitting synthetic PQ ramp");
                (
                    synth16_pq(64, 48),
                    "synthetic PQ ramp (hdr-png unreadable)".into(),
                )
            }
        },
        None => (
            synth16_pq(64, 48),
            "synthetic PQ ramp (no --hdr-png given)".into(),
        ),
    };
    fixtures.push(Fixture {
        name: "rgb16-pq.png",
        data: png(64, 48, 2, 16, &pq_data),
        format: "png-rgb16-pq".into(),
        origin: pq_origin,
        args: "--lossy --quality 90 --cicp 9,16".into(),
    });

    // Extended/float fixtures as raw buffers (PNG cannot carry them).
    for (name, w, h, ch, s, gen) in [
        ("f16.rgb.raw", 64usize, 48usize, 3usize, "f16", 6u32),
        ("u32.rgba.raw", 48, 32, 4, "u32", 7),
        ("i32.rgb.raw", 48, 32, 3, "i32", 8),
        ("f32.rgb.raw", 48, 32, 3, "f32", 9),
    ] {
        fixtures.push(Fixture {
            name,
            data: synth_raw(w, h, ch, s, gen),
            format: format!("raw-{}x{}-{}ch-{s}", w, h, ch),
            origin: "synthetic generated".into(),
            args: format!("--raw {w} {h} {} {s}", ch_name(ch)),
        });
    }

    let mut tsv = String::from("file\tsha256\tformat\torigin\tsuggested_args\n");
    for f in &fixtures {
        let p = format!("{outdir}/{}", f.name);
        std::fs::File::create(&p)
            .and_then(|mut fp| fp.write_all(&f.data))
            .map_err(|e| format!("{p}: {e}"))?;
        tsv.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            f.name,
            sha256_hex(&f.data),
            f.format,
            f.origin,
            f.args
        ));
    }
    std::fs::write(format!("{outdir}/FIXTURES.tsv"), &tsv)
        .map_err(|e| format!("FIXTURES.tsv: {e}"))?;
    eprintln!(
        "wrote {} fixtures + FIXTURES.tsv to {outdir}",
        fixtures.len()
    );
    Ok(())
}

fn ch_name(n: usize) -> &'static str {
    match n {
        1 => "gray",
        2 => "ga",
        3 => "rgb",
        _ => "rgba",
    }
}

/// Deterministic pixel field: gradient + block steps + LCG dither.
/// `alpha_ramp` makes channel 3 (or 1 for GA) a diagonal alpha wedge.
fn synth(w: usize, h: usize, nc: usize, alpha_ramp: bool, seed: u32) -> Vec<u8> {
    let mut v = vec![0u8; w * h * nc];
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nc.min(3) {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                let g = ((x * 255 / w.max(1)) + (y * 128 / h.max(1))) as u32;
                let block = (((x / 16) + (y / 12) + c * 3) % 4) as u32 * 32;
                v[(y * w + x) * nc + c] = ((g + block + (s >> 24) % 7) % 256) as u8;
            }
            if nc == 2 || nc == 4 {
                v[(y * w + x) * nc + nc - 1] = if alpha_ramp {
                    ((x + y) * 255 / (w + h).max(1)) as u8
                } else {
                    255
                };
            }
        }
    }
    v
}

/// 4x4 Bayer thresholds, 0..16.
const BAYER4: [usize; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// Ordered dither of a two-axis gradient onto 16 colours (four red levels
/// across, four blue levels down): dense, low-colour content of the kind
/// template palette stripes code (SPEC 2.14).
fn synth_dither16(w: usize, h: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let t = BAYER4[(y % 4) * 4 + x % 4];
            let r = (x * 48 / (w - 1) + t) / 16;
            let b = (y * 48 / (h - 1) + t) / 16;
            v.extend_from_slice(&[
                (r * 85) as u8,
                (40 + 30 * ((r + b) % 4)) as u8,
                (b * 85) as u8,
            ]);
        }
    }
    v
}

/// RGB16 (big-endian, PNG's order) whose channels each draw from 120
/// scattered values along a smooth index field: what HDR reconstructions and
/// RGBE decodes look like to the coder, coded with value tables (SPEC 2.1
/// map 9).
fn synth_sparse16(w: usize, h: usize) -> Vec<u8> {
    const COUNT: usize = 120;
    let mut s = 0x2545_f491u32;
    let sets: Vec<Vec<u16>> = (0..3)
        .map(|_| {
            let mut set: Vec<u16> = (0..2 * COUNT)
                .map(|_| {
                    s ^= s << 13;
                    s ^= s >> 17;
                    s ^= s << 5;
                    s as u16
                })
                .collect();
            set.sort_unstable();
            set.dedup();
            set.truncate(COUNT);
            set
        })
        .collect();
    let mut v = Vec::with_capacity(w * h * 6);
    for y in 0..h {
        for x in 0..w {
            for (c, set) in sets.iter().enumerate() {
                let n = set.len();
                let f = (x * 3 + y * 2 + c * 17) % (2 * n);
                let i = if f < n { f } else { 2 * n - 1 - f };
                v.extend_from_slice(&set[i].to_be_bytes());
            }
        }
    }
    v
}

/// Gray values 0 and 255 only: an ordered dither of a diagonal gradient with
/// solid bars, which codes as a one-bit slot in bilevel stripes (SPEC 2.10).
fn synth_bilevel(w: usize, h: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let level = (x + y) * 16 / (w + h - 1);
            let bar = (y / 8) % 4 == 1 && x % 24 < 12;
            v.push(if bar || level > BAYER4[(y % 4) * 4 + x % 4] {
                255
            } else {
                0
            });
        }
    }
    v
}

/// 16-bit big-endian samples (PNG's storage order) for the PNG writer.
fn synth16(w: usize, h: usize, nc: usize, seed: u32) -> Vec<u8> {
    let mut v = vec![0u8; w * h * nc * 2];
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
    for y in 0..h {
        for x in 0..w {
            for c in 0..nc {
                s = s.wrapping_mul(1103515245).wrapping_add(12345);
                let g = (x * 65535 / w.max(1)) as u32 + (y * 32768 / h.max(1)) as u32;
                let val = ((g + (s >> 20) % 512) % 65536) as u16;
                v[((y * w + x) * nc + c) * 2..][..2].copy_from_slice(&val.to_be_bytes());
            }
        }
    }
    v
}

/// Synthetic PQ-coded HDR ramp: code values in the PQ range with spatial
/// structure (not a real photo — used only when no corpus crop is given).
fn synth16_pq(w: usize, h: usize) -> Vec<u8> {
    let mut v = vec![0u8; w * h * 3 * 2];
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                // PQ range ~0x0000..0xA000+, channels offset for color.
                let val = (0x800 + x * 0x140 + y * 0x60 + c * 0x900) as u16;
                v[((y * w + x) * 3 + c) * 2..][..2].copy_from_slice(&val.to_be_bytes());
            }
        }
    }
    v
}

/// Load the RGB16 HDR source and crop to the fixture size, returning
/// big-endian sample bytes for the PNG writer.
fn load_crop(path: &str) -> Result<Vec<u8>, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let out = zenpng::decode(&data, &Default::default(), &enough::Unstoppable)
        .map_err(|e| format!("{e:?}"))?;
    let d = out.pixels.descriptor();
    if d.layout() as u8 != 3 || d.channel_type() as u8 != 2 {
        return Err(format!(
            "expected RGB16 png, got layout {} type {}",
            d.layout() as u8,
            d.channel_type() as u8
        ));
    }
    let (w, h) = (64usize, 48usize);
    let (sw, sh) = (out.info.width as usize, out.info.height as usize);
    if sw < w || sh < h {
        return Err(format!("source {sw}x{sh} smaller than crop {w}x{h}"));
    }
    // Centre crop, native-endian u16 -> big-endian.
    let (x0, y0) = ((sw - w) / 2, (sh - h) / 2);
    let stride = sw * 6;
    let src = out.pixels.copy_to_contiguous_bytes();
    let src = src.as_slice();
    let mut v = vec![0u8; w * h * 6];
    for y in 0..h {
        let srow = &src[(y0 + y) * stride + x0 * 6..];
        for i in 0..w * 3 {
            let px = u16::from_ne_bytes(srow[i * 2..][..2].try_into().unwrap());
            v[(y * w * 3 + i) * 2..][..2].copy_from_slice(&px.to_be_bytes());
        }
    }
    Ok(v)
}

/// Raw fixture buffer: `ch` channels of `sample` code values, native-endian.
fn synth_raw(w: usize, h: usize, ch: usize, sample: &str, seed: u32) -> Vec<u8> {
    let bytes = match sample {
        "f16" => 2,
        "u32" | "i32" | "f32" => 4,
        _ => 1,
    };
    let mut v = vec![0u8; w * h * ch * bytes];
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
    for i in 0..w * h * ch {
        s = s.wrapping_mul(1103515245).wrapping_add(12345);
        let t = (i % (w * ch)) as f32 / (w * ch) as f32;
        let val: u32 = match sample {
            // f16 values spread across [0,1]; u32 across the full range;
            // i32 centered on 0; f32 spread across [0,4).
            "f16" => crate::convert::f32_to_f16(t + ((s >> 20) % 64) as f32 / 4096.0) as u32,
            "u32" => ((t * u32::MAX as f32) as u32).wrapping_add(s >> 8),
            "i32" => (((t - 0.5) * 4.0 * i32::MAX as f32) as i32) as u32,
            "f32" => (t * 4.0 - 0.5 + ((s >> 20) % 32) as f32 / 1024.0).to_bits(),
            _ => s >> 24,
        };
        let b = val.to_ne_bytes();
        v[i * bytes..(i + 1) * bytes].copy_from_slice(&b[..bytes]);
    }
    v
}

// -- Stored-block PNG writer (no compression; deterministic output) ---------

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &v in chunk {
            a += v as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    let mut crc_in = Vec::with_capacity(4 + body.len());
    crc_in.extend_from_slice(tag);
    crc_in.extend_from_slice(body);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

/// PNG bytes for a big-endian u8/u16 scanline buffer (filter 0 rows).
/// `color`: 0 gray, 2 rgb, 4 gray+alpha, 6 rgba.
fn png(w: usize, h: usize, color: u8, depth: u8, samples_be: &[u8]) -> Vec<u8> {
    let ch = match color {
        0 => 1,
        2 => 3,
        4 => 2,
        _ => 4,
    };
    let row = w * ch * (depth as usize / 8);
    assert_eq!(samples_be.len(), row * h, "png sample buffer size");

    let mut out = Vec::with_capacity(samples_be.len() + 256);
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.push(depth);
    ihdr.push(color);
    ihdr.extend_from_slice(&[0, 0, 0]); // deflate, filter, no interlace
    chunk(&mut out, b"IHDR", &ihdr);

    // zlib: CMF/FLG for stored deflate, stored blocks, adler32.
    let mut z = Vec::with_capacity(samples_be.len() + h + 16);
    z.extend_from_slice(&[0x78, 0x01]);
    let mut idat_raw = Vec::with_capacity(samples_be.len() + h);
    for y in 0..h {
        idat_raw.push(0); // filter: none
        idat_raw.extend_from_slice(&samples_be[y * row..(y + 1) * row]);
    }
    for blk in idat_raw.chunks(65535) {
        let last = blk.len() < 65535;
        z.push(last as u8);
        z.extend_from_slice(&(blk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!blk.len() as u16).to_le_bytes());
        z.extend_from_slice(blk);
    }
    if idat_raw.len() % 65535 == 0 {
        // A multiple of the block size still needs a final empty block.
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    z.extend_from_slice(&adler32(&idat_raw).to_be_bytes());
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}
