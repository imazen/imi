use super::*;
use crate::planes::{Rect, SliceReader};
use crate::region::RegionReader;

/// Gradients, noise, flat patches and saturated white/black, so the
/// deringing and clamp-aware DC paths run.
fn pixels(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let (w, h, n) = (
        info.width as usize,
        info.height as usize,
        info.channels.count(),
    );
    let mut s = seed.wrapping_mul(2_654_435_761) | 1;
    let mut out = vec![0u8; w * h * n];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let v = match (x / 37 + y / 29) % 4 {
                    0 => ((x * 3 + y * 2 + c * 50) % 256) as u32,
                    1 => s % 256,
                    2 => 255,
                    _ => (((x ^ y) as u32 * 7 + c as u32 * 31) % 256) & !31,
                };
                out[(y * w + x) * n + c] = v as u8;
            }
        }
    }
    out
}

fn bounded(
    info: &ImageInfo,
    px: &[u8],
    cfg: &LossyConfig,
) -> Result<(Vec<u8>, Vec<(u8, usize, usize)>), Error> {
    let rb = info.row_bytes();
    let mut calls = Vec::new();
    let file = encode_lossy_bounded(info, &Metadata::default(), cfg, &mut |pass, y0, out| {
        calls.push((pass, y0, out.len() / rb));
        out.copy_from_slice(&px[y0 * rb..y0 * rb + out.len()]);
        Ok(())
    })?;
    Ok((file, calls))
}

#[test]
fn bounded_files_decode_to_the_whole_image_encoders_pixels() {
    use crate::DecodeSpeed as D;
    // (channels, w, h, column groups, tier, effort, quality, checksum)
    let cases: &[(Channels, u32, u32, Option<u32>, Option<D>, f32, f32, bool)] = &[
        (Channels::Rgb, 300, 260, None, None, 7.5, 80.0, false),
        (
            Channels::Rgb,
            257,
            211,
            Some(64),
            Some(D::Fast),
            7.5,
            80.0,
            true,
        ),
        (
            Channels::Rgb,
            640,
            360,
            Some(128),
            Some(D::Fastest),
            5.0,
            30.0,
            false,
        ),
        (
            Channels::Rgb,
            520,
            300,
            None,
            Some(D::Balanced),
            7.5,
            95.0,
            false,
        ),
        (
            Channels::Gray,
            333,
            190,
            Some(96),
            Some(D::Fast),
            3.0,
            60.0,
            true,
        ),
        (Channels::Gray, 64, 9, None, None, 7.5, 80.0, false),
        (
            Channels::Rgb,
            2300,
            97,
            None,
            Some(D::Fast),
            7.5,
            70.0,
            false,
        ),
        (
            Channels::Rgba,
            300,
            200,
            Some(64),
            Some(D::Fast),
            7.5,
            80.0,
            true,
        ),
        (Channels::GrayAlpha, 257, 130, None, None, 5.0, 60.0, false),
        // The AC trellis: split-Huffman and run-level rate models.
        (
            Channels::Rgb,
            640,
            360,
            Some(128),
            Some(D::Fast),
            10.0,
            70.0,
            false,
        ),
        (Channels::Rgb, 520, 300, None, None, 10.0, 85.0, true),
        (
            Channels::Rgb,
            600,
            400,
            None,
            Some(D::Balanced),
            9.0,
            60.0,
            false,
        ),
        (
            Channels::Rgba,
            400,
            300,
            Some(96),
            Some(D::Fast),
            10.0,
            75.0,
            false,
        ),
    ];
    for (i, &(ch, w, h, groups, speed, effort, quality, checksum)) in cases.iter().enumerate() {
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let px = pixels(&info, i as u32 + 1);
        let mut cfg = LossyConfig::new(quality)
            .with_effort(effort)
            .with_checksum(checksum);
        cfg.column_groups = groups;
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        let whole = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let (file, calls) = bounded(&info, &px, &cfg).unwrap();
        let (winfo, want) = crate::decode(&whole).unwrap();
        let (binfo, got) = crate::decode(&file).unwrap();
        assert_eq!(binfo, winfo, "case {i}: info");
        assert_eq!(got, want, "case {i}: pixels");
        // Each pass requests the bands once, contiguous, top to bottom.
        for pass in [1, 2] {
            let mut y = 0;
            for &(_, y0, n) in calls.iter().filter(|c| c.0 == pass) {
                assert_eq!(y0, y, "case {i}: pass {pass} order");
                y += n;
            }
            assert_eq!(y, h as usize, "case {i}: pass {pass} coverage");
        }
        // Indexed region reads of the bounded file.
        let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
        let whole_rd = RegionReader::open(SliceReader::new(&whole)).unwrap();
        assert_eq!(rd.has_unit_table(), whole_rd.has_unit_table(), "case {i}");
        assert_eq!(rd.has_file_checksum(), checksum, "case {i}");
        let bpp = info.channels.count();
        for r in [
            Rect::new(0, 0, w, h),
            Rect::new(w / 3, h / 3, (w / 2).max(1), (h / 3).max(1)),
            Rect::new(w - 1, h - 1, 1, 1),
        ] {
            let rb = r.width() as usize * bpp;
            let mut out = vec![0u8; rb * r.height() as usize];
            rd.decode_region(r, &mut out, rb).unwrap();
            for y in 0..r.height() as usize {
                let at = ((r.y() as usize + y) * w as usize + r.x() as usize) * bpp;
                assert_eq!(
                    &out[y * rb..(y + 1) * rb],
                    &want[at..at + rb],
                    "case {i}: {r:?}"
                );
            }
        }
    }
}

/// The unit table at the head (hint, or a reservation filled in place
/// after the records), and its trailing fallbacks: the bounded file opens
/// and reads regions in the same batches as the whole-image file.
#[test]
fn bounded_files_keep_every_unit_table_form() {
    use crate::v1::{UnitTableForm, with_unit_table};
    let info = ImageInfo::new(700, 300, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 21);
    let mut cfg = LossyConfig::new(75.0)
        .with_effort(7.5)
        .with_decode_speed(crate::DecodeSpeed::Fast)
        .with_checksum(true);
    cfg.column_groups = Some(128);
    let open = |file: &[u8]| {
        let mut rd = RegionReader::open(SliceReader::new(file)).unwrap();
        let opened = rd.read_stats().batches;
        rd.reset_read_stats();
        let r = Rect::new(300, 100, 90, 70);
        let mut out = vec![0u8; 90 * 70 * 3];
        rd.decode_region(r, &mut out, 90 * 3).unwrap();
        (rd.has_unit_table(), opened, rd.read_stats().batches, out)
    };
    let mut hint_open = None;
    for form in [
        UnitTableForm::Hint,
        UnitTableForm::Reserved(4),
        UnitTableForm::Reserved(1),
        UnitTableForm::ReservedTail(2),
        UnitTableForm::Trailing,
        UnitTableForm::None,
    ] {
        let (whole, file) = with_unit_table(form, || {
            let whole =
                encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
            (whole, bounded(&info, &px, &cfg).unwrap().0)
        });
        assert_eq!(
            crate::decode(&file).unwrap().1,
            crate::decode(&whole).unwrap().1,
            "{form:?}"
        );
        let (wt, wo, wr, wpx) = open(&whole);
        let (bt, bo, br, bpx) = open(&file);
        assert_eq!(
            (bt, bo, br),
            (wt, wo, wr),
            "{form:?}: table, open and region batches"
        );
        assert_eq!(bpx, wpx, "{form:?}: region pixels");
        match form {
            UnitTableForm::Hint => hint_open = Some(bo),
            // Filled in place: the index is at the head, as with the hint.
            UnitTableForm::Reserved(4) => assert_eq!(Some(bo), hint_open, "filled reservation"),
            _ => {}
        }
    }
}

