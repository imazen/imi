//! Matched original/zenbitmaps experiment; QB01 is a tool-only band bundle.
#![forbid(unsafe_code)]
#[path = "../../quick-pixels/core.rs"]
mod quick;
use enough::Unstoppable;
use quick::Shape;
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::{fs, hint::black_box, time::Instant};
type Result<T> = std::result::Result<T, String>;

fn one_encode(codec: &str, raw: &[u8], stride: usize, s: Shape) -> Result<Vec<u8>> {
    match codec {
        "original" | "wide" => quick::encode(
            raw,
            stride,
            s,
            (codec == "original").then_some(0),
            || Ok(()),
        )
        .map_err(str::to_owned),
        "zenbitmaps" => {
            if s.bytes != 1 || ![3, 4].contains(&s.channels) {
                return Err("QOI shape".into());
            }
            let row = s.row()?;
            let packed;
            let raw = if stride == row {
                &raw[..row * s.height]
            } else {
                packed = raw
                    .chunks(stride)
                    .take(s.height)
                    .flat_map(|r| r[..row].iter().copied())
                    .collect::<Vec<_>>();
                &packed
            };
            zenbitmaps::encode_qoi(
                raw,
                s.width as u32,
                s.height as u32,
                if s.channels == 3 {
                    zenbitmaps::PixelLayout::Rgb8
                } else {
                    zenbitmaps::PixelLayout::Rgba8
                },
                Unstoppable,
            )
            .map_err(|e| format!("{e:?}"))
        }
        _ => Err("codec".into()),
    }
}
enum Pixels<'a> {
    Original(Vec<u8>),
    Zen(zenbitmaps::DecodeOutput<'a>),
}
impl AsRef<[u8]> for Pixels<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Original(v) => v,
            Self::Zen(v) => v.pixels(),
        }
    }
}
fn one_decode<'a>(codec: &str, bytes: &'a [u8], s: Shape) -> Result<Pixels<'a>> {
    if codec == "zenbitmaps" {
        let d = zenbitmaps::decode_qoi(bytes, Unstoppable).map_err(|e| format!("{e:?}"))?;
        let layout = if s.channels == 3 {
            zenbitmaps::PixelLayout::Rgb8
        } else {
            zenbitmaps::PixelLayout::Rgba8
        };
        if (d.width as usize, d.height as usize, d.layout) != (s.width, s.height, layout) {
            return Err("decoded shape".into());
        }
        Ok(Pixels::Zen(d))
    } else {
        let row = s.row()?;
        let mut raw = vec![0; row.checked_mul(s.height).ok_or("raster size")?];
        if quick::decode_into(bytes, &mut raw, row, || Ok(()))? != s {
            return Err("decoded shape".into());
        }
        Ok(Pixels::Original(raw))
    }
}

fn image_info(s: Shape) -> imij::ImageInfo {
    imij::ImageInfo::new(
        s.width as u32,
        s.height as u32,
        match s.channels {
            1 => imij::Channels::Gray,
            2 => imij::Channels::GrayAlpha,
            3 => imij::Channels::Rgb,
            _ => imij::Channels::Rgba,
        },
        match s.bytes {
            1 => imij::SampleType::U8,
            2 => imij::SampleType::U16,
            4 => imij::SampleType::F32,
            _ => imij::SampleType::U64,
        },
    )
}

