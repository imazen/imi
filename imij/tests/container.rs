//! Container: metadata round trips, chunk semantics (critical / ancillary),
//! open `DATA` with trailing chunks, header extension, decode tiers.
// Literal V2/V3 wire mutation controls select the retained grammar explicitly.

mod common;
use common::{join, parts, put_varint, read_varint};
use imij::{
    Channels, Chunk, Cicp, Coding, DecodeSpeed, Error, GainMap, GainMapParams, ImageInfo,
    ImiEncoderConfig, Metadata, SampleType, decode, header, probe,
};

/// HDR gain map parameters: base SDR, alternate 2 stops brighter.
fn params() -> GainMapParams {
    let mut p = GainMapParams::default();
    for c in &mut p.channels {
        c.min = 0.0;
        c.max = 2.0;
    }
    p.alternate_hdr_headroom = 2.0;
    p
}

fn image(w: u32, h: u32) -> (ImageInfo, Vec<u8>) {
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let px = (0..w * h * 3)
        .map(|i| ((i * 7 + (i / 3) * 13) % 251) as u8)
        .collect();
    (info, px)
}

fn lossless(meta: &Metadata) -> Vec<u8> {
    let (info, px) = image(37, 21);
    imij::ImiEncoderConfig::default()
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), meta)
        .unwrap()
}

fn lossy(meta: &Metadata, cfg: &ImiEncoderConfig) -> Vec<u8> {
    let (info, px) = image(64, 40);
    cfg.encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), meta)
        .unwrap()
}

fn full_meta() -> Metadata {
    let gm_img = lossless(&Metadata::default());
    Metadata {
        icc: Some(vec![1, 2, 3]),
        exif: Some(b"MM\0*exif".to_vec()),
        xmp: Some(b"<x:xmpmeta/>".to_vec()),
        mastering_display: Some([7; 24]),
        diffuse_white: Some(zenpixels::hdr::DiffuseWhite::BT2408),
        content_light_level: Some([0, 15, 66, 64, 0, 7, 161, 32]),
        orientation: 6,
        gain_map: Some(GainMap {
            params: params(),
            alt_cicp: Some(Cicp::BT2100_PQ),
            alt_icc: Some(vec![9; 40]),
            image: gm_img,
        }),
        chunks: vec![
            Chunk {
                tag: *b"vnDr",
                data: b"vendor".to_vec(),
            },
            Chunk {
                tag: *b"abcd",
                data: vec![],
            },
        ],
    }
}

#[test]
fn metadata_round_trips() {
    let meta = full_meta();
    for f in [
        lossless(&meta),
        lossy(&meta, &imij::ImiEncoderConfig::lossy(80.0)),
    ] {
        let (_, m) = probe(&f).unwrap();
        assert_eq!(m, meta);
        // The gain map image is an IMI file of its own.
        let g = m.gain_map.unwrap();
        assert_eq!(decode(&g.image).unwrap().0.width, 37);
    }
}

