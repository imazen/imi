//! Baseline profile conformance probe (experiments lane, 2026-10-05).
//!
//! `check` decodes each file through the public API and prints the outcome
//! class (`ok` / `corrupt:<msg>` / `unsupported:<msg>`) — the verdict feed
//! for experiments/baseline_profile_20261005/REFUSAL_TEST_PLAN.md.
//!
//! `fields` prints the byte offsets of wire fields a mutation vector would
//! patch (lossy version, color, entropy, flags, nested head version), and
//! `mutate FILE NAME HEX` writes `FILE.mut-NAME` with that field replaced —
//! a self-locating fixture generator, no hardcoded offsets.
//!
//! Usage:
//!   profile_refuse_20261005 check FILE...
//!   profile_refuse_20261005 fields FILE
//!   profile_refuse_20261005 mutate FILE FIELD HEX
//!   profile_refuse_20261005 enc DIR
//!
//! FIELD names: lossy_version, color, stripe_blocks, entropy, flags,
//! nested_version, coding, speed, sample_type.
//!
//! `enc` writes three small deterministic fixtures into DIR covering the
//! parts of the wire the frozen corpus does not exercise:
//!   enc-i32dc.imi  — U16 with extreme per-block swings → flags bit1
//!                    (nested I32 DC samples, A4 in the test plan)
//!   enc-alpha.imi  — RGBA → nested alpha head + alpha records
//!   enc-aq.imi     — U8 RGB, aq_gamma > 0 → flags bit0 + AQ codes in the
//!                    DC image (channel 3)