/// The streamed writer: header and `DATA` head first, records as coded,
/// then the delayed fill of the head reservation (or the trailing index).
#[test]
fn streamed_output_keeps_the_head_index() {
    use std::io::Cursor;
    let info = ImageInfo::new(900, 340, Channels::Rgb, SampleType::U8);
    let px = pixels(&info, 33);
    let mut cfg = LossyConfig::new(80.0)
        .with_effort(7.5)
        .with_decode_speed(crate::DecodeSpeed::Fast);
    cfg.column_groups = Some(256);
    let rb = info.row_bytes();
    let streamed = |index, budget| {
        let mut out = Cursor::new(Vec::new());
        encode_lossy_bounded_to(
            &info,
            &Metadata::default(),
            &cfg,
            &mut |_, y0, rows| {
                rows.copy_from_slice(&px[y0 * rb..y0 * rb + rows.len()]);
                Ok(())
            },
            &mut out,
            index,
            Cursor::new(Vec::new()),
            None::<Cursor<Vec<u8>>>,
            budget,
            &how_far::NoPulse,
        )
        .map(|_| out.into_inner())
    };
    let want = crate::decode(&encode_lossy(&px, rb, &info, &Metadata::default(), &cfg).unwrap())
        .unwrap()
        .1;
    let hint = bounded(&info, &px, &cfg).unwrap().0;
    let probe = |file: &[u8]| {
        let mut rd = RegionReader::open(SliceReader::new(file)).unwrap();
        let opened = rd.read_stats().batches;
        rd.reset_read_stats();
        let r = Rect::new(400, 150, 300, 120);
        let mut out = vec![0u8; 300 * 120 * 3];
        rd.decode_region(r, &mut out, 900).unwrap();
        for y in 0..120 {
            let at = ((150 + y) * 900 + 400) * 3;
            assert_eq!(&out[y * 900..(y + 1) * 900], &want[at..at + 900]);
        }
        (rd.has_unit_table(), opened, rd.read_stats().batches)
    };
    let (_, hint_open, _) = probe(&hint);
    for (index, tail) in [
        (StreamIndex::Reserved(4), false),
        (StreamIndex::Reserved(8), false),
        (StreamIndex::Reserved(1), true),
        (StreamIndex::Trailing, true),
    ] {
        let file = streamed(index, 1 << 20).unwrap();
        assert_eq!(crate::decode(&file).unwrap().1, want, "{index:?}");
        let (table, opened, region) = probe(&file);
        assert!(table, "{index:?}: unit table at open");
        assert_eq!(region, 1, "{index:?}: region batches");
        let has_uidx = file.windows(4).rev().take(4).any(|w| w == b"uIDX");
        assert_eq!(has_uidx, tail, "{index:?}: trailing index");
        if !tail {
            // Filled in place: the index is read with the head.
            assert_eq!(opened, hint_open, "{index:?}: open batches");
        }
    }
    // The spans spool to caller scratch under an explicit budget.
    assert!(matches!(
        streamed(StreamIndex::Reserved(4), 8),
        Err(crate::stream_encode::StreamEncodeError::Codec(
            Error::TooLarge
        ))
    ));
    // Streamed file checksums: a trailing `cSUM` (before a trailing
    // index) over the final bytes, the filled reservation included.
    let mut checked = cfg.clone();
    checked.checksum = true;
    let streamed_checked = |index| {
        let mut out = Cursor::new(Vec::new());
        encode_lossy_bounded_to(
            &info,
            &Metadata::default(),
            &checked,
            &mut |_, y0, rows| {
                rows.copy_from_slice(&px[y0 * rb..y0 * rb + rows.len()]);
                Ok(())
            },
            &mut out,
            index,
            Cursor::new(Vec::new()),
            Some(Cursor::new(Vec::new())),
            1 << 20,
            &how_far::NoPulse,
        )
        .map(|_| out.into_inner())
        .unwrap()
    };
    for index in [
        StreamIndex::Reserved(4),
        StreamIndex::Reserved(1),
        StreamIndex::Trailing,
    ] {
        let file = streamed_checked(index);
        assert_eq!(crate::decode(&file).unwrap().1, want, "{index:?}");
        let (table, _, region) = probe(&file);
        assert!(table && region == 1, "{index:?}");
        let rd = RegionReader::open(SliceReader::new(&file)).unwrap();
        assert!(rd.has_file_checksum(), "{index:?}");
        // One flipped byte in the records fails verification.
        let mut bad = file.clone();
        let mid = bad.len() / 2;
        bad[mid] ^= 0x10;
        assert!(crate::decode(&bad).is_err(), "{index:?}: tampered");
    }
}

/// The streamed `cSUM` table equals the in-memory writer's over the
/// final bytes, held bytes patched after feeding, any feed split.
#[test]
fn stream_sum_matches_the_in_memory_table() {
    let mut s = 0x9e37_79b9u32;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    for len in [1usize, 700, 1024, 1025, 3000, 5000] {
        for hold in [0u64, 1, 900, 1024, 2047] {
            let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let mut sum =
                StreamSum::new(0, 10, crate::HashAlgorithm::DEFAULT, hold, 1 << 20).unwrap();
            let mut io = MemIo::default();
            let mut at = 0;
            while at < len {
                let n = (1 + next() as usize % 700).min(len - at);
                ready(sum.feed(&mut io, &bytes[at..at + n])).unwrap();
                at += n;
            }
            let patch_len = (hold as usize).min(len).min(5);
            if patch_len > 0 {
                let patch: Vec<u8> = (0..patch_len).map(|_| next() as u8).collect();
                let at = hold as usize - patch_len.min(hold as usize);
                let at = at.min(len - patch_len);
                sum.patch(at as u64, &patch);
                bytes[at..at + patch_len].copy_from_slice(&patch);
            }
            let n = ready(sum.write_chunk(&mut io, 0)).unwrap();
            assert_eq!(n as usize, io.output.len());
            assert_eq!(
                io.output,
                crate::checksum::chunk_bytes(&bytes, &[], 10, crate::HashAlgorithm::DEFAULT, 16),
                "len {len} hold {hold}"
            );
        }
    }
}

/// [`StreamIo`] over three byte vectors; source rows from `rows`.
#[derive(Default)]
pub(super) struct MemIo {
    pub(super) output: Vec<u8>,
    pub(super) index: Vec<u8>,
    pub(super) sums: Vec<u8>,
    pub(super) source: Vec<u8>,
    pub(super) row_bytes: usize,
}

impl StreamIo for MemIo {
    async fn source(
        &mut self,
        _: u8,
        y0: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let n = out.len();
        out.copy_from_slice(&self.source[y0 * self.row_bytes..][..n]);
        Ok(())
    }
    async fn write(
        &mut self,
        stream: Stream,
        at: u64,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let v = match stream {
            Stream::Output => &mut self.output,
            Stream::Index => &mut self.index,
            Stream::Checksum => &mut self.sums,
        };
        let end = at as usize + bytes.len();
        if v.len() < end {
            v.resize(end, 0);
        }
        v[at as usize..end].copy_from_slice(bytes);
        Ok(())
    }
    async fn read(
        &mut self,
        stream: Stream,
        at: u64,
        buf: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let v = match stream {
            Stream::Output => &self.output,
            Stream::Index => &self.index,
            Stream::Checksum => &self.sums,
        };
        buf.copy_from_slice(&v[at as usize..][..buf.len()]);
        Ok(())
    }
}

fn ready<T>(f: impl core::future::Future<Output = T>) -> T {
    crate::parse::payload::ready(f)
}