#[test]
fn orientation_defaults_to_one_and_is_checked() {
    let f = lossless(&Metadata::default());
    assert_eq!(header(&f).unwrap().orientation, 1);
    assert_eq!(probe(&f).unwrap().1.orientation, 1);
    let (info, px) = image(4, 4);
    let bad = Metadata {
        orientation: 9,
        ..Default::default()
    };
    assert!(matches!(
        imij::ImiEncoderConfig::default()
            .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &bad),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn extra_chunk_tags_must_be_ancillary_and_unused() {
    let (info, px) = image(4, 4);
    for tag in [*b"Abcd", *b"eXIf", *b"ab1d"] {
        let m = Metadata {
            chunks: vec![Chunk { tag, data: vec![] }],
            ..Default::default()
        };
        assert!(
            matches!(
                imij::ImiEncoderConfig::default()
                    .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &m),
                Err(Error::InvalidInput(_))
            ),
            "{tag:?}"
        );
    }
}

#[test]
fn unknown_chunks_skipped_or_rejected_by_case() {
    for f in [
        lossless(&Metadata::default()),
        lossy(&Metadata::default(), &imij::ImiEncoderConfig::lossy(70.0)),
    ] {
        let want = decode(&f).unwrap();
        let (head, mut chunks) = parts(&f);
        // Ancillary chunks before and after DATA: skipped, kept as chunks.
        chunks.insert(0, (*b"frOB", vec![1, 2, 3]));
        chunks.push((*b"tAIl", vec![4]));
        let g = join(&head, &chunks, false);
        assert_eq!(decode(&g).unwrap(), want);
        let tags: Vec<[u8; 4]> = probe(&g).unwrap().1.chunks.iter().map(|c| c.tag).collect();
        assert_eq!(tags, vec![*b"frOB", *b"tAIl"]);
        // A critical chunk this decoder does not know.
        chunks.insert(0, (*b"NEWS", vec![0]));
        let g = join(&head, &chunks, false);
        assert!(matches!(decode(&g), Err(Error::Unsupported(_))));
    }
}

#[test]
fn open_data_with_trailing_chunks() {
    let meta = Metadata {
        exif: Some(b"exif-after-pixels".to_vec()),
        ..Default::default()
    };
    for f in [
        lossless(&Metadata::default()),
        lossy(&Metadata::default(), &imij::ImiEncoderConfig::lossy(70.0)),
        lossy(
            &Metadata::default(),
            &imij::ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Fast),
        ),
    ] {
        let want = decode(&f).unwrap();
        let (head, mut chunks) = parts(&f);
        // A streaming writer: DATA without a length, Exif after it.
        chunks.push((*b"eXIf", meta.exif.clone().unwrap()));
        let g = join(&head, &chunks, true);
        assert_eq!(decode(&g).unwrap(), want);
        assert_eq!(probe(&g).unwrap().1.exif, meta.exif);
        let mut sd = imij::StreamDecoder::new(&g).unwrap();
        let mut row = vec![0u8; want.0.row_bytes()];
        let mut y = 0;
        while sd.next_row(&mut row).unwrap() {
            assert_eq!(row[..], want.1[y * row.len()..(y + 1) * row.len()]);
            y += 1;
        }
        // Truncated inside the open DATA chunk.
        let cut = &g[..g.len() - 30];
        assert!(decode(cut).is_err());
    }
}

#[test]
fn header_fields_can_be_appended() {
    let f = lossless(&Metadata::default());
    let want = decode(&f).unwrap();
    let mut at = 8;
    let n = read_varint(&f, &mut at) as usize;
    // A future ignorable field: id 14 is unassigned in the global registry.
    // Header ids 2 and 4 already mean checksum and locator.
    let with_extension = |ext: &[u8]| {
        let mut g = f[..8].to_vec();
        put_varint(&mut g, (n + ext.len()) as u64);
        g.extend_from_slice(&f[at..at + n]);
        g.extend_from_slice(ext);
        g.extend_from_slice(&f[at + n..]);
        g
    };
    assert_eq!(decode(&with_extension(&[14, 1, 9])).unwrap(), want);
    // An odd id must be understood: decoding stops instead of guessing.
    assert!(matches!(
        decode(&with_extension(&[5, 1, 9])),
        Err(Error::Unsupported(_))
    ));
}

#[test]
fn decode_speed_tiers_are_recorded() {
    let f = lossless(&Metadata::default());
    let h = header(&f).unwrap();
    assert_eq!(h.coding, Coding::Lossless);
    for speed in [
        DecodeSpeed::Compact,
        DecodeSpeed::Balanced,
        DecodeSpeed::Fast,
        DecodeSpeed::Fastest,
    ] {
        let cfg = imij::ImiEncoderConfig::lossy(75.0).with_decode_speed(speed);
        assert_eq!(cfg.decode_speed(), Some(speed));
        let f = lossy(&Metadata::default(), &cfg);
        let h = header(&f).unwrap();
        assert_eq!((h.coding, h.decode_speed), (Coding::Lossy, speed));
        decode(&f).unwrap();
    }
}