fn write_and_check(dir: &str, name: &str, mut enc: &[u8]) {
    let path = format!("{dir}/{name}");
    std::fs::write(&path, enc).unwrap();
    let bytes = enc.len();
    let mut rows = 0u64;
    let verdict = match imij::decode_reader(&mut enc, &imij::Limits::DEFAULT, |_, _, row| {
        rows += row.len() as u64;
        Ok(())
    }) {
        Ok((info, _)) => format!(
            "ok {}x{} ch={} sample={:?} rows={}",
            info.width,
            info.height,
            info.channels.count(),
            info.sample,
            rows
        ),
        Err(e) => class(e),
    };
    println!("{name}: {bytes} bytes; {verdict}");
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("enc") => {
            let dir = args.next().unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            // F32 HDR: U16/U32 sources are normalized to <=255 before the
            // transform (mod.rs:1261,1326 / precision.rs:115), so their
            // quantized DC stays inside i16 at every quality — only
            // unbounded linear-light input can push a DC past 32767.
            // 0/1000 blocks at q116 (DC step 1/16) quantize to ~4M.
            let (w, h) = (128u32, 128u32);
            let info = imij::ImageInfo::new(w, h, imij::Channels::Rgb, imij::SampleType::F32);
            let mut px = Vec::with_capacity(info.row_bytes() * h as usize);
            for y in 0..h {
                for x in 0..w {
                    let v = (((x / 8 + y / 8) % 2) as f32) * 1000.0;
                    for _ in 0..3 {
                        px.extend_from_slice(&v.to_ne_bytes());
                    }
                }
            }
            let cfg = imij::ImiEncoderConfig::lossy(116.0);
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-i32dc.imi", &enc);
            // F16 outer (baseline storage tier, <=16-bit) with the same
            // 0/1000 HDR blocks: nested DC still overflows i16 -> I32 DC
            // inside a file whose outer image is NOT 32-bit. This is the
            // measured witness for review §10.10: a no-`extended` build
            // refuses a baseline-outer file at the nested-head tier gate.
            let info = imij::ImageInfo::new(w, h, imij::Channels::Rgb, imij::SampleType::F16);
            let mut px = Vec::with_capacity(info.row_bytes() * h as usize);
            for y in 0..h {
                for x in 0..w {
                    let v: u16 = if (x / 8 + y / 8) % 2 == 0 { 0x63D0 } else { 0 };
                    for _ in 0..3 {
                        px.extend_from_slice(&v.to_ne_bytes());
                    }
                }
            }
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-f16i32dc.imi", &enc);
            // RGBA: exercises the nested alpha head and alpha records.
            let info = imij::ImageInfo::new(64, 64, imij::Channels::Rgba, imij::SampleType::U8);
            let px: Vec<u8> = (0..64u32 * 64)
                .flat_map(|i| [(i % 251) as u8, (i % 173) as u8, (i % 97) as u8, 200u8])
                .collect();
            let cfg = imij::ImiEncoderConfig::lossy(85.0);
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-alpha.imi", &enc);
            // AQ on: DC image carries the per-block multiplier codes.
            let info = imij::ImageInfo::new(256, 128, imij::Channels::Rgb, imij::SampleType::U8);
            let px: Vec<u8> = (0..256u32 * 128)
                .flat_map(|i| {
                    let x = i % 256;
                    let y = i / 256;
                    let t = if (x / 16 + y / 16) % 2 == 0 { 255 } else { 0 };
                    [t, (t / 2 + x as u8 / 4), t]
                })
                .collect();
            let mut cfg = imij::ImiEncoderConfig::lossy(85.0);
            cfg = cfg.tune_lossy(|t| {
                t.aq_gamma = 0.5;
            });
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-aq.imi", &enc);
            // Animation: frame 2 keeps most blocks of frame 1 → inter
            // coding emits the skip/reference map (nested flags bit2).
            // Frames are `fRAM` chunks, so `fields` won't see the inner
            // head — decode success is the check.
            let info = imij::ImageInfo::new(128, 128, imij::Channels::Rgb, imij::SampleType::U8);
            let f1: Vec<u8> = (0..128u32 * 128)
                .flat_map(|i| [(i % 97) as u8, (i % 89) as u8, (i % 83) as u8])
                .collect();
            // Frame 2 differs in the two corner pixels only: the changed
            // rect is the whole canvas while 254/256 blocks are "keep",
            // so the nested stream sets the skip-map flag with a real map.
            let mut f2 = f1.clone();
            f2[0] = f2[0].wrapping_add(17);
            let last = f2.len() - 3;
            f2[last] = f2[last].wrapping_add(17);
            let cfg = imij::FrameConfig::new(imij::ImiEncoderConfig::lossy(85.0));
            let mut fe =
                imij::FrameEncoder::animation(info.clone(), imij::Metadata::default(), cfg);
            let dur = imij::FrameDuration::from_millis(100);
            fe.push_frame(&f1, info.row_bytes(), dur).unwrap();
            fe.push_frame(&f2, info.row_bytes(), dur).unwrap();
            let anim = fe.finish().unwrap();
            write_and_check(&dir, "enc-skipmap-anim.imi", &anim);
            // Column groups: 1024-wide image at group width 256 → groups=4,
            // independent per-group record streams (s%k interleave order).
            let info = imij::ImageInfo::new(1024, 128, imij::Channels::Rgb, imij::SampleType::U8);
            let px: Vec<u8> = (0..1024u32 * 128)
                .flat_map(|i| {
                    let x = i % 1024;
                    let y = i / 1024;
                    [
                        (x % 173 + y % 61) as u8,
                        ((x / 7) % 191) as u8,
                        (y % 127) as u8,
                    ]
                })
                .collect();
            let mut cfg = imij::ImiEncoderConfig::lossy(85.0);
            cfg = cfg.with_column_groups(Some(256));
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-groups.imi", &enc);
            // Non-default channel layout: chroma subsampled → the 0x82
            // envelope carries the layouts field (version2).
            let info = imij::ImageInfo::new(128, 128, imij::Channels::Rgb, imij::SampleType::U8);
            let px: Vec<u8> = (0..128u32 * 128)
                .flat_map(|i| {
                    let x = i % 128;
                    let y = i / 128;
                    [
                        ((x * 3 + y) % 251) as u8,
                        ((x + y * 2) % 223) as u8,
                        ((x * 2 + y * 3) % 199) as u8,
                    ]
                })
                .collect();
            let full = imij::LossyChannelLayout::new(
                imij::LossyTransform::Dct8,
                imij::ChannelSampling::Full,
            );
            let sub = imij::LossyChannelLayout::new(
                imij::LossyTransform::Dct8,
                imij::ChannelSampling::HalfBoth,
            );
            let cfg = imij::ImiEncoderConfig::lossy(85.0)
                .tune_lossy(|t| t.channel_layouts = [full, sub, sub]);
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-layout.imi", &enc);
            // Overhead floor: smallest possible lossy files (one block or
            // part of one) for the complete-file envelope accounting.
            for (n, dim) in [("enc-1x1.imi", 1u32), ("enc-8x8.imi", 8u32)] {
                let info =
                    imij::ImageInfo::new(dim, dim, imij::Channels::Rgb, imij::SampleType::U8);
                let px: Vec<u8> = (0..dim * dim)
                    .flat_map(|i| [(i % 251) as u8, (i % 173) as u8, (i % 97) as u8])
                    .collect();
                let cfg = imij::ImiEncoderConfig::lossy(85.0);
                let enc = cfg
                    .encode_samples(
                        imij::ImageRef::new(&px, info.row_bytes(), &info),
                        &imij::Metadata::default(),
                    )
                    .unwrap();
                write_and_check(&dir, n, &enc);
            }
            // Gray + GrayAlpha: channels 1/2 (gray replicates to the
            // color transform per mod.rs:415-423). Coverage gap — every
            // other fixture is 3/4-channel.
            let cfg = imij::ImiEncoderConfig::lossy(85.0);
            let info = imij::ImageInfo::new(64, 64, imij::Channels::Gray, imij::SampleType::U8);
            let px: Vec<u8> = (0..64u32 * 64).map(|i| (i % 251) as u8).collect();
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-gray.imi", &enc);
            let info =
                imij::ImageInfo::new(64, 64, imij::Channels::GrayAlpha, imij::SampleType::U8);
            let px: Vec<u8> = (0..64u32 * 64)
                .flat_map(|i| [(i % 251) as u8, (255 - i % 128) as u8])
                .collect();
            let enc = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-graya.imi", &enc);
            // Lossless: R3 must-refuse-known for a lossy-only profile.
            let info = imij::ImageInfo::new(64, 64, imij::Channels::Rgb, imij::SampleType::U8);
            let px: Vec<u8> = (0..64u32 * 64)
                .flat_map(|i| [(i % 251) as u8, (i % 173) as u8, (i % 97) as u8])
                .collect();
            let enc = imij::ImiEncoderConfig::default()
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &imij::Metadata::default(),
                )
                .unwrap();
            write_and_check(&dir, "enc-lossless.imi", &enc);
        }
        Some("check") => {
            for path in args {
                let data = std::fs::read(&path).unwrap();
                let mut rows = 0u64;
                let verdict = match imij::decode_reader(
                    &mut data.as_slice(),
                    &imij::Limits::DEFAULT,
                    |_, _, row| {
                        rows += row.len() as u64;
                        Ok(())
                    },
                ) {
                    Ok((info, meta)) => {
                        let kept: Vec<String> = meta
                            .chunks
                            .iter()
                            .map(|c| String::from_utf8_lossy(&c.tag[..]).into_owned())
                            .collect();
                        format!(
                            "ok {}x{} ch={} sample={:?} rows={} kept=[{}] \
                             icc={} exif={} xmp={} mdcv={} clli={} \
                             gainmap={} orientation={}",
                            info.width,
                            info.height,
                            info.channels.count(),
                            info.sample,
                            rows,
                            kept.join(","),
                            meta.icc.is_some(),
                            meta.exif.is_some(),
                            meta.xmp.is_some(),
                            meta.mastering_display.is_some(),
                            meta.content_light_level.is_some(),
                            meta.gain_map.is_some(),
                            meta.orientation,
                        )
                    }
                    Err(e) => class(e),
                };
                println!("{path}: {verdict}");
                // Whole-buffer threaded entry point covers the second
                // public decode surface; verdicts should agree.
                if let Ok((info, _)) =
                    imij::decode_reader(&mut data.as_slice(), &imij::Limits::DEFAULT, |_, _, _| {
                        Ok(())
                    })
                {
                    let mut out = vec![0u8; info.row_bytes() * info.height as usize];
                    let v2 = match imij::decode_into_threads(&data, &mut out, info.row_bytes(), 0) {
                        Ok(i) => format!("threads:ok {}x{}", i.width, i.height),
                        Err(e) => format!("threads:{}", class(e)),
                    };
                    println!("  {v2}");
                }
            }
        }
        Some("fields") => {
            let path = args.next().expect("fields FILE");
            let data = std::fs::read(&path).unwrap();
            for (name, off) in field_offsets(&data).unwrap() {
                let cur = data
                    .get(off)
                    .map(|b| format!("{b:#04x}"))
                    .unwrap_or_else(|| "eof".into());
                println!("{name}\t{off}\t{cur}");
            }
        }
        Some("mutate") => {
            let path = args.next().expect("mutate FILE FIELD HEX");
            let field = args.next().expect("FIELD");
            let value = u8::from_str_radix(args.next().expect("HEX").trim_start_matches("0x"), 16)
                .expect("hex byte");
            let mut data = std::fs::read(&path).unwrap();
            let offsets = field_offsets(&data).unwrap();
            let off = offsets
                .iter()
                .find(|(n, _)| n == &field)
                .unwrap_or_else(|| panic!("no field {field}"))
                .1;
            assert!(off < data.len(), "field offset past eof");
            let dst = format!("{path}.mut-{field}-{value:02x}");
            data[off] = value;
            std::fs::write(&dst, &data).unwrap();
            println!("{dst}: {field} @{off} := {value:#04x}");
        }
        Some("opendata") => {
            // Rewrite the final DATA chunk to the open (len==0) form:
            // splice its length varint to a single zero byte. A10 fixture.
            let path = args.next().expect("opendata FILE");
            let mut data = std::fs::read(&path).unwrap();
            let (tag_at, len_at, len_len, payload_end) = data_chunk(&data).unwrap();
            assert_eq!(
                payload_end,
                data.len(),
                "DATA is not the last chunk (animation?)"
            );
            let payload_at = len_at + len_len;
            println!("{path}: DATA @{tag_at}, len varint {len_len}B @{len_at} -> open");
            data[len_at] = 0;
            data.drain(len_at + 1..payload_at);
            let dst = format!("{path}.opendata");
            std::fs::write(&dst, &data).unwrap();
            println!("{dst}: {} bytes", data.len());
        }
        Some("compress") => {
            // Remux iCCP/eXIf/xMPp chunks through `imij::compress_metadata`
            // (public API): emits a real mCMP method-1 record when the
            // seeded LZ4 pack wins. R9 coverage for the compressed arm.
            let path = args.next().expect("compress FILE");
            let data = std::fs::read(&path).unwrap();
            match imij::compress_metadata(&data, 16 * 1024 * 1024) {
                Ok(out) => {
                    let dst = format!("{path}.mcmp");
                    std::fs::write(&dst, &out).unwrap();
                    println!("{path} -> {dst}: {} -> {} bytes", data.len(), out.len());
                }
                Err(e) => println!("{path}: {e}"),
            }
        }
        _ => {
            eprintln!(
                "usage: check FILE... | enc DIR | fields FILE | mutate FILE FIELD HEX | opendata FILE | compress FILE"
            );
            std::process::exit(2);
        }
    }
}