/// The 8-bit test texture as `info`'s sample type (native-endian).
fn samples(info: &ImageInfo, seed: u32) -> Vec<u8> {
    let base = pixels(
        &ImageInfo::new(info.width, info.height, info.channels, SampleType::U8),
        seed,
    );
    let mut out = Vec::with_capacity(base.len() * info.sample.bytes());
    for (i, &v) in base.iter().enumerate() {
        match info.sample {
            SampleType::U16 => out.extend_from_slice(
                &(u16::from(v) * 257)
                    .saturating_add(i as u16 % 7)
                    .to_ne_bytes(),
            ),
            SampleType::F16 => {
                out.extend_from_slice(&color::f32_to_f16_rne(f32::from(v) / 255.0).to_ne_bytes())
            }
            SampleType::F32 => out.extend_from_slice(&(f32::from(v) / 255.0).to_ne_bytes()),
            _ => out.push(v),
        }
    }
    out
}

/// 16-bit (XYB with a known transfer, YCoCg without) and float (XYB)
/// sources, with alpha and the trellis: same pixels as `encode_lossy`.
#[test]
fn bounded_wide_and_float_samples_match_the_whole_image_encoder() {
    use crate::DecodeSpeed as D;
    let cases: &[(Channels, SampleType, u8, u32, u32, Option<D>, f32)] = &[
        (
            Channels::Rgb,
            SampleType::U16,
            13,
            320,
            250,
            Some(D::Fast),
            7.5,
        ),
        (Channels::Rgb, SampleType::U16, 2, 300, 260, None, 7.5),
        (
            Channels::Rgba,
            SampleType::U16,
            13,
            257,
            200,
            Some(D::Fast),
            7.5,
        ),
        (
            Channels::Rgb,
            SampleType::F32,
            8,
            300,
            240,
            Some(D::Fast),
            7.5,
        ),
        (Channels::GrayAlpha, SampleType::F16, 8, 211, 157, None, 5.0),
        (
            Channels::Rgb,
            SampleType::U16,
            13,
            400,
            300,
            Some(D::Fast),
            10.0,
        ),
    ];
    for (i, &(ch, st, tc, w, h, speed, effort)) in cases.iter().enumerate() {
        let mut info = ImageInfo::new(w, h, ch, st);
        info.cicp.transfer_characteristics = tc;
        let px = samples(&info, 40 + i as u32);
        let mut cfg = LossyConfig::new(80.0).with_effort(effort);
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        let whole = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let (file, _) = bounded(&info, &px, &cfg).unwrap();
        assert_eq!(
            crate::decode(&file).unwrap(),
            crate::decode(&whole).unwrap(),
            "case {i}"
        );
    }
}

/// Declared significant bits (u8, u16, u32) and full-range u32 sources,
/// YCoCg and XYB, with alpha and the trellis: same pixels as
/// `encode_lossy`. A sample above its declared range in a later band is
/// refused when that band is read.
#[test]
fn bounded_precision_sources_match_the_whole_image_encoder() {
    use crate::DecodeSpeed as D;
    type Case = (
        Channels,
        SampleType,
        &'static [u8],
        u8,
        LossyColor,
        Option<D>,
        f32,
    );
    let cases: &[Case] = &[
        (
            Channels::Rgb,
            SampleType::U8,
            &[6, 6, 6],
            13,
            LossyColor::YCoCg,
            None,
            7.5,
        ),
        (
            Channels::Rgb,
            SampleType::U8,
            &[5, 6, 5],
            13,
            LossyColor::Xyb,
            Some(D::Fast),
            7.5,
        ),
        (
            Channels::Rgba,
            SampleType::U16,
            &[12, 12, 12, 10],
            13,
            LossyColor::YCoCg,
            None,
            5.0,
        ),
        (
            Channels::Rgb,
            SampleType::U16,
            &[10, 10, 10],
            16,
            LossyColor::Xyb,
            None,
            7.5,
        ),
        (
            Channels::Rgb,
            SampleType::U32,
            &[32, 32, 32],
            13,
            LossyColor::YCoCg,
            None,
            7.5,
        ),
        (
            Channels::Rgba,
            SampleType::U32,
            &[20, 20, 20, 32],
            13,
            LossyColor::Xyb,
            Some(D::Fast),
            7.5,
        ),
        (
            Channels::Gray,
            SampleType::U32,
            &[24],
            13,
            LossyColor::YCoCg,
            Some(D::Fastest),
            5.0,
        ),
        (
            Channels::Rgb,
            SampleType::U32,
            &[16, 16, 16],
            13,
            LossyColor::Xyb,
            None,
            10.0,
        ),
    ];
    let scaled = |info: &ImageInfo, seed: u32| -> Vec<u8> {
        let base = pixels(
            &ImageInfo::new(info.width, info.height, info.channels, SampleType::U8),
            seed,
        );
        let n = info.channels.count();
        let mut out = Vec::with_capacity(base.len() * info.sample.bytes());
        for (i, &v) in base.iter().enumerate() {
            let max = crate::mask_of(info.channel_bits(i % n).unwrap() as u32) as u64;
            let x = u64::from(v) * max / 255;
            match info.sample {
                SampleType::U8 => out.push(x as u8),
                SampleType::U16 => out.extend_from_slice(&(x as u16).to_ne_bytes()),
                _ => out.extend_from_slice(&(x as u32).to_ne_bytes()),
            }
        }
        out
    };
    for (i, &(ch, st, bits, tc, color, speed, effort)) in cases.iter().enumerate() {
        let (w, h) = if effort >= 9.0 {
            (400, 300)
        } else {
            (260, 230)
        };
        let mut info = ImageInfo::new(w, h, ch, st)
            .with_significant_bits(bits)
            .unwrap();
        info.cicp.transfer_characteristics = tc;
        let px = scaled(&info, 70 + i as u32);
        let mut cfg = LossyConfig::new(80.0).with_effort(effort);
        cfg.color = color;
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        if st == SampleType::U32 && !cfg!(feature = "extended") {
            // Both encoders refuse 32-bit samples outside the extended tier.
            assert!(matches!(
                bounded(&info, &px, &cfg),
                Err(Error::Unsupported(_))
            ));
            continue;
        }
        let whole = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let (file, _) = bounded(&info, &px, &cfg).unwrap();
        assert_eq!(
            crate::decode(&file).unwrap(),
            crate::decode(&whole).unwrap(),
            "case {i}"
        );
    }
    // An out-of-range sample in the last band.
    let info = ImageInfo::new(260, 230, Channels::Rgb, SampleType::U8)
        .with_significant_bits(&[6, 6, 6])
        .unwrap();
    let mut px = scaled(&info, 3);
    let last = px.len() - 1;
    px[last] = 64;
    let cfg = LossyConfig::new(80.0).with_effort(5.0);
    assert!(matches!(
        bounded(&info, &px, &cfg),
        Err(Error::InvalidInput(_))
    ));
}

/// Flat UI-like patches, text-like edges, noise and gradients: content
/// where exact blocks qualify.
fn page(w: usize, h: usize, n: usize, seed: u32) -> Vec<u8> {
    let mut s = seed.wrapping_mul(2_654_435_761) | 1;
    let mut out = vec![0u8; w * h * n];
    for y in 0..h {
        for x in 0..w {
            for c in 0..n {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let v = match (x / 23 + y / 17) % 5 {
                    0 => 240,
                    1 if (x + y) % 3 == 0 => 20,
                    1 => 235,
                    2 => s % 256,
                    3 => ((x * 3 + y * 2 + c * 50) % 256) as u32,
                    _ => (((x ^ y) as u32 * 7 + c as u32 * 31) % 256) & !63,
                };
                out[(y * w + x) * n + c] = v as u8;
            }
        }
    }
    out
}

