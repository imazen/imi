//! Lab-only reversible integer YCoCg-R carrier comparison. No timing.
#![forbid(unsafe_code)]
use imij::{Channels, Cicp, ImageInfo, ImiEncoderConfig, Metadata, SampleType};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{error::Error, fs, path::Path};
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MAGIC: &[u8; 8] = b"YCGRLAB1";
const UNSPECIFIED: Cicp = Cicp {
    color_primaries: 2,
    transfer_characteristics: 2,
    matrix_coefficients: 0,
    full_range: true,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    image: String,
    class: String,
    source: String,
    source_sha256: String,
    width: u32,
    height: u32,
    bits: u8,
    sample: u8,
    cicp: [u8; 4],
}
fn need(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn lifting([r, g, b]: [i64; 3]) -> [i64; 3] {
    let co = r - b;
    let t = b + (co >> 1);
    let cg = g - t;
    [t + (cg >> 1), co, cg]
}
fn inverse([y, co, cg]: [i64; 3]) -> [i64; 3] {
    let t = y - (cg >> 1);
    let g = cg + t;
    let b = t - (co >> 1);
    [b + co, g, b]
}
fn sample(tag: u8) -> Result<SampleType> {
    match tag {
        0 => Ok(SampleType::U8),
        1 => Ok(SampleType::U16),
        3 => Ok(SampleType::U32),
        4 => Ok(SampleType::I32),
        _ => Err("unsupported carrier type".into()),
    }
}
fn values(raw: &[u8], tag: u8) -> Result<Vec<i64>> {
    let st = sample(tag)?;
    need(
        raw.len().is_multiple_of(st.bytes()),
        "unaligned sample buffer",
    )?;
    Ok(raw
        .chunks_exact(st.bytes())
        .map(|b| match st {
            SampleType::U8 => i64::from(b[0]),
            SampleType::U16 => i64::from(u16::from_ne_bytes(b.try_into().unwrap())),
            SampleType::U32 => i64::from(u32::from_ne_bytes(b.try_into().unwrap())),
            SampleType::I32 => i64::from(i32::from_ne_bytes(b.try_into().unwrap())),
            _ => unreachable!(),
        })
        .collect())
}
fn bytes(vals: &[i64], tag: u8) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(vals.len() * sample(tag)?.bytes());
    for &v in vals {
        match tag {
            0 => out.push(u8::try_from(v)?),
            1 => out.extend(u16::try_from(v)?.to_ne_bytes()),
            3 => out.extend(u32::try_from(v)?.to_ne_bytes()),
            4 => out.extend(i32::try_from(v)?.to_ne_bytes()),
            _ => return Err("unsupported carrier type".into()),
        }
    }
    Ok(out)
}
fn source_info(case: &Case) -> Result<ImageInfo> {
    let mut info = ImageInfo::new(case.width, case.height, Channels::Rgb, sample(case.sample)?)
        .with_significant_bits(&[case.bits; 3])?;
    info.cicp = Cicp {
        color_primaries: case.cicp[0],
        transfer_characteristics: case.cicp[1],
        matrix_coefficients: case.cicp[2],
        full_range: case.cicp[3] != 0,
    };
    Ok(info)
}
fn encode_native(raw: &[u8], info: &ImageInfo, effort: f32) -> Result<Vec<u8>> {
    let encoded = imij::ImiEncoderConfig::default()
        .with_effort(effort)
        .encode_samples(
            imij::ImageRef::new(raw, info.row_bytes(), info),
            &Metadata::default(),
        )?;
    let (actual_info, decoded) = imij::decode(&encoded)?;
    need(
        actual_info == *info && decoded == raw,
        "native carrier storage differs",
    )?;
    Ok(encoded)
}
fn transformed(raw: &[u8], case: &Case, layout: u8, effort: f32) -> Result<Vec<u8>> {
    let source = values(raw, case.sample)?;
    let mut triples: Vec<i64> = Vec::with_capacity(source.len());
    let max = (1i64 << case.bits) - 1;
    for rgb in source.as_chunks::<3>().0 {
        need(
            rgb.iter().all(|v| (0..=max).contains(v)),
            "source outside declared grid",
        )?;
        let v = lifting(*rgb);
        need(
            (0..=max).contains(&v[0]) && v[1..].iter().all(|x| (-max..=max).contains(x)),
            "lifting outside logical ranges",
        )?;
        need(inverse(v) == *rgb, "source lifting inverse differs")?;
        triples.extend(v);
    }
    let mut streams = Vec::new();
    let mut descriptors = Vec::new();
    let mut add = |vals: &[i64], tag: u8, channels: Channels, sig: &[u8]| -> Result<()> {
        let mut info = ImageInfo::new(case.width, case.height, channels, sample(tag)?)
            .with_significant_bits(sig)?;
        info.cicp = UNSPECIFIED;
        let native = encode_native(&bytes(vals, tag)?, &info, effort)?;
        let mut descriptor = [0u8; 16];
        descriptor[..8].copy_from_slice(&(native.len() as u64).to_le_bytes());
        descriptor[8] = tag;
        descriptor[9] = channels.count() as u8;
        descriptor[10..10 + sig.len()].copy_from_slice(sig);
        descriptors.push(descriptor);
        streams.push(native);
        Ok(())
    };
    match layout {
        1 => {
            for p in triples.as_chunks_mut::<3>().0 {
                p.swap(0, 1);
            }
            add(
                &triples,
                4,
                Channels::Rgb,
                &[case.bits + 1, case.bits, case.bits + 1],
            )?;
        }
        2 => {
            for slot in 0..3 {
                let plane: Vec<_> = triples.as_chunks::<3>().0.iter().map(|p| p[slot]).collect();
                add(
                    &plane,
                    if slot == 0 {
                        if case.bits <= 8 { 0 } else { 1 }
                    } else {
                        4
                    },
                    Channels::Gray,
                    &[case.bits + u8::from(slot != 0)],
                )?;
            }
        }
        3 => {
            let bias = 1i64 << case.bits;
            for p in triples.as_chunks_mut::<3>().0 {
                p[1] += bias;
                p[2] += bias;
                p.swap(0, 1);
            }
            add(
                &triples,
                if case.bits < 16 { 1 } else { 3 },
                Channels::Rgb,
                &[case.bits + 1, case.bits, case.bits + 1],
            )?;
        }
        _ => return Err("unknown transform layout".into()),
    }
    let mut out = Vec::new();
    out.extend(MAGIC);
    out.extend([layout, case.sample, case.bits, streams.len() as u8]);
    out.extend(case.width.to_le_bytes());
    out.extend(case.height.to_le_bytes());
    out.extend(case.cicp);
    out.extend([case.bits, case.bits + 1, case.bits + 1, 0b110]);
    out.extend([
        u8::from(cfg!(target_endian = "little")),
        u8::from(layout != 2),
        0,
        0,
    ]);
    for d in descriptors {
        out.extend(d);
    }
    for stream in streams {
        out.extend(stream);
    }
    Ok(out)
}
fn decode_wrapper(raw: &[u8]) -> Result<(ImageInfo, Vec<u8>, usize, Vec<usize>)> {
    need(
        raw.len() >= 32 && &raw[..8] == MAGIC,
        "invalid wrapper magic/header",
    )?;
    let layout = raw[8];
    let tag = raw[9];
    let bits = raw[10];
    let count = raw[11] as usize;
    need(
        (1..=3).contains(&layout) && count == if layout == 2 { 3 } else { 1 },
        "invalid layout/count",
    )?;
    need(
        (1..=16).contains(&bits) && (tag == 0 || tag == 1) && (tag != 0 || bits <= 8),
        "invalid source type/precision",
    )?;
    need(
        raw[24..28] == [bits, bits + 1, bits + 1, 6]
            && raw[28] == u8::from(cfg!(target_endian = "little"))
            && raw[29] == u8::from(layout != 2)
            && raw[30..32] == [0; 2],
        "invalid logical precision/endianness",
    )?;
    let width = u32::from_le_bytes(raw[12..16].try_into()?);
    let height = u32::from_le_bytes(raw[16..20].try_into()?);
    let case = Case {
        image: String::new(),
        class: String::new(),
        source: String::new(),
        source_sha256: String::new(),
        width,
        height,
        bits,
        sample: tag,
        cicp: raw[20..24].try_into()?,
    };
    let header = 32 + 16 * count;
    need(raw.len() >= header, "truncated descriptors")?;
    let mut cursor = header;
    let mut planes = Vec::new();
    let mut sizes = Vec::new();
    for slot in 0..count {
        let d = &raw[32 + 16 * slot..48 + 16 * slot];
        let len = usize::try_from(u64::from_le_bytes(d[..8].try_into()?))?;
        let end = cursor.checked_add(len).ok_or("length overflow")?;
        need(
            end <= raw.len() && d[13..] == [0; 3],
            "invalid stream descriptor",
        )?;
        let expected_tag = if layout == 1 {
            4
        } else if layout == 3 {
            if bits < 16 { 1 } else { 3 }
        } else if slot == 0 {
            if bits <= 8 { 0 } else { 1 }
        } else {
            4
        };
        let channels = if layout == 2 {
            Channels::Gray
        } else {
            Channels::Rgb
        };
        let sig = if layout == 2 {
            vec![bits + u8::from(slot != 0)]
        } else {
            vec![bits + 1, bits, bits + 1]
        };
        need(
            d[8] == expected_tag
                && d[9] as usize == channels.count()
                && d[10..10 + sig.len()] == sig
                && d[10 + sig.len()..13].iter().all(|&x| x == 0),
            "carrier descriptor mismatch",
        )?;
        let mut expected = ImageInfo::new(width, height, channels, sample(expected_tag)?)
            .with_significant_bits(&sig)?;
        expected.cicp = UNSPECIFIED;
        let (info, decoded) = imij::decode(&raw[cursor..end])?;
        need(info == expected, "nested info differs")?;
        planes.push(values(&decoded, expected_tag)?);
        sizes.push(len);
        cursor = end;
    }
    need(cursor == raw.len(), "trailing wrapper bytes")?;
    let npix = (width as usize)
        .checked_mul(height as usize)
        .ok_or("geometry overflow")?;
    let max = (1i64 << bits) - 1;
    let mut rgb = Vec::with_capacity(npix * 3);
    for p in 0..npix {
        let mut triple = if layout == 2 {
            [planes[0][p], planes[1][p], planes[2][p]]
        } else {
            planes[0][3 * p..3 * p + 3].try_into()?
        };
        if layout != 2 {
            triple.swap(0, 1);
        }
        if layout == 3 {
            triple[1] -= 1i64 << bits;
            triple[2] -= 1i64 << bits;
        }
        need(
            (0..=max).contains(&triple[0]) && triple[1..].iter().all(|x| (-max..=max).contains(x)),
            "decoded logical headroom differs",
        )?;
        let v = inverse(triple);
        need(
            v.iter().all(|x| (0..=max).contains(x)),
            "inverse outside source grid",
        )?;
        rgb.extend(v);
    }
    Ok((source_info(&case)?, bytes(&rgb, tag)?, header, sizes))
}
fn run(plan: &Path, dest: &Path) -> Result<()> {
    need(!dest.exists(), "output directory must be new")?;
    fs::create_dir_all(dest)?;
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(plan)?)?;
    let mut rows: Vec<Value> = Vec::new();
    for case in cases {
        let raw = fs::read(&case.source)?;
        let info = source_info(&case)?;
        need(
            raw.len() == info.row_bytes() * info.height as usize,
            "source geometry/stride differs",
        )?;
        for effort in [7.5, 10.0] {
            for (layout, name) in [
                (0, "direct"),
                (1, "signed_interleaved"),
                (2, "planes"),
                (3, "biased_interleaved"),
            ] {
                let file = if layout == 0 {
                    encode_native(&raw, &info, effort)?
                } else {
                    transformed(&raw, &case, layout, effort)?
                };
                let (actual, decoded, wrapper, native) = if layout == 0 {
                    let (i, d) = imij::decode(&file)?;
                    (i, d, 0, vec![file.len()])
                } else {
                    decode_wrapper(&file)?
                };
                need(
                    actual == info && decoded == raw,
                    "corpus inverse/storage differs",
                )?;
                let id = format!("{}-n{}-e{effort}-{name}", case.image, case.bits);
                let encoded = dest.join(format!("{id}.bin"));
                let decoded_path = dest.join(format!("{id}.raw"));
                fs::write(&encoded, &file)?;
                fs::write(&decoded_path, &decoded)?;
                rows.push(json!({"case_id":id,"image":case.image,"class":case.class,"bits":case.bits,
                    "source":case.source,"source_sha256":case.source_sha256,"effort":effort,"layout":name,
                    "width":case.width,"height":case.height,"encoded":encoded,"encoded_bytes":file.len(),
                    "wrapper_bytes":wrapper,"native_complete_bytes":native,"decoded":decoded_path,
                    "exact":true,"decoded_info_exact":true}));
            }
        }
        fs::write(dest.join("results.json"), serde_json::to_vec_pretty(&rows)?)?;
        eprintln!("completed {} n{}", case.image, case.bits);
    }
    Ok(())
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    need(
        args.len() == 3,
        "usage: ycocgr_lossless_probe CASES_JSON NEW_OUTPUT_DIRECTORY",
    )?;
    run(Path::new(&args[1]), Path::new(&args[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(rgb: [i64; 3], bits: u8) {
        let v = lifting(rgb);
        let t = (rgb[0] + rgb[2]).div_euclid(2);
        assert_eq!(v, [(t + rgb[1]).div_euclid(2), rgb[0] - rgb[2], rgb[1] - t]);
        assert_eq!(inverse(v), rgb);
        let m = (1i64 << bits) - 1;
        assert!((0..=m).contains(&v[0]) && v[1..].iter().all(|x| (-m..=m).contains(x)));
    }
    #[test]
    fn exhaustive_rgb8_and_wide_oracle() {
        let mut outside = 0u64;
        for r in 0..256 {
            for g in 0..256 {
                for b in 0..256 {
                    let rgb = [r, g, b];
                    check(rgb, 8);
                    outside +=
                        u64::from(lifting(rgb)[1..].iter().any(|x| !(-128..=127).contains(x)));
                }
            }
        }
        assert_eq!(outside, 6_641_088);
        let mut state = 0x372db59c4144a921u64;
        for bits in 1..=32 {
            let m = (1i64 << bits) - 1;
            for r in [0, 1, m / 2, m - 1, m] {
                for g in [0, 1, m / 2, m - 1, m] {
                    for b in [0, 1, m / 2, m - 1, m] {
                        check([r, g, b], bits);
                    }
                }
            }
            for _ in 0..10000 {
                let mut rgb = [0; 3];
                for v in &mut rgb {
                    state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                    *v = ((state >> 16) as i64) & m;
                }
                check(rgb, bits);
            }
        }
    }
    #[test]
    fn tiny_odd_constant_extreme_carriers() -> Result<()> {
        for (w, h) in [(1, 1), (3, 5), (17, 9)] {
            for bits in [8, 10, 12, 16] {
                let case = Case {
                    image: String::new(),
                    class: String::new(),
                    source: String::new(),
                    source_sha256: String::new(),
                    width: w,
                    height: h,
                    bits,
                    sample: if bits == 8 { 0 } else { 1 },
                    cicp: [1, 13, 0, 1],
                };
                let m = (1i64 << bits) - 1;
                for pattern in 0..4 {
                    let vals: Vec<_> = (0..w * h)
                        .flat_map(|p| match pattern {
                            0 => [0, 0, 0],
                            1 => [m, m, m],
                            2 => {
                                if p % 2 == 0 {
                                    [m, 0, 0]
                                } else {
                                    [0, m, m]
                                }
                            }
                            _ => [
                                i64::from(p * 73) & m,
                                i64::from(p * 239) & m,
                                i64::from(p * 593) & m,
                            ],
                        })
                        .collect();
                    let raw = bytes(&vals, case.sample)?;
                    for layout in 1..=3 {
                        let encoded = transformed(&raw, &case, layout, 10.0)?;
                        let (info, decoded, header, sizes) = decode_wrapper(&encoded)?;
                        assert_eq!(info, source_info(&case)?);
                        assert_eq!(decoded, raw);
                        assert_eq!(header + sizes.iter().sum::<usize>(), encoded.len());
                        let mut trailing = encoded.clone();
                        trailing.push(0);
                        assert!(decode_wrapper(&trailing).is_err());
                        let mut narrow = encoded.clone();
                        narrow[26] = bits;
                        assert!(decode_wrapper(&narrow).is_err());
                    }
                }
            }
        }
        Ok(())
    }
}