/// Offset of the DATA chunk tag, its length-varint offset and length, and
/// the offset just past the payload.
fn data_chunk(data: &[u8]) -> Result<(usize, usize, usize, usize), String> {
    if !data.starts_with(b"IMIJ\r\n\x87\n") {
        return Err("signature".into());
    }
    let varint = |p: &mut usize| -> Result<u64, String> {
        let mut v = 0u64;
        for sh in (0..64).step_by(7) {
            let b = *data.get(*p).ok_or("eof")?;
            *p += 1;
            v |= ((b & 127) as u64) << sh;
            if b < 128 {
                return Ok(v);
            }
        }
        Err("varint".into())
    };
    let mut p = 8usize;
    let hlen = varint(&mut p)? as usize;
    p += hlen;
    loop {
        if p + 4 > data.len() {
            return Err("eof before DATA".into());
        }
        let tag_at = p;
        p += 4;
        let len_at = p;
        let l = varint(&mut p)? as usize;
        if data[tag_at..tag_at + 4] == *b"DATA" {
            let payload_end = p + l.saturating_sub(1);
            return Ok((tag_at, len_at, p - len_at, payload_end));
        }
        p += l.saturating_sub(1);
    }
}

fn class(e: imij::Error) -> String {
    match e {
        imij::Error::Corrupt(m) => format!("corrupt:{m}"),
        imij::Error::Unsupported(m) => format!("unsupported:{m}"),
        other => format!("{other:?}"),
    }
}