/// Exact blocks: priced over the whole image by an extra source pass,
/// chosen band by band, coded in a nested image whose stripes are coded
/// in another pass and placed in pass 2. The same blocks are exact as in
/// `encode_lossy` (same per-unit counts) and the pixels are the same.
#[test]
fn bounded_exact_blocks_match_the_whole_image_encoder() {
    use crate::DecodeSpeed as D;
    type Case = (Channels, u32, u32, Option<D>, f32, Option<f32>, Option<u32>);
    let cases: &[Case] = &[
        (Channels::Rgb, 120, 100, Some(D::Compact), 7.5, None, None),
        (Channels::Rgba, 110, 90, Some(D::Compact), 5.0, None, None),
        (Channels::Rgb, 300, 260, Some(D::Fast), 7.5, Some(1.5), None),
        (Channels::Rgb, 333, 211, None, 7.5, Some(2.0), Some(64)),
        (
            Channels::Rgb,
            400,
            300,
            Some(D::Fast),
            10.0,
            Some(1.5),
            None,
        ),
        (
            Channels::Rgba,
            350,
            280,
            Some(D::Fastest),
            7.5,
            Some(3.0),
            Some(128),
        ),
    ];
    for (i, &(ch, w, h, speed, effort, ratio, groups)) in cases.iter().enumerate() {
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let px = page(w as usize, h as usize, ch.count(), 30 + i as u32);
        let mut cfg = LossyConfig::new(80.0).with_effort(effort);
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        cfg.exact_blocks = ratio;
        cfg.column_groups = groups;
        let whole = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let (file, calls) = bounded(&info, &px, &cfg).unwrap();
        let we = parse_lossy(&whole).unwrap().exact.map(|e| e.ranges);
        let be = parse_lossy(&file).unwrap().exact.map(|e| e.ranges);
        assert!(we.is_some(), "case {i}: no exact block in encode_lossy");
        assert_eq!(be, we, "case {i}: per-unit exact counts");
        assert_eq!(
            crate::decode(&file).unwrap(),
            crate::decode(&whole).unwrap(),
            "case {i}"
        );
        // Prices (pass 0), statistics (1), nested stripes (3), coding (2):
        // every band once each, top to bottom.
        for pass in [0u8, 1, 3, 2] {
            let rows: usize = calls.iter().filter(|c| c.0 == pass).map(|c| c.2).sum();
            assert_eq!(rows, h as usize, "case {i}: pass {pass}");
        }
    }
    // No qualifying block (a smooth gradient): no exact blocks, as in
    // `encode_lossy`.
    let info = ImageInfo::new(200, 150, Channels::Rgb, SampleType::U8);
    let px: Vec<u8> = (0..200 * 150 * 3).map(|i| ((i / 3) % 200) as u8).collect();
    let mut cfg = LossyConfig::new(80.0).with_effort(7.5);
    cfg.exact_blocks = Some(0.01);
    let whole = encode_lossy(&px, 600, &info, &Metadata::default(), &cfg).unwrap();
    let (file, _) = bounded(&info, &px, &cfg).unwrap();
    assert!(parse_lossy(&whole).unwrap().exact.is_none());
    assert!(parse_lossy(&file).unwrap().exact.is_none());
    assert_eq!(
        crate::decode(&file).unwrap(),
        crate::decode(&whole).unwrap()
    );
}

/// Streamed output with exact blocks: the nested stripes spool to the
/// caller scratch ahead of the index entries; both index forms and the
/// file checksum.
#[cfg(feature = "std")]
#[test]
fn streamed_exact_blocks_spool_to_scratch() {
    use std::io::Cursor;
    let info = ImageInfo::new(420, 330, Channels::Rgb, SampleType::U8);
    let px = page(420, 330, 3, 77);
    let rb = info.row_bytes();
    let mut cfg = LossyConfig::new(80.0)
        .with_effort(7.5)
        .with_decode_speed(crate::DecodeSpeed::Fast);
    cfg.exact_blocks = Some(1.5);
    cfg.column_groups = Some(128);
    let want = crate::decode(&encode_lossy(&px, rb, &info, &Metadata::default(), &cfg).unwrap())
        .unwrap()
        .1;
    for checksum in [false, true] {
        let mut cfg = cfg.clone();
        cfg.checksum = checksum;
        for index in [StreamIndex::Reserved(4), StreamIndex::Trailing] {
            let mut out = Cursor::new(Vec::new());
            let (scratch, _) = encode_lossy_bounded_to(
                &info,
                &Metadata::default(),
                &cfg,
                &mut |_, y0, rows| {
                    rows.copy_from_slice(&px[y0 * rb..y0 * rb + rows.len()]);
                    Ok(())
                },
                &mut out,
                index,
                Cursor::new(Vec::new()),
                checksum.then(|| Cursor::new(Vec::new())),
                1 << 20,
                &how_far::NoPulse,
            )
            .unwrap();
            let file = out.into_inner();
            assert!(parse_lossy(&file).unwrap().exact.is_some());
            assert!(!scratch.get_ref().is_empty());
            assert_eq!(
                crate::decode(&file).unwrap().1,
                want,
                "{index:?} {checksum}"
            );
            let mut rd = RegionReader::open(SliceReader::new(&file)).unwrap();
            let r = Rect::new(100, 90, 200, 150);
            let mut got = vec![0u8; 200 * 150 * 3];
            rd.decode_region(r, &mut got, 600).unwrap();
            for y in 0..150 {
                let at = ((90 + y) * 420 + 100) * 3;
                assert_eq!(&got[y * 600..(y + 1) * 600], &want[at..at + 600]);
            }
        }
    }
    // The nested stripes share the scratch budget with the index: room
    // for the index alone is too little.
    let index_only = 10 * plan(&info, &cfg).unwrap().record_count(true) as u64;
    let mut out = Cursor::new(Vec::new());
    assert!(matches!(
        encode_lossy_bounded_to(
            &info,
            &Metadata::default(),
            &cfg,
            &mut |_, y0, rows| {
                rows.copy_from_slice(&px[y0 * rb..y0 * rb + rows.len()]);
                Ok(())
            },
            &mut out,
            StreamIndex::Trailing,
            Cursor::new(Vec::new()),
            None::<Cursor<Vec<u8>>>,
            index_only,
            &how_far::NoPulse,
        ),
        Err(crate::stream_encode::StreamEncodeError::Codec(
            Error::TooLarge
        ))
    ));
}