// Same band geometry and scheduling for both implementations. Indexed collection
// preserves raster order regardless of worker completion order.
fn encode(codec: &str, raw: &[u8], stride: usize, s: Shape, rows: usize) -> Result<Vec<u8>> {
    if codec.starts_with("imij") {
        let info = image_info(s);
        let rows = if rows == 0 { s.height } else { rows };
        return if codec.starts_with("imij-quick") {
            let bits = match codec {
                "imij-quick" => imij::QuickBitSmarts::Off,
                "imij-quick-packed" => imij::QuickBitSmarts::Packed,
                "imij-quick-bits" => imij::QuickBitSmarts::Packets,
                _ => return Err("quick codec".into()),
            };
            imij::ImiEncoderConfig::quick()
                .with_quick(bits)
                .with_threads(rayon::current_num_threads())
                .tune_lossless(|t| t.stripe_rows = Some(rows as u32))
                .encode_samples(
                    imij::ImageRef::new(raw, stride, &info),
                    &imij::Metadata::default(),
                )
        } else {
            let effort: f32 = codec
                .strip_prefix("imij-")
                .ok_or("codec")?
                .parse()
                .map_err(|_| "effort")?;
            imij::ImiEncoderConfig::default()
                .with_effort(effort)
                .with_threads(rayon::current_num_threads())
                .tune_lossless(|t| t.stripe_rows = Some(rows as u32))
                .encode_samples(
                    imij::ImageRef::new(raw, stride, &info),
                    &imij::Metadata::default(),
                )
        }
        .map_err(|e| format!("{e:?}"));
    }
    let row = s.row()?;
    let extent = (s.height - 1)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(row))
        .ok_or("extent")?;
    if stride < row || raw.len() < extent {
        return Err("input extent".into());
    }
    if rows == 0 {
        return one_encode(codec, raw, stride, s);
    }
    if rows > u32::MAX as usize {
        return Err("band rows".into());
    }
    let parts: Result<Vec<_>> = (0..s.height.div_ceil(rows))
        .into_par_iter()
        .map(|i| {
            let y = i * rows;
            let sub = Shape {
                height: rows.min(s.height - y),
                ..s
            };
            one_encode(codec, &raw[y * stride..], stride, sub)
        })
        .collect();
    let parts = parts?;
    let size = parts
        .iter()
        .try_fold(24usize, |n, p| n.checked_add(8)?.checked_add(p.len()))
        .ok_or("bundle size")?;
    let mut out = Vec::new();
    out.try_reserve_exact(size)
        .map_err(|_| "bundle allocation")?;
    out.extend_from_slice(b"QB01");
    for n in [s.width, s.height, s.channels, s.bytes, rows] {
        out.extend_from_slice(&(n as u32).to_le_bytes());
    }
    for p in parts {
        out.extend_from_slice(&(p.len() as u64).to_le_bytes());
        out.extend_from_slice(&p);
    }
    Ok(out)
}
fn parts(bytes: &[u8], s: Shape, rows: usize) -> Result<Vec<&[u8]>> {
    if rows == 0 {
        return Ok(vec![bytes]);
    }
    let head = bytes.get(..24).ok_or("bundle header")?;
    if &head[..4] != b"QB01" {
        return Err("bundle magic".into());
    }
    for (b, n) in head[4..]
        .as_chunks::<4>()
        .0
        .iter()
        .zip([s.width, s.height, s.channels, s.bytes, rows])
    {
        if u32::from_le_bytes(*b) as usize != n {
            return Err("bundle geometry".into());
        }
    }
    let mut cursor = 24usize;
    let mut out = Vec::new();
    for _ in 0..s.height.div_ceil(rows) {
        let end = cursor.checked_add(8).ok_or("length overflow")?;
        let size = usize::try_from(u64::from_le_bytes(
            bytes.get(cursor..end).ok_or("length")?.try_into().unwrap(),
        ))
        .map_err(|_| "length overflow")?;
        cursor = end;
        let end = cursor.checked_add(size).ok_or("span overflow")?;
        out.push(bytes.get(cursor..end).ok_or("span")?);
        cursor = end;
    }
    if cursor != bytes.len() {
        return Err("trailing bytes".into());
    }
    Ok(out)
}
fn decode<'a>(codec: &str, bytes: &'a [u8], s: Shape, rows: usize) -> Result<Pixels<'a>> {
    if codec.starts_with("imij") {
        let mut out = vec![0; s.row()?.checked_mul(s.height).ok_or("raster size")?];
        let info =
            imij::decode_into_threads(bytes, &mut out, s.row()?, rayon::current_num_threads())
                .map_err(|e| format!("{e:?}"))?;
        if info != image_info(s) {
            return Err("decoded shape".into());
        }
        return Ok(Pixels::Original(out));
    }
    if rows == 0 {
        return one_decode(codec, bytes, s);
    }
    let chunks: Result<Vec<_>> = parts(bytes, s, rows)?
        .par_iter()
        .enumerate()
        .map(|(i, b)| {
            one_decode(
                codec,
                b,
                Shape {
                    height: rows.min(s.height - i * rows),
                    ..s
                },
            )
        })
        .collect();
    let mut out = Vec::new();
    out.try_reserve_exact(s.row()?.checked_mul(s.height).ok_or("raster size")?)
        .map_err(|_| "raster allocation")?;
    for chunk in chunks? {
        out.extend_from_slice(chunk.as_ref());
    }
    Ok(Pixels::Original(out))
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn within<R: Send>(pool: &Option<rayon::ThreadPool>, f: impl FnOnce() -> R + Send) -> R {
    match pool {
        Some(pool) => pool.install(f),
        None => f(),
    }
}
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() != 8 {
        return Err("quick_pair prepare|encode|decode original|zenbitmaps|wide THREADS BAND_ROWS RAW GEOMETRY OUTPUT".into());
    }
    let threads: usize = a[3].parse().map_err(|_| "threads")?;
    let rows: usize = a[4].parse().map_err(|_| "rows")?;
    if ![1, 2, 4, 8].contains(&threads) || (rows == 0 && threads != 1) {
        return Err("thread geometry".into());
    }
    let g = fs::read_to_string(&a[6]).map_err(|e| e.to_string())?;
    let g: Vec<usize> = g
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| "geometry")?;
    if g.len() != 4 {
        return Err("geometry".into());
    }
    let s = Shape {
        width: g[0],
        height: g[1],
        channels: g[2],
        bytes: g[3],
    };
    let raw = fs::read(&a[5]).map_err(|e| e.to_string())?;
    if raw.len() != s.row()?.checked_mul(s.height).ok_or("raster size")? {
        return Err("raw size".into());
    }
    let direct = matches!(
        a[1].as_str(),
        "decode-direct-file" | "decode-into-direct-file"
    );
    let builder = rayon::ThreadPoolBuilder::new().num_threads(threads);
    let pool = if direct {
        builder.build_global().map_err(|e| e.to_string())?;
        None
    } else {
        Some(builder.build().map_err(|e| e.to_string())?)
    };
    let file_action = matches!(
        a[1].as_str(),
        "decode-file"
            | "decode-into-file"
            | "copy-file"
            | "decode-direct-file"
            | "decode-into-direct-file"
    );
    let (reference, mut encode_total_ns) = if file_action {
        (fs::read(&a[7]).map_err(|e| e.to_string())?, 0)
    } else {
        let warm_start = Instant::now();
        let bytes = within(&pool, || encode(&a[2], &raw, s.row().unwrap(), s, rows))?;
        (bytes, warm_start.elapsed().as_nanos())
    };
    // All correctness work and hashing are outside measured regions.
    for codec in if a[2] == "wide" || a[2].starts_with("imij") {
        vec![a[2].as_str()]
    } else {
        vec!["original", "zenbitmaps"]
    } {
        if within(&pool, || decode(codec, &reference, s, rows))?.as_ref() != raw {
            return Err("cross pixels".into());
        }
    }
    let mut encode_ns = 0;
    let mut decode_ns = 0;
    if a[1] == "encode" {
        let t = Instant::now();
        let encoded = within(&pool, || {
            encode(&a[2], black_box(&raw), s.row().unwrap(), s, rows)
        })?;
        encode_ns = t.elapsed().as_nanos();
        encode_total_ns += encode_ns;
        if encoded != reference {
            return Err("encode repeat".into());
        }
    } else if matches!(
        a[1].as_str(),
        "decode-into-file" | "decode-into-direct-file" | "copy-file"
    ) {
        if !a[2].starts_with("imij") {
            return Err("caller-buffer scope requires IMIJ".into());
        }
        let mut out = vec![0; raw.len()];
        let mut one = || -> Result<()> {
            within(&pool, || {
                if a[1] == "copy-file" {
                    out.par_chunks_mut(s.row().unwrap() * rows.max(1))
                        .enumerate()
                        .for_each(|(i, dest)| {
                            let at = i * s.row().unwrap() * rows.max(1);
                            dest.copy_from_slice(&raw[at..at + dest.len()]);
                        });
                    Ok(())
                } else {
                    let info = imij::decode_into_threads(
                        black_box(&reference),
                        &mut out,
                        s.row().unwrap(),
                        threads,
                    )
                    .map_err(|e| format!("{e:?}"))?;
                    if info != image_info(s) {
                        return Err("decoded shape".into());
                    }
                    Ok(())
                }
            })
        };
        for _ in 0..3 {
            one()?;
        }
        let t = Instant::now();
        for _ in 0..10 {
            one()?;
        }
        decode_ns = t.elapsed().as_nanos() / 10;
        if out != raw {
            return Err("caller-buffer pixels".into());
        }
    } else if matches!(
        a[1].as_str(),
        "decode" | "decode-file" | "decode-direct-file"
    ) {
        for _ in 0..3 {
            black_box(within(&pool, || decode(&a[2], &reference, s, rows))?);
        }
        let t = Instant::now();
        for _ in 0..10 {
            black_box(within(&pool, || {
                decode(&a[2], black_box(&reference), s, rows)
            })?);
        }
        decode_ns = t.elapsed().as_nanos() / 10;
    } else if a[1] != "prepare" {
        return Err("action".into());
    }
    if a[7] != "-" && !file_action {
        fs::write(&a[7], &reference).map_err(|e| e.to_string())?;
    }
    println!("encode_ns={encode_ns}\ndecode_ns={decode_ns}\nencode_total_ns={encode_total_ns}");
    println!("check={{\"raw_sha256\":\"{}\"}}", hash(&raw));
    println!(
        "result={{\"bytes\":{},\"pixels\":{},\"encoded_sha256\":\"{}\"}}",
        reference.len(),
        s.width * s.height,
        hash(&reference)
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strided_odd_bands_cross_decode_and_thread_identity() {
        for channels in [3, 4] {
            let s = Shape {
                width: 17,
                height: 131,
                channels,
                bytes: 1,
            };
            let row = s.row().unwrap();
            let stride = row + 13;
            let mut raw = vec![0xa5; stride * (s.height - 1) + row];
            for y in 0..s.height {
                for x in 0..row {
                    raw[y * stride + x] = ((x * 37 + y * 19) % 256) as u8;
                }
            }
            let packed: Vec<_> = raw
                .chunks(stride)
                .flat_map(|r| r[..row].iter().copied())
                .collect();
            for codec in ["original", "zenbitmaps"] {
                let mut first = None;
                for threads in [1, 2, 4, 8] {
                    let pool = rayon::ThreadPoolBuilder::new()
                        .num_threads(threads)
                        .build()
                        .unwrap();
                    let b = pool.install(|| encode(codec, &raw, stride, s, 64)).unwrap();
                    if let Some(ref old) = first {
                        assert_eq!(&b, old);
                    } else {
                        first = Some(b.clone());
                    }
                    for dec in ["original", "zenbitmaps"] {
                        assert_eq!(
                            pool.install(|| decode(dec, &b, s, 64)).unwrap().as_ref(),
                            packed
                        );
                    }
                    assert!(parts(&b[..b.len() - 1], s, 64).is_err());
                    let mut bad = b.clone();
                    bad[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
                    assert!(parts(&bad, s, 64).is_err());
                }
            }
        }
    }
    #[test]
    fn wide_bands_preserve_all_storage_bits() {
        for channels in 1..=4 {
            for bytes in [1, 2, 4, 8] {
                let s = Shape {
                    width: 7,
                    height: 5,
                    channels,
                    bytes,
                };
                let raw: Vec<_> = (0..s.row().unwrap() * s.height)
                    .map(|i| ((i * 131 + 17) % 256) as u8)
                    .collect();
                let b = encode("wide", &raw, s.row().unwrap(), s, 2).unwrap();
                assert_eq!(decode("wide", &b, s, 2).unwrap().as_ref(), raw);
            }
        }
    }
}