/// Wire-field offsets for the legacy header + lossy 0x81 grammar. Returns
/// the subset that exists in the file; fails on non-imij input only.
fn field_offsets(data: &[u8]) -> Result<Vec<(&'static str, usize)>, String> {
    if !data.starts_with(b"IMIJ\r\n\x87\n") {
        return Err("signature".into());
    }
    // These are version 1 wire fields (2026-10-05). Version 2 files (SPEC
    // Appendix V2.1) have none of them: no payload version or color byte, no flags
    // bytes. The refusal cases live in tests/matrix.rs now.
    let at = 8
        + data[8..]
            .iter()
            .position(|&b| b < 128)
            .ok_or("header length")?
        + 1;
    if data.get(at) != Some(&1) {
        return Err("fields/mutate read the version 1 wire only".into());
    }
    let mut p = 8usize;
    let varint = |p: &mut usize| -> Result<u64, String> {
        let mut v = 0u64;
        for sh in (0..64).step_by(7) {
            let b = *data.get(*p).ok_or("eof")?;
            *p += 1;
            v |= ((b & 127) as u64) << sh;
            if b < 128 {
                return Ok(v);
            }
        }
        Err("varint".into())
    };
    let hlen = varint(&mut p)? as usize;
    if p + hlen > data.len() {
        return Err("header beyond file".into());
    }
    let h = p;
    // Legacy header: w, h varints then a 10-byte tail; compact starts 0,1.
    let mut hp = h;
    let w = varint(&mut hp)? as usize;
    let mut out = Vec::new();
    if w != 0 {
        let _h = varint(&mut hp)?;
        let tail = hp;
        if hp + 10 > h + hlen {
            return Err("header tail".into());
        }
        out.push(("sample_type", tail + 1));
        // Legacy tail: channels,sample,premult,cicp(4),orient,coding,speed.
        // tail+8 is the coding byte, not a version field.
        out.push(("premult_flags", tail + 2));
        out.push(("cicp_fullrange", tail + 6));
        out.push(("orientation", tail + 7));
        out.push(("coding", tail + 8));
        out.push(("speed", tail + 9));
        hp += 10;
        if hp != h + hlen {
            return Err("header trailing".into());
        }
    } else {
        // Compact head (container.rs read_header_sized): version, mask,
        // w, h varints, then mask-selected fields.
        out.push(("compact_version", hp));
        let cver = *data.get(hp).ok_or("eof")?;
        hp += 1;
        out.push(("compact_mask", hp));
        let mask = *data.get(hp).ok_or("eof")?;
        hp += 1;
        if mask & !0x3f != 0 {
            return Err("reserved compact header bits".into());
        }
        let _w = varint(&mut hp)?;
        let _h = varint(&mut hp)?;
        let mut ch = 3u8;
        if mask & 1 != 0 {
            out.push(("channels", hp));
            out.push(("sample_type", hp + 1));
            ch = data[hp];
            hp += 2;
        }
        if mask & 4 != 0 {
            hp += 4;
        }
        if mask & 8 != 0 {
            hp += 1;
        }
        if mask & 0x20 != 0 {
            out.push(("speed", hp));
            hp += 1;
        }
        if cver == 2 {
            hp += ch as usize;
        }
        if hp != h + hlen {
            return Err("header trailing".into());
        }
    }
    p = h + hlen;
    // chunks until DATA
    loop {
        if p + 4 > data.len() {
            return Err("eof before DATA".into());
        }
        if data[p..p + 4] != *b"DATA" {
            p += 4;
            let l = varint(&mut p)? as usize;
            p += l.saturating_sub(1);
            continue;
        }
        p += 4;
        let l = varint(&mut p)? as usize;
        if l != 0 && p + l - 1 != data.len() {
            return Err("non-final DATA".into());
        }
        break;
    }
    out.push(("lossy_version", p));
    let ver = *data.get(p).ok_or("eof at payload")?;
    p += 1;
    if ver != 0x81 && ver != 0x82 {
        return Ok(out);
    }
    out.push(("color", p));
    out.push(("stripe_blocks", p + 1));
    p += 2;
    out.push(("groups_varint", p));
    let _groups = varint(&mut p)?;
    if ver == 0x81 {
        // fixed 34B blob: scale(12) qscale(4) floor(4) mul(12) matrix(1)
        // then the entropy byte.
        out.push(("entropy", p + 33));
        p += 34;
    } else {
        // 0x82 envelope: varint len, params — the entropy byte is the
        // last byte inside the envelope (envelope.rs:115).
        let n = varint(&mut p)? as usize;
        out.push(("compact_flags", p));
        out.push(("entropy", p + n - 1));
        p += n;
    }
    let nt = varint(&mut p)? as usize;
    p += nt;
    out.push(("flags", p));
    p += 1;
    let dl = varint(&mut p)? as usize;
    out.push(("nested_version", p));
    let _ = dl;
    Ok(out)
}