/// The Compact tier: superblock merges priced by an extra source pass
/// over the whole image and chosen stripe by stripe in passes 1 and 2,
/// with and without exact blocks (whose parents never merge). Same
/// pixels, the same exact blocks and superblock declaration as
/// `encode_lossy`; at effort 10 the trellis turns merges off.
#[test]
fn bounded_superblocks_match_the_whole_image_encoder() {
    type Case = (Channels, u32, u32, f32, Option<f32>, Option<u32>, bool);
    // (channels, w, h, effort, exact ratio, column groups, split-Huffman)
    let cases: &[Case] = &[
        (Channels::Rgb, 300, 260, 7.5, None, None, false),
        (Channels::Rgba, 257, 300, 5.0, None, Some(128), false),
        (Channels::Rgb, 600, 500, 7.5, None, None, false),
        (Channels::Rgb, 400, 330, 7.5, Some(0.0), Some(64), false),
        (Channels::Rgb, 300, 260, 7.5, None, None, true),
        (Channels::Rgb, 400, 300, 10.0, None, None, false),
        // DC stripes of 16 block rows: two work bands each.
        (Channels::Rgb, 160, 1100, 7.5, None, None, false),
        (Channels::Rgba, 136, 1100, 5.0, None, Some(64), false),
    ];
    let mut splits = 0;
    for (i, &(ch, w, h, effort, ratio, groups, split_huf)) in cases.iter().enumerate() {
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let px = page(w as usize, h as usize, ch.count(), 50 + i as u32);
        let mut cfg = LossyConfig::new(80.0)
            .with_effort(effort)
            .with_decode_speed(crate::DecodeSpeed::Compact);
        cfg.exact_blocks = ratio;
        cfg.column_groups = groups;
        if split_huf {
            cfg.entropy = LossyEntropy::SplitHuf;
        }
        let whole = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let (file, calls) = bounded(&info, &px, &cfg).unwrap();
        let (wp, bp) = (parse_lossy(&whole).unwrap(), parse_lossy(&file).unwrap());
        assert_eq!(bp.sb.is_some(), wp.sb.is_some(), "case {i}: superblocks");
        assert!(wp.sb.is_some(), "case {i}: Compact merges at all efforts");
        assert_eq!(
            bp.exact.as_ref().map(|e| e.ranges.clone()),
            wp.exact.map(|e| e.ranges),
            "case {i}: per-unit exact counts"
        );
        assert_eq!(
            crate::decode(&file).unwrap(),
            crate::decode(&whole).unwrap(),
            "case {i}"
        );
        let reads = |pass: u8| -> usize { calls.iter().filter(|c| c.0 == pass).map(|c| c.2).sum() };
        assert_eq!(reads(4), h as usize, "case {i}");
        // Compact's DC stripes span many work bands (here one covers the
        // image); every read is still one work band of whole stripes.
        assert!(calls.iter().all(|c| c.2 <= 64), "case {i}: band rows");
        let split = plan(&info, &cfg).unwrap().split_dc();
        splits += usize::from(split);
        let nested = split || bp.exact.is_some();
        assert_eq!(
            reads(3),
            if nested { h as usize } else { 0 },
            "case {i}: nested pass"
        );
    }
    assert!(splits > 0, "no case spans a DC stripe over several bands");
    // No tier keeps 32 DC stripes: 4200 rows give 16-block-row stripes.
    let info = ImageInfo::new(72, 4200, Channels::Rgb, SampleType::U8);
    let px = page(72, 4200, 3, 9);
    let cfg = LossyConfig::new(80.0).with_effort(7.5);
    assert!(plan(&info, &cfg).unwrap().split_dc());
    let whole = encode_lossy(&px, 216, &info, &Metadata::default(), &cfg).unwrap();
    let (file, calls) = bounded(&info, &px, &cfg).unwrap();
    assert!(calls.iter().all(|c| c.2 <= 64));
    assert_eq!(
        crate::decode(&file).unwrap(),
        crate::decode(&whole).unwrap()
    );
}

/// Small transforms (Compact U8): the same planes split and the same
/// maps as `encode_lossy`, merges or not, with exact blocks and groups.
#[test]
fn bounded_small_transforms_match_the_whole_image_encoder() {
    // (channels, w, h, effort, exact ratio, column groups, quality)
    type Case = (Channels, u32, u32, f32, Option<f32>, Option<u32>, f32);
    let cases: &[Case] = &[
        (Channels::Rgb, 300, 260, 7.5, None, None, 80.0),
        (Channels::Rgb, 600, 500, 7.5, None, Some(128), 60.0),
        (Channels::Rgb, 400, 330, 7.5, Some(1.5), Some(64), 85.0),
        (Channels::Rgba, 257, 300, 5.0, None, None, 70.0),
        // Below the merge size: small transforms alone.
        (Channels::Rgb, 120, 100, 7.5, None, None, 80.0),
        // DC stripes over several work bands.
        (Channels::Rgb, 160, 1100, 7.5, None, None, 75.0),
    ];
    let mut joint = 0;
    for (i, &(ch, w, h, effort, ratio, groups, quality)) in cases.iter().enumerate() {
        let info = ImageInfo::new(w, h, ch, SampleType::U8);
        let px = page(w as usize, h as usize, ch.count(), 70 + i as u32);
        let mut cfg = LossyConfig::new(quality)
            .with_effort(effort)
            .with_decode_speed(crate::DecodeSpeed::Compact);
        cfg.small_transforms = true;
        cfg.exact_blocks = ratio;
        cfg.column_groups = groups;
        let whole = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let (file, _) = bounded(&info, &px, &cfg).unwrap();
        let (wp, bp) = (parse_lossy(&whole).unwrap(), parse_lossy(&file).unwrap());
        assert_eq!(
            bp.small.is_some(),
            wp.small.is_some(),
            "case {i}: joint maps"
        );
        assert_eq!(bp.sb.is_some(), wp.sb.is_some(), "case {i}: merges");
        joint += usize::from(wp.small.is_some());
        assert_eq!(
            crate::decode(&file).unwrap(),
            crate::decode(&whole).unwrap(),
            "case {i}"
        );
    }
    assert!(joint > 0, "no case split a plane");
    // As `encode_lossy`: small transforms need the Compact tier.
    let info = ImageInfo::new(300, 260, Channels::Rgb, SampleType::U8);
    let mut cfg = LossyConfig::new(80.0).with_decode_speed(crate::DecodeSpeed::Fast);
    cfg.small_transforms = true;
    assert!(matches!(
        bounded(&info, &page(300, 260, 3, 1), &cfg),
        Err(Error::Unsupported(_))
    ));
}

/// Channel layouts: the same row-local transforms, sampling and per-parent
/// choices as `encode_lossy`, which also turn off what they turn off
/// there (merges, exact blocks, the trellis).
#[test]
fn bounded_channel_layouts_match_the_whole_image_encoder() {
    use crate::DecodeSpeed as D;
    use layout::{ChannelSampling as S, LossyTransform as T};
    let l = |t, s| LossyChannelLayout::new(t, s);
    let full = l(T::Dct8, S::Full);
    // (channels, sample, w, h, layouts, tier, effort, groups, exact)
    type Case = (
        Channels,
        SampleType,
        u32,
        u32,
        [LossyChannelLayout; 3],
        Option<D>,
        f32,
        Option<u32>,
        Option<f32>,
    );
    let cases: &[Case] = &[
        (
            Channels::Rgb,
            SampleType::U8,
            300,
            260,
            [full, l(T::Dct8, S::HalfBoth), l(T::Dct8, S::HalfBoth)],
            Some(D::Fast),
            7.5,
            None,
            None,
        ),
        (
            Channels::Rgb,
            SampleType::U8,
            333,
            190,
            [l(T::Dct4, S::Full), full, l(T::Dct8, S::HalfHorizontal)],
            None,
            5.0,
            Some(96),
            None,
        ),
        (
            Channels::Rgb,
            SampleType::U8,
            400,
            300,
            [l(T::Adaptive4, S::Full), l(T::Adaptive4, S::Full), full],
            Some(D::Balanced),
            7.5,
            None,
            None,
        ),
        (
            Channels::Rgba,
            SampleType::U8,
            257,
            200,
            [full, l(T::Dct2, S::HalfVertical), l(T::Dct4, S::HalfBoth)],
            Some(D::Fast),
            7.5,
            Some(64),
            None,
        ),
        (
            Channels::Rgb,
            SampleType::U16,
            240,
            180,
            [
                l(T::Adaptive4, S::Full),
                l(T::Dct8, S::HalfBoth),
                l(T::Dct8, S::HalfBoth),
            ],
            Some(D::Fast),
            7.5,
            None,
            None,
        ),
        // Layouts turn exact blocks, merges and the trellis off.
        (
            Channels::Rgb,
            SampleType::U8,
            400,
            330,
            [full, l(T::Dct8, S::HalfBoth), l(T::Dct8, S::HalfBoth)],
            Some(D::Compact),
            7.5,
            None,
            Some(1.5),
        ),
        (
            Channels::Rgb,
            SampleType::U8,
            520,
            300,
            [full, l(T::Dct8, S::HalfBoth), l(T::Dct8, S::HalfBoth)],
            Some(D::Fast),
            10.0,
            None,
            None,
        ),
        // DC stripes over several work bands.
        (
            Channels::Rgb,
            SampleType::U8,
            160,
            1100,
            [l(T::Adaptive4, S::Full), full, l(T::Dct4, S::Full)],
            Some(D::Compact),
            7.5,
            None,
            None,
        ),
    ];
    for (i, &(ch, sample, w, h, layouts, speed, effort, groups, exact)) in cases.iter().enumerate()
    {
        let info = ImageInfo::new(w, h, ch, sample);
        let px = if sample == SampleType::U8 {
            page(w as usize, h as usize, ch.count(), 90 + i as u32)
        } else {
            samples(&info, 90 + i as u32)
        };
        let mut cfg = LossyConfig::new(75.0)
            .with_effort(effort)
            .with_channel_layouts(layouts);
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        cfg.column_groups = groups;
        cfg.exact_blocks = exact;
        let whole = encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let (file, _) = bounded(&info, &px, &cfg).unwrap();
        let (wp, bp) = (parse_lossy(&whole).unwrap(), parse_lossy(&file).unwrap());
        assert_eq!(bp.layouts, wp.layouts, "case {i}: layouts");
        assert_eq!(bp.exact.is_some(), wp.exact.is_some(), "case {i}: exact");
        assert_eq!(bp.sb.is_some(), wp.sb.is_some(), "case {i}: merges");
        assert_eq!(
            crate::decode(&file).unwrap(),
            crate::decode(&whole).unwrap(),
            "case {i}"
        );
    }
}