#[test]
fn signature_detects_text_mode_damage() {
    let f = lossless(&Metadata::default());
    // CR LF -> LF, and a stripped high bit.
    let mut a = f.clone();
    a.remove(4);
    assert!(matches!(decode(&a), Err(Error::NotImij)));
    let mut b = f.clone();
    b[6] &= 0x7F;
    assert!(matches!(decode(&b), Err(Error::NotImij)));
}

#[test]
fn tiers_keep_run_level_where_it_decodes_as_fast() {
    // A smooth image codes few tokens: Fast keeps run-level (same bytes as
    // Compact but for the header's tier byte); a busy one switches. Above
    // the small-image size, where the cost model alone decides.
    // 16-bit input: Compact merges blocks of 8-bit images from 128 px
    // (superblocks), which changes its pixels, and never those of 16-bit
    // input (`tests/superblocks.rs` asserts it), so the two tiers differ
    // here in the entropy mode alone. Any 16-bit values would do; these are
    // v x 256 + 128. 8-bit merging: `tests/exact_blocks.rs` (the feature
    // mask matches the merges) and `tests/superblocks.rs`.
    let (w, h) = (320u32, 256u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U16);
    let wide = |v: u8| (v as u16 * 256 + 128).to_ne_bytes();
    let smooth: Vec<u8> = (0..w * h * 3)
        .flat_map(|i| wide((i % (w * 3) / 3 / 2) as u8))
        .collect();
    let mut s = 1u32;
    let busy: Vec<u8> = (0..w * h * 3)
        .flat_map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            wide(s as u8)
        })
        .collect();
    let enc = |px: &[u8], speed| {
        // Fixed stripes: tiers also pick automatic stripe heights; this
        // test is about the entropy mode alone.
        imij::ImiEncoderConfig::lossy(60.0)
            .with_decode_speed(speed)
            .tune_lossy(|t| t.stripe_block_rows = 1)
            .encode_legacy_samples(
                imij::ImageRef::new(px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap()
    };
    let (c, f) = (
        enc(&smooth, DecodeSpeed::Compact),
        enc(&smooth, DecodeSpeed::Fast),
    );
    assert_eq!(c.len(), f.len());
    assert_eq!(c.iter().zip(&f).filter(|(a, b)| a != b).count(), 1);
    assert_eq!(decode(&c).unwrap(), decode(&f).unwrap());
    let (c, f) = (
        enc(&busy, DecodeSpeed::Compact),
        enc(&busy, DecodeSpeed::Fast),
    );
    assert_ne!(c.len(), f.len());
    assert_eq!(decode(&c).unwrap(), decode(&f).unwrap());
}

#[test]
fn compact_is_never_larger_than_run_level() {
    // Small images code both ways; Compact keeps the smaller.
    for (w, h) in [(16u32, 16u32), (64, 48), (200, 120)] {
        let (info, px) = image(w, h);
        let enc = |cfg: &ImiEncoderConfig| {
            cfg.encode_legacy_samples(
                imij::ImageRef::new(&px, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap()
        };
        let rl = enc(&imij::ImiEncoderConfig::lossy(70.0));
        let c = enc(&imij::ImiEncoderConfig::lossy(70.0).with_decode_speed(DecodeSpeed::Compact));
        assert!(c.len() <= rl.len(), "{w}x{h}: {} > {}", c.len(), rl.len());
        assert_eq!(decode(&c).unwrap().1.len(), decode(&rl).unwrap().1.len());
    }
}

#[test]
fn gain_map_params_are_checked_and_bad_chunks_tolerated() {
    let (info, px) = image(8, 8);
    let gm = |p: GainMapParams, image: Vec<u8>| Metadata {
        gain_map: Some(GainMap {
            params: p,
            alt_cicp: None,
            alt_icc: None,
            image,
        }),
        ..Default::default()
    };
    // Invalid parameters (max below min) and a non-imij image are refused.
    let mut bad = params();
    bad.channels[0].max = -3.0;
    let img = lossless(&Metadata::default());
    for m in [gm(bad, img.clone()), gm(params(), vec![1, 2, 3])] {
        assert!(matches!(
            imij::ImiEncoderConfig::default()
                .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &m),
            Err(Error::InvalidInput(_))
        ));
    }
    // A gMAP chunk this decoder cannot parse: the image still decodes and
    // the chunk comes back raw.
    let f = lossless(&Metadata::default());
    let want = decode(&f).unwrap();
    let (head, mut chunks) = parts(&f);
    chunks.insert(0, (*b"gMAP", vec![0, 3, 9, 9, 9]));
    let g = join(&head, &chunks, false);
    assert_eq!(decode(&g).unwrap(), want);
    let m = probe(&g).unwrap().1;
    assert!(m.gain_map.is_none());
    assert_eq!(m.chunks.len(), 1);
}

/// A damaged chunk before `DATA` is rejected by `header` (a prefix read,
/// which keeps no metadata) as the decoder rejects it, except an `mCMP`
/// record's syntax: `header` checks what a chunk header and its first
/// payload bytes show and does not parse the record (SPEC 1.1), so where
/// the decoder fails at the record, `header` accepts the file or fails at
/// a later chunk.
#[test]
fn header_rejects_damaged_metadata_like_the_decoder() {
    let info = ImageInfo::new(8, 4, Channels::Rgb, SampleType::U8);
    let px = vec![7u8; info.row_bytes() * 4];
    let meta = Metadata {
        icc: Some((0..4096u32).map(|i| (i * 7 % 251) as u8).collect()),
        xmp: Some(b"<x:xmpmeta/>".repeat(400)),
        ..Default::default()
    };
    let plain = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    let packed = imij::compress_metadata(&plain, 1 << 20).unwrap();
    let at = packed.windows(4).position(|w| w == b"mCMP").unwrap();
    let data = packed.windows(4).position(|w| w == b"DATA").unwrap();
    assert!(at < data);
    let mut rejected = 0;
    for off in at + 4..data {
        for x in [0x01u8, 0x80, 0xFF] {
            let mut bad = packed.clone();
            bad[off] ^= x;
            let h = header(&bad).err();
            let d = decode(&bad).err();
            // header reads only the prefix: what it rejects, the decoder
            // rejects, the same way unless the decoder failed at the
            // record's syntax first.
            let at_record = matches!(
                &d,
                Some(Error::Corrupt(m) | Error::Unsupported(m)) if m.contains("metadata")
            );
            if h.is_some() {
                assert!(d.is_some(), "byte {off} ^ {x:#x}");
                if !at_record {
                    assert_eq!(h, d, "byte {off} ^ {x:#x}");
                }
                rejected += 1;
            } else if d.is_some() {
                assert!(at_record, "byte {off} ^ {x:#x}: {d:?}");
            }
        }
    }
    assert!(rejected > 0);
}

/// `file` with its `DATA` payload replaced.
fn with_data_payload(file: &[u8], payload: Vec<u8>) -> Vec<u8> {
    let (header, mut chunks) = parts(file);
    chunks.iter_mut().find(|c| &c.0 == b"DATA").unwrap().1 = payload;
    join(&header, &chunks, false)
}

/// Writers parse a lossy head for its structure only (`container::locate`):
/// a remux reads every head record a lossy file may carry, the Lossy 0.4
/// ones included, and leaves `DATA` untouched (review 2026-10-10 #6).
/// Keep blocks (record 9) occur only in the nested files of inter frames,
/// which a remux does not parse.
#[test]
fn remux_reads_every_lossy_head_record() {
    let (w, h) = (64u32, 48u32);
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let mut px = vec![255u8; (w * h * 3) as usize];
    for y in 0..h as usize {
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 3;
            if x < 32 {
                if (x % 9 < 2 && y % 13 > 2) || (y % 13 == 7 && x % 17 < 11) {
                    px[o..o + 3].copy_from_slice(&[20, 30, 90]);
                }
            } else {
                for c in 0..3 {
                    px[o + c] = ((x * 31 + y * 17 + c * 50) % 200) as u8 + 20;
                }
            }
        }
    }
    let meta = Metadata {
        xmp: Some(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>".repeat(40)),
        ..Metadata::default()
    };
    let lossy = |cfg: ImiEncoderConfig| {
        cfg.encode_legacy_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
            .unwrap()
    };
    let mut aq = imij::ImiEncoderConfig::lossy(60.0);
    aq = aq.tune_lossy(|t| {
        t.aq_gamma = 0.5;
    });
    let mut exact = imij::ImiEncoderConfig::lossy(80.0).with_decode_speed(DecodeSpeed::Compact);
    exact = exact.tune_lossy(|t| {
        t.exact_blocks = Some(3.0);
    });
    let layouts = {
        use imij::{ChannelSampling as S, LossyChannelLayout as L, LossyTransform as T};
        imij::ImiEncoderConfig::lossy(60.0).tune_lossy(|t| {
            t.channel_layouts = [
                L::new(T::Adaptive4, S::Full),
                L::new(T::Dct8, S::HalfBoth),
                L::new(T::Dct8, S::HalfBoth),
            ]
        })
    };
    let mut dc32 =
        imij::ImiEncoderConfig::lossy(116.0).tune_lossy(|t| t.color = imij::LossyColor::YCoCg);
    dc32 = dc32.with_effort(0.0);
    let ids = |f: &[u8]| -> Vec<u64> {
        let d = &f[common::data_payload(f)];
        common::payload(d, &info, true)
            .head_records
            .iter()
            .map(|r| r.0)
            .collect()
    };
    let remuxed = |f: &[u8]| {
        let r = imij::compress_metadata(f, 1 << 20).unwrap();
        assert!(r.len() < f.len(), "the XMP packet compresses");
        assert_eq!(r[common::data_payload(&r)], f[common::data_payload(f)]);
        r
    };
    // Internal 32-bit DC coding is available in every build.
    let cases = [(7, aq), (5, exact), (27, layouts), (11, dc32)];
    for (record, cfg) in cases {
        let f = lossy(cfg);
        assert!(ids(&f).contains(&record), "record {record}: {:?}", ids(&f));
        let r = remuxed(&f);
        assert_eq!(
            decode(&r).unwrap().1,
            decode(&f).unwrap().1,
            "record {record}"
        );
    }
    // The Lossy 0.4 records (13, 15, 17), the explicit-table matrix and an
    // unknown even record, declared: a remux reads them, and the decoder
    // decodes them.
    // A YCoCg file: the signalled color transform (record 17) is defined
    // for the YCoCg family.
    let f = lossy(
        imij::ImiEncoderConfig::lossy(60.0).tune_lossy(|t| t.color = imij::LossyColor::YCoCg),
    );
    let d = f[common::data_payload(&f)].to_vec();
    let mut p = common::payload(&d, &info, true);
    let mut d = d;
    // Record 13: one table (share mode 2), precision 0, 64 values of 1
    // (a varint, then 63 signed varint deltas of 0).
    let mut tables = vec![2u8, 1];
    tables.extend([0u8; 63]);
    // Record 15: one channel group, one band, the pair (4, 5).
    let bias = [0u8, 4, 5];
    for (id, rec) in [
        (13u64, &tables[..]),
        (15, &bias[..]),
        (17, &[1][..]),
        (20, &[9][..]),
    ] {
        d = common::with_head_record(&d, &p, id, Some(rec));
        p = common::payload(&d, &info, true);
    }
    // Matrix 63: presence bit 2 is set (the YCoCg default writes matrix
    // 5); its byte follows the quantizer scale and the seed and entropy
    // fields.
    assert_ne!(p.present & 4, 0);
    let (_, n) = common::varint(&d[p.present_at..]);
    let mut at = p.present_at + n + 4;
    at += (p.present & 1) as usize + (p.present >> 1 & 1) as usize;
    assert_eq!(d[at], 5);
    let mut e = d.clone();
    e[at] = 63;
    let declared = common::feature_mask(&f)
        | common::feature::DEQUANT_PRESETS
        | common::feature::EXPLICIT_TABLES
        | common::feature::RECON_BIAS
        | common::feature::COLOR_TRANSFORM;
    let g = common::with_feature_mask(&with_data_payload(&f, e), Some(declared));
    assert!(decode(&g).is_ok(), "{:?}", decode(&g).err());
    remuxed(&g);
}