/// A decoder memory budget narrows the stripes and column groups exactly
/// as the whole-image encoder narrows them, with the same pixels.
#[test]
fn bounded_decoder_memory_budgets_match_the_whole_image_encoder() {
    use crate::DecodeSpeed as D;
    for (i, (w, h, speed)) in [
        (1200u32, 400u32, Some(D::Fast)),
        (900, 500, Some(D::Compact)),
        (700, 300, None),
    ]
    .into_iter()
    .enumerate()
    {
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let px = pixels(&info, 40 + i as u32);
        let mut cfg = LossyConfig::new(75.0).with_effort(7.5);
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        // Budgets from the unconstrained estimate down to a third of it,
        // so the planner narrows stripes and then groups.
        let full = cfg.estimated_decoder_memory(&info).unwrap();
        let mut seen = Vec::new();
        for budget in [full, full * 2 / 3, full / 3] {
            let cfg = cfg
                .clone()
                .with_decoder_memory_limit(crate::DecoderMemoryLimit::new(budget));
            let whole = match encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg)
            {
                Ok(f) => f,
                // Too small for this image either way.
                Err(Error::InvalidInput(_)) => {
                    assert!(matches!(
                        bounded(&info, &px, &cfg),
                        Err(Error::InvalidInput(_))
                    ));
                    seen.push(None);
                    continue;
                }
                Err(e) => panic!("{e:?}"),
            };
            let (file, _) = bounded(&info, &px, &cfg).unwrap();
            let (pw, pb) = (parse_lossy(&whole).unwrap(), parse_lossy(&file).unwrap());
            seen.push(Some((pw.ngroups(), pw.g.sb)));
            assert_eq!(
                (pb.ngroups(), pb.g.sb, pb.g.stripes()),
                (pw.ngroups(), pw.g.sb, pw.g.stripes()),
                "case {i} budget {budget}: geometry"
            );
            assert_eq!(
                crate::decode(&file).unwrap().1,
                crate::decode(&whole).unwrap().1,
                "case {i} budget {budget}: pixels"
            );
        }
        seen.dedup();
        assert!(seen.len() > 1, "case {i}: no budget narrowed: {seen:?}");
    }
}

#[test]
fn bounded_encoding_refuses_what_needs_the_whole_image() {
    let rgb = ImageInfo::new(200, 120, Channels::Rgb, SampleType::U8);
    let px = pixels(&rgb, 9);
    let unsupported = |info: &ImageInfo, cfg: LossyConfig| {
        matches!(bounded(info, &px, &cfg), Err(Error::Unsupported(_)))
    };
    // The AC trellis (effort 9 and up).
    assert!(unsupported(&rgb, LossyConfig::new(80.0).with_effort(10.0)));
    let mut aq = LossyConfig::new(80.0).with_effort(7.5);
    aq.aq_gamma = 0.5;
    assert!(unsupported(&rgb, aq));
}

/// `bounded_working_bytes` against heaptrack heap peaks of
/// `examples/bounded_lossy_rss bounded` at 8192 x 8192 and 32768 x 2048 (benchmarks
/// `bounded_lossy_2026-10-17.md`, "Working-memory bound"): (label,
/// sample, quality, effort, tier, exact ratio, checksum, measured peak).
#[test]
fn bounded_working_bytes_covers_the_heap_peak() {
    use crate::DecodeSpeed::{Compact, Fast};
    // heaptrack prints MB with two decimals; read as MiB, rounded up.
    let cases = [
        (
            "fast",
            SampleType::U8,
            7.5,
            Some(Fast),
            None,
            false,
            11083449,
        ),
        (
            "compact",
            SampleType::U8,
            7.5,
            Some(Compact),
            None,
            false,
            20342375,
        ),
        (
            "fast, exact 1.5",
            SampleType::U8,
            7.5,
            Some(Fast),
            Some(1.5),
            false,
            12142511,
        ),
        (
            "u16",
            SampleType::U16,
            7.5,
            Some(Fast),
            None,
            false,
            13002343,
        ),
        (
            "f32",
            SampleType::F32,
            7.5,
            Some(Fast),
            None,
            false,
            16022242,
        ),
        (
            "effort 10",
            SampleType::U8,
            10.0,
            Some(Fast),
            None,
            false,
            11765023,
        ),
        ("no tier", SampleType::U8, 7.5, None, None, false, 11723080),
        (
            "checksum",
            SampleType::U8,
            7.5,
            Some(Fast),
            None,
            true,
            11083449,
        ),
    ];
    // The same at 32768 x 2048: the bound grows with width as the heap does.
    let wide = [
        (
            "wide fast",
            SampleType::U8,
            7.5,
            Some(Fast),
            None,
            false,
            40097547,
        ),
        (
            "wide compact",
            SampleType::U8,
            7.5,
            Some(Compact),
            None,
            false,
            53529805,
        ),
    ];
    let cases = cases.iter().map(|c| (8192, 8192, c));
    let wide = wide.iter().map(|c| (32768, 2048, c));
    for (w, h, &(label, sample, effort, speed, exact, checksum, peak)) in cases.chain(wide) {
        let info = ImageInfo::new(w, h, Channels::Rgb, sample);
        let mut cfg = LossyConfig::new(70.0)
            .with_effort(effort)
            .with_threads(1)
            .with_checksum(checksum);
        if let Some(s) = speed {
            cfg = cfg.with_decode_speed(s);
        }
        cfg.exact_blocks = exact;
        let bound = bounded_working_bytes(&info, &cfg).unwrap();
        std::println!("{label}: bound {bound}, measured peak {peak}");
        assert!(bound >= peak, "{label}: {bound} < {peak}");
    }
}

#[cfg(all(test, feature = "std", feature = "encode"))]
mod session_terminal_tests {
    use super::super::*;
    use crate::parse::payload::ready;
    use crate::stream_encode::StreamEncodeError;
    use crate::stream_encode::spool::REPLAY_BYTES;

    #[derive(Debug, PartialEq, Eq)]
    enum Access {
        Read(u64, usize),
        Write(u64, usize),
    }

    struct ReplayIo {
        scratch: Vec<u8>,
        output: Vec<u8>,
        accesses: Vec<Access>,
        reads: usize,
        writes: usize,
        fail_read: Option<usize>,
        fail_write: Option<usize>,
        suspend_read: bool,
    }

    impl StreamIo for ReplayIo {
        async fn source(
            &mut self,
            _: u8,
            _: usize,
            _: &mut Vec<u8>,
        ) -> Result<(), StreamEncodeError> {
            panic!("replay must not read source pixels")
        }

        async fn read(
            &mut self,
            stream: Stream,
            at: u64,
            buf: &mut [u8],
        ) -> Result<(), StreamEncodeError> {
            assert_eq!(stream, Stream::Index);
            self.accesses.push(Access::Read(at, buf.len()));
            self.reads += 1;
            if self.suspend_read {
                core::future::pending::<()>().await;
            }
            if self.fail_read == Some(self.reads) {
                buf[0] = 0xff; // A partial read is not a consumable chunk.
                return Err(StreamEncodeError::Read(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "replay read failed",
                )));
            }
            buf.copy_from_slice(&self.scratch[at as usize..][..buf.len()]);
            Ok(())
        }

        async fn write(
            &mut self,
            stream: Stream,
            at: u64,
            bytes: &[u8],
        ) -> Result<(), StreamEncodeError> {
            assert_eq!(stream, Stream::Output);
            self.accesses.push(Access::Write(at, bytes.len()));
            self.writes += 1;
            let fails = self.fail_write == Some(self.writes);
            let confirmed = if fails {
                bytes.len().min(3)
            } else {
                bytes.len()
            };
            assert_eq!(at as usize, self.output.len());
            self.output.extend_from_slice(&bytes[..confirmed]);
            if fails {
                return Err(StreamEncodeError::Write(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "replay write failed",
                )));
            }
            Ok(())
        }
    }

    fn replay_fixture(len: usize) -> (Range, ReplayIo, Vec<u8>) {
        let source: Vec<_> = (0..len).map(|i| (i % 251) as u8).collect();
        let mut range =
            Range::new(Stream::Index, 17, len as u64, "replay content changed").unwrap();
        range.len = len as u64;
        range.hash.update(&source);
        let mut scratch = vec![0xa5; 17];
        scratch.extend_from_slice(&source);
        (
            range,
            ReplayIo {
                scratch,
                output: Vec::new(),
                accesses: Vec::new(),
                reads: 0,
                writes: 0,
                fail_read: None,
                fail_write: None,
                suspend_read: false,
            },
            source,
        )
    }

    // As at every production replay site: consumption succeeds before the next
    // read/verification, and both read and consumer errors leave immediately.
    async fn copy_replay(range: &Range, io: &mut ReplayIo) -> Result<(), StreamEncodeError> {
        let mut replay = range.replay();
        let mut at = 0;
        while let Some(chunk) = replay.next(io).await? {
            io.write(Stream::Output, at, chunk).await?;
            at += chunk.len() as u64;
        }
        Ok(())
    }

    fn require_send<F: core::future::Future + Send>(future: F) -> F {
        future
    }

    #[test]
    fn checked_replay_preserves_chunk_order_and_empty_completion() {
        for len in [
            0,
            1,
            REPLAY_BYTES - 1,
            REPLAY_BYTES,
            REPLAY_BYTES + 1,
            2 * REPLAY_BYTES + 17,
        ] {
            let (range, mut io, source) = replay_fixture(len);
            ready(require_send(copy_replay(&range, &mut io))).unwrap();
            assert_eq!(io.output, source);
            let mut expected = Vec::new();
            for at in (0..len).step_by(REPLAY_BYTES) {
                let n = (len - at).min(REPLAY_BYTES);
                expected.push(Access::Read(17 + at as u64, n));
                expected.push(Access::Write(at as u64, n));
            }
            assert_eq!(io.accesses, expected);
        }
    }

    #[test]
    fn checked_replay_verifies_after_the_last_consumer_effect() {
        let (range, mut io, _) = replay_fixture(REPLAY_BYTES + 17);
        io.scratch[17 + REPLAY_BYTES] ^= 1;
        assert!(matches!(
            ready(copy_replay(&range, &mut io)),
            Err(StreamEncodeError::Codec(Error::InvalidInput(
                "replay content changed"
            )))
        ));
        // A late checksum failure is not a rollback or an early-read promise.
        assert_eq!(io.output, io.scratch[17..]);
        assert_eq!((io.reads, io.writes), (2, 2));
    }

    #[test]
    fn checked_replay_stops_on_partial_read_or_write_without_retry() {
        let (range, mut io, source) = replay_fixture(2 * REPLAY_BYTES + 17);
        io.fail_read = Some(2);
        let error = ready(copy_replay(&range, &mut io)).unwrap_err();
        assert!(
            matches!(error, StreamEncodeError::Read(e) if e.kind() == std::io::ErrorKind::UnexpectedEof)
        );
        assert_eq!(io.output, source[..REPLAY_BYTES]);
        assert_eq!((io.reads, io.writes), (2, 1));

        let (range, mut io, source) = replay_fixture(2 * REPLAY_BYTES + 17);
        io.fail_write = Some(2);
        let error = ready(copy_replay(&range, &mut io)).unwrap_err();
        assert!(
            matches!(error, StreamEncodeError::Write(e) if e.kind() == std::io::ErrorKind::BrokenPipe)
        );
        assert_eq!(io.output, source[..REPLAY_BYTES + 3]);
        assert_eq!((io.reads, io.writes), (2, 2));
    }

    #[test]
    fn checked_replay_cannot_resume_an_interrupted_read() {
        use core::task::{Context, Poll, Waker};
        let (range, mut io, _) = replay_fixture(17);
        let mut replay = range.replay();
        io.suspend_read = true;
        {
            let mut next = core::pin::pin!(replay.next(&mut io));
            assert!(matches!(
                core::future::Future::poll(next.as_mut(), &mut Context::from_waker(Waker::noop())),
                Poll::Pending
            ));
        }
        io.suspend_read = false;
        assert!(matches!(
            ready(replay.next(&mut io)),
            Err(StreamEncodeError::Codec(Error::Corrupt(
                "bounded replay failed"
            )))
        ));
        assert_eq!((io.reads, io.writes), (1, 0));
    }

    #[test]
    fn native_store_future_is_send_and_commits_each_answered_record_once() {
        use core::task::{Context, Poll, Waker};
        let plan = prepare_plan(
            &ImageInfo::new(129, 97, Channels::Rgb, SampleType::U8),
            &LossyConfig::default(),
        )
        .unwrap()
        .plan;
        let mail = std::sync::Arc::new(std::sync::Mutex::new(Mailbox {
            request: Request::Idle,
            answered: false,
        }));
        let mut port = Port {
            mail: mail.clone(),
            spare: Vec::new(),
            #[cfg(feature = "lab")]
            output_limit: None,
        };
        let mut drive = Drive {
            x: &mut port,
            p: &plan,
            failure: None,
            spool: Range::new(Stream::Index, 0, 64, "stored record changed").unwrap(),
            exact_at: Vec::new(),
            dc_at: Vec::new(),
            index: None,
            sum: None,
            end: 0,
            data_len: 0,
            v4: None,
            v4_directory: crate::v4::DirectoryPlacement::Tail,
        };
        let record = [3u8, 1, 4, 1, 5];
        for kind in [Spooled::Exact, Spooled::Dc] {
            let offset = drive.spool.len;
            {
                // Compile the concrete native Port future as Send, then suspend
                // and answer its real mailbox request without an async closure.
                let mut store = core::pin::pin!(require_send(drive.store(kind, &record)));
                let mut context = Context::from_waker(Waker::noop());
                assert!(matches!(
                    core::future::Future::poll(store.as_mut(), &mut context),
                    Poll::Pending
                ));
                {
                    let mut mail = mail.lock().unwrap();
                    match &mail.request {
                        Request::Write { stream, at, buf } => {
                            assert_eq!(*stream, Stream::Index);
                            assert_eq!(*at, offset);
                            assert_eq!(buf.as_slice(), &record[..]);
                        }
                        _ => panic!("store must yield one index write"),
                    }
                    mail.answered = true;
                }
                assert!(matches!(
                    core::future::Future::poll(store.as_mut(), &mut context),
                    Poll::Ready(Ok(()))
                ));
            }
            assert_eq!(drive.spool.len, offset + record.len() as u64);
        }
        let digest = crate::checksum::digest_of(crate::HashAlgorithm::Xxh3_64, [&record[..]]);
        assert_eq!(drive.exact_at, [(0, record.len(), digest)]);
        assert_eq!(drive.dc_at, [(record.len() as u64, record.len(), digest)]);
        assert!(drive.failure.is_none());
    }

    fn session() -> Session {
        SessionPlan::new(
            ImageInfo::new(129, 97, Channels::Rgb, SampleType::U8),
            Metadata::default(),
            LossyConfig::default(),
            StreamIndex::Trailing,
            64 << 20,
        )
        .unwrap()
        .start()
        .unwrap()
    }

    fn released(session: &Session) {
        assert!(session.run.is_none());
        assert!(session.held.is_none());
        assert_eq!(std::sync::Arc::strong_count(&session.mail), 1);
        let mail = session.mail.lock().unwrap_or_else(|e| e.into_inner());
        assert!(matches!(mail.request, Request::Idle));
    }

    #[test]
    fn caller_failure_releases_suspended_native_state_before_next_step() {
        let mut session = session();
        assert!(matches!(session.step().unwrap(), Step::Rows { .. }));
        assert!(session.held.is_some());
        session.fail(StreamEncodeError::Codec(Error::Cancelled));
        released(&session);
        assert!(matches!(
            session.step(),
            Err(StreamEncodeError::Codec(Error::Cancelled))
        ));
        assert!(session.step().is_err());
        released(&session);
    }

    #[test]
    fn poisoned_mailbox_fails_and_releases_payload_without_resuming_native_work() {
        let mut session = session();
        assert!(matches!(session.step().unwrap(), Step::Rows { .. }));
        let mail = session.mail.clone();
        let panic = std::panic::catch_unwind(|| {
            let mut mail = mail.lock().unwrap();
            mail.request = Request::Rows {
                pass: 255,
                y0: 0,
                buf: vec![7; 64],
            };
            panic!("deliberate mailbox poison");
        });
        assert!(panic.is_err());
        drop(mail);
        assert!(matches!(
            session.step(),
            Err(StreamEncodeError::Codec(Error::Corrupt(
                "bounded session mailbox poisoned"
            )))
        ));
        released(&session);
        assert!(session.step().is_err());
        released(&session);
    }

    #[test]
    fn native_sample_error_releases_state_and_cannot_be_cleared() {
        let info = ImageInfo::new(129, 97, Channels::Rgb, SampleType::U8)
            .with_significant_bits(&[5; 3])
            .unwrap();
        let mut session = SessionPlan::new(
            info,
            Metadata::default(),
            LossyConfig::default(),
            StreamIndex::Trailing,
            64 << 20,
        )
        .unwrap()
        .start()
        .unwrap();
        match session.step().unwrap() {
            Step::Rows { rows, .. } => rows.fill(255),
            _ => panic!("expected source request"),
        }
        assert!(matches!(
            session.step(),
            Err(StreamEncodeError::Codec(Error::InvalidInput(
                "sample exceeds declared significant-bit range"
            )))
        ));
        released(&session);
        session.fail(StreamEncodeError::Codec(Error::Cancelled));
        assert!(matches!(
            session.step(),
            Err(StreamEncodeError::Codec(Error::Corrupt(_)))
        ));
        released(&session);
    }
}

#[cfg(all(test, feature = "std"))]
mod retained_preparation_tests {
    use super::super::*;

    #[test]
    fn native_preparation_defers_conversion_tables_until_startup() {
        let info = ImageInfo::new(65, 33, Channels::Rgb, SampleType::U16)
            .with_significant_bits(&[10, 10, 10])
            .unwrap();
        let cfg = LossyConfig::new(70.0).with_effort(0.0);
        let prepared = prepare_plan(&info, &cfg).unwrap();
        assert!(prepared.precision_input);
        assert!(prepared.plan.lut16.is_empty());
        assert!(prepared.plan.input.is_none());
        let materialized = prepared.materialize().unwrap();
        assert!(materialized.input.is_some());
    }
}

#[cfg(all(test, feature = "std", feature = "lab"))]
mod one_pass_retained_plan_tests {
    use super::super::*;

    #[test]
    fn retained_one_pass_modes_defer_tables_and_keep_exact_geometry_and_budget() {
        for bits in [8, 10, 12, 16] {
            let sample = if bits == 8 {
                SampleType::U8
            } else {
                SampleType::U16
            };
            let info = ImageInfo::new(65, 129, Channels::Rgb, sample)
                .with_significant_bits(&[bits; 3])
                .unwrap();
            let mut cfg = LossyConfig::new(70.0).with_effort(0.0);
            cfg.stripe_block_rows = 8;
            let deferred = prepare_plan(&info, &cfg).unwrap();
            assert_eq!(
                plan_working_bytes(&deferred.plan).unwrap(),
                plan_working_bytes(&plan(&info, &cfg).unwrap()).unwrap()
            );
            for budgeted in [false, true] {
                let metadata = Metadata {
                    xmp: Some(b"retained one-pass".to_vec()),
                    ..Metadata::default()
                };
                let pointer = metadata.xmp.as_ref().unwrap().as_ptr();
                let prepared = if budgeted {
                    SessionPlan::budgeted(
                        info.clone(),
                        metadata,
                        cfg.clone(),
                        64 << 20,
                        64 << 20,
                        one_pass::rate::Limits {
                            frame_bytes: 64 << 20,
                            band_record_bytes: 64 << 20,
                        },
                    )
                } else {
                    SessionPlan::one_pass(info.clone(), metadata, cfg.clone(), 64 << 20, 64 << 20)
                }
                .unwrap();
                assert_eq!(prepared.passes(), (1, 1, 64));
                assert!(prepared.preparation.plan.lut16.is_empty());
                assert!(prepared.preparation.plan.input.is_none());
                assert_eq!(prepared.metadata().xmp.as_ref().unwrap().as_ptr(), pointer);
                let mut session = prepared.start().unwrap();
                assert_eq!(session.passes(), (1, 1, 64));
                assert!(matches!(
                    session.step().unwrap(),
                    Step::Rows { pass: 2, y0: 0, .. }
                ));
            }
            assert!(
                SessionPlan::one_pass(info.clone(), Metadata::default(), cfg.clone(), 64 << 20, 1)
                    .is_err()
            );
            assert!(
                SessionPlan::budgeted(
                    info,
                    Metadata::default(),
                    cfg.clone(),
                    64 << 20,
                    1,
                    one_pass::rate::Limits {
                        frame_bytes: 64 << 20,
                        band_record_bytes: 64 << 20
                    }
                )
                .is_err()
            );
        }
    }
}
