//! Independent ANS models inside an existing indexed AC unit.
//!
//! After any transform/partition map: control = table_byte_len * 2 + compact.
//! A zero table length inherits only the image-wide model. Nonzero tables are
//! unseeded RunLevel tables, followed by the existing lane payload. No previous
//! unit supplies model state. The critical head flag selects this grammar.
use super::*;
use alloc::boxed::Box;

pub(super) const HEAD: u64 = crate::registry::HEAD_LOCAL_ANS;
pub(super) const MAX_TABLE_BYTES: usize = 64 * 1024;

pub(super) struct DecodedModel {
    pub tables: Tables,
    pub rans: Option<RansTables>,
    pub huf: Option<(huf::HufTables, huf::Dec, huf::WidRows)>,
}

impl DecodedModel {
    pub(super) fn read(
        bytes: &[u8],
        mode: LossyEntropy,
        bank: Option<&'static crate::seeds::Bank>,
    ) -> Result<Self, Error> {
        if bytes.is_empty() || bytes.len() > MAX_TABLE_BYTES {
            return Err(Error::TooLarge);
        }
        let mut bits = BitReader::new(bytes);
        let model = if mode == LossyEntropy::RunLevel {
            let tables = Tables::read(&mut bits, mode, bank)?;
            let rans = Some(RansTables::new(&tables));
            Self {
                tables,
                rans,
                huf: None,
            }
        } else {
            let ht = huf::HufTables::read(&mut bits, bank)?;
            let dec = ht.dec();
            let rows = huf::WidRows::new(&ht);
            Self {
                tables: Tables::placeholder(mode),
                rans: None,
                huf: Some((ht, dec, rows)),
            }
        };
        let used = bits.bits_read() as usize;
        if used.div_ceil(8) != bytes.len()
            || (!used.is_multiple_of(8) && bytes[used / 8] >> (used % 8) != 0)
        {
            return Err(Error::Corrupt("catalog model padding or trailing bytes"));
        }
        Ok(model)
    }
}

pub(super) struct Unit<'a> {
    pub shared: Option<&'a DecodedModel>,
    pub(super) inline: Option<Box<DecodedModel>>,
    pub body: &'a [u8],
    pub compact: bool,
}
impl Unit<'_> {
    pub fn model(&self) -> Option<&DecodedModel> {
        self.shared.or(self.inline.as_deref())
    }
}

impl<'a> Unit<'a> {
    pub(super) fn words(&self, active: usize) -> Result<(Words<'a>, [u32; NL]), Error> {
        if self.compact {
            let (states, body) = compact::states(self.body, active)?;
            Ok((Words::from_body(body)?, states))
        } else {
            Words::from_bytes(self.body, active)
        }
    }
}

/// Resolve a unit's model outside the scalar/SIMD entropy loop. Global files
/// borrow their existing tables; only an inline definition constructs tables.
#[inline]
pub(super) fn unit<'a>(p: &'a LossyParsed<'_>, bytes: &'a [u8]) -> Result<Unit<'a>, Error> {
    if let Some(models) = &p.model_refs {
        return models.ac(bytes);
    }
    #[cfg(all(feature = "lab", test))]
    if let Some(catalog) = &p.catalog_lab {
        return catalog.select(bytes);
    }
    let (tables, compact, body) = if p.local_ans_contexts {
        split(bytes)?
    } else {
        (None, p.compact_states, bytes)
    };
    let inline = tables
        .map(|b| {
            let tables = read(b)?;
            let rans = Some(RansTables::new(&tables));
            Ok::<_, Error>(Box::new(DecodedModel {
                tables,
                rans,
                huf: None,
            }))
        })
        .transpose()?;
    Ok(Unit {
        shared: None,
        inline,
        body,
        compact,
    })
}

pub(super) fn split(bytes: &[u8]) -> Result<(Option<&[u8]>, bool, &[u8]), Error> {
    let mut r = ByteReader::new(bytes);
    let control = r.varint()?;
    let length = usize::try_from(control >> 1).map_err(|_| Error::TooLarge)?;
    if length > MAX_TABLE_BYTES {
        return Err(Error::TooLarge);
    }
    let tables = r.bytes(length)?;
    let body = r.bytes(r.remaining())?;
    Ok((
        (!tables.is_empty()).then_some(tables),
        control & 1 != 0,
        body,
    ))
}

pub(super) fn read(bytes: &[u8]) -> Result<Tables, Error> {
    let mut bits = BitReader::new(bytes);
    let tables = Tables::read(&mut bits, LossyEntropy::RunLevel, None)?;
    let used = bits.bits_read() as usize;
    if used.div_ceil(8) != bytes.len()
        || (!used.is_multiple_of(8) && bytes[used / 8] >> (used % 8) != 0)
    {
        return Err(Error::Corrupt("local ANS table padding or trailing bytes"));
    }
    Ok(tables)
}

#[cfg(feature = "encode")]
pub(super) fn validate(cfg: &LossyConfig) -> Result<(), Error> {
    if cfg.model_references && cfg.local_ans_contexts {
        return Err(Error::InvalidInput("two model selection envelopes"));
    }
    if cfg.local_ans_contexts
        && (cfg.entropy != LossyEntropy::RunLevel || cfg.decode_speed.is_some())
    {
        return Err(Error::Unsupported(
            "local ANS contexts require explicit RunLevel coding without automatic tier selection",
        ));
    }
    Ok(())
}

#[cfg(feature = "encode")]
fn frame(tables: &[u8], body: &[u8], packed: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(3 + tables.len() + body.len());
    put_varint(&mut out, (tables.len() as u64) * 2 + u64::from(packed));
    out.extend_from_slice(tables);
    out.extend_from_slice(body);
    out
}

/// Price every byte of the four legal alternatives. Ties retain the shared
/// model, then raw states. Symbols are those already made for this unit.
#[cfg(feature = "encode")]
pub(super) fn encode(
    lanes: &[Vec<Sym>; NL],
    active: usize,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    global: &[u8],
) -> Result<Vec<u8>, Error> {
    let mut best = frame(&[], global, false);
    if cfg.compact_streams {
        let packed = frame(&[], &compact::pack(global, active)?, true);
        if packed.len() < best.len() {
            best = packed;
        }
    }
    let mut hist = vec![vec![0u64; coef::ALPHA]; LossyEntropy::RunLevel.nraw()];
    for symbol in lanes.iter().flatten() {
        hist[symbol.ctx as usize][symbol.tok as usize] += 1;
    }
    let tables = Tables::build_seeded(
        &hist,
        4,
        cfg.max_ac_tables,
        LossyEntropy::RunLevel,
        effort,
        None,
        table_k_mode(cfg),
    )
    .0;
    let mut bits = BitWriter::new();
    tables.write(&mut bits);
    let tables_bytes = bits.finish();
    if tables_bytes.len() > MAX_TABLE_BYTES {
        return Ok(best);
    }
    let encoder = EncodeTables::new(&tables.freq, &tables.ctx_map);
    let mut body = Vec::new();
    encode_lanes_tables(&encoder, lanes, active, &mut body);
    let local = frame(&tables_bytes, &body, false);
    if local.len() < best.len() {
        best = local;
    }
    if cfg.compact_streams {
        let packed = frame(&tables_bytes, &compact::pack(&body, active)?, true);
        if packed.len() < best.len() {
            best = packed;
        }
    }
    Ok(best)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::planes::{Rect, SliceReader};

    #[test]
    fn local_ans_public_option_survives_expert_tuning_and_preserves_pixels() {
        let source: Vec<u8> = (0..65 * 129 * 3).map(|x| (x * 41) as u8).collect();
        let pixels = zenpixels::PixelSlice::new(
            &source,
            65,
            129,
            65 * 3,
            zenpixels::PixelDescriptor::RGB8_SRGB,
        )
        .unwrap();
        let cfg = crate::ImiEncoderConfig::lossy(40.0)
            .with_effort(0.0)
            .with_local_ans_contexts(true)
            .with_lossy_tuning(Default::default())
            .tune_lossy(|t| t.stripe_block_rows = 8);
        assert!(cfg.local_ans_contexts());
        let encoded = cfg.encode(&pixels, &Metadata::default()).unwrap();
        assert_eq!(crate::header(&encoded).unwrap().version, 4);
        let native = cfg
            .encode_legacy_samples(
                crate::ImageRef::packed(
                    &source,
                    &ImageInfo::new(65, 129, crate::Channels::Rgb, crate::SampleType::U8),
                ),
                &Metadata::default(),
            )
            .unwrap();
        assert!(parse_lossy(&native).unwrap().local_ans_contexts);
        assert!(
            crate::header(&native)
                .unwrap()
                .profiles
                .iter()
                .any(|v| v.profile == crate::Profile::Lossy && v.minor == 6)
        );
        struct DenyLocal;
        impl crate::parse::limits::Admission for DenyLocal {
            fn local_ans(&self, _: u128) -> Result<(), Error> {
                Err(Error::TooLarge)
            }
        }
        let (i, c, b, f) = crate::v1::lossy_body(&native).unwrap().unwrap();
        assert!(matches!(
            parse_lossy_checked(i, 3, c, f, b, &DenyLocal),
            Err(Error::TooLarge)
        ));
        let legacy = cfg
            .with_local_ans_contexts(false)
            .encode(&pixels, &Metadata::default())
            .unwrap();
        assert_eq!(
            crate::decode(&encoded).unwrap(),
            crate::decode(&legacy).unwrap()
        );
    }

    #[test]
    fn native_local_ans_preserves_coefficients_pixels_and_indexed_regions() {
        let mut local_units = 0;
        let mut global_units = 0;
        for bits in [8, 10, 12, 16] {
            let info = ImageInfo::new(
                1025,
                131,
                crate::Channels::Rgba,
                if bits == 8 {
                    crate::SampleType::U8
                } else {
                    crate::SampleType::U16
                },
            );
            let info = info.with_significant_bits(&[bits; 4]).unwrap();
            let stride = info.row_bytes() + 13;
            let mut source = vec![0xa5; stride * 130 + info.row_bytes()];
            for y in 0..131 {
                for x in 0..info.width as usize * 4 {
                    let raw = (x * 997 + y * 7907 + x * y * 11) & ((1 << bits) - 1);
                    let v = if y < 64 {
                        ((1 << bits) - 1) / 4 + raw / 16
                    } else {
                        raw
                    } as u16;
                    let at = y * stride + x * info.sample.bytes();
                    if bits == 8 {
                        source[at] = v as u8;
                    } else {
                        source[at..at + 2].copy_from_slice(&v.to_ne_bytes());
                    }
                }
            }
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                for compact in [false, true] {
                    for quality in [5.0, 40.0, 95.0] {
                        let mut cfg = LossyConfig::new(quality).with_effort(0.0);
                        cfg.color = color;
                        cfg.compact_streams = compact;
                        cfg.stripe_block_rows = 8;
                        cfg.column_groups = Some(512);
                        cfg.checksum = false;
                        let before =
                            encode_lossy(&source, stride, &info, &Metadata::default(), &cfg)
                                .unwrap();
                        let reference = crate::decode(&before).unwrap().1;
                        cfg.local_ans_contexts = true;
                        let after =
                            encode_lossy(&source, stride, &info, &Metadata::default(), &cfg)
                                .unwrap();
                        let old = parse_lossy(&before).unwrap();
                        let p = parse_lossy(&after).unwrap();
                        assert_eq!(old.ngroups(), p.ngroups());
                        assert_eq!(
                            dc_image_lab(&before).unwrap().1,
                            dc_image_lab(&after).unwrap().1
                        );
                        for (u, bytes) in p.stripes.iter().enumerate() {
                            let g = p.group(u % p.ngroups()).1;
                            let stripe = u / p.ngroups();
                            let a = decode_stripe_raw_bytes(
                                &old,
                                &g,
                                stripe,
                                old.stripes[u],
                                Group::ALL,
                                false,
                                Vec::new(),
                                Vec::new(),
                            )
                            .unwrap()
                            .0;
                            let b = decode_stripe_raw_bytes(
                                &p,
                                &g,
                                stripe,
                                bytes,
                                Group::ALL,
                                false,
                                Vec::new(),
                                Vec::new(),
                            )
                            .unwrap()
                            .0;
                            assert_eq!(a.len(), b.len());
                            assert_eq!(
                                a.iter().zip(&b).position(|(a, b)| a != b),
                                None,
                                "scalar coefficient mismatch at unit {u}"
                            );
                        }
                        assert!(p.local_ans_contexts);
                        for (u, record) in p.stripes.iter().enumerate() {
                            let g = p.group(u % p.ngroups()).1;
                            let (_, record) = p.record(&g, u / p.ngroups(), record).unwrap();
                            if split(record).unwrap().0.is_some() {
                                local_units += 1;
                            } else {
                                global_units += 1;
                            }
                        }
                        if bits == 8 && quality == 40.0 && color == LossyColor::Xyb && compact {
                            let selected = p.ngroups() + 1;
                            let mut withheld = after.clone();
                            for (u, bytes) in p.stripes.iter().enumerate() {
                                if u != selected {
                                    let at = bytes.as_ptr() as usize - after.as_ptr() as usize;
                                    withheld[at..at + bytes.len()].fill(0);
                                }
                            }
                            let rect = Rect::new(513, 65, 1, 1);
                            let mut pixel = vec![0; info.bytes_per_pixel()];
                            crate::region::RegionReader::open(SliceReader::new(&withheld))
                                .unwrap()
                                .decode_region(rect, &mut pixel, info.bytes_per_pixel())
                                .unwrap();
                            let at = 65 * info.row_bytes() + 513 * info.bytes_per_pixel();
                            assert_eq!(pixel, reference[at..at + pixel.len()]);
                        }
                        for threads in [1, 2, 4, 8] {
                            let mut out = vec![0x6d; source.len()];
                            crate::decode_into_threads(&after, &mut out, stride, threads).unwrap();
                            for y in 0..131 {
                                assert_eq!(
                                    &out[y * stride..][..info.row_bytes()],
                                    &reference[y * info.row_bytes()..][..info.row_bytes()]
                                );
                                if y < 130 {
                                    assert!(
                                        out[y * stride + info.row_bytes()..(y + 1) * stride]
                                            .iter()
                                            .all(|&v| v == 0x6d)
                                    );
                                }
                            }
                            let rect = Rect::new(509, 61, 13, 13);
                            let rb = 13 * info.bytes_per_pixel();
                            let mut crop = vec![0; rb * 13];
                            crate::region::RegionReader::open(SliceReader::new(&after))
                                .unwrap()
                                .decode_region_threads(rect, &mut crop, rb, threads)
                                .unwrap();
                            for y in 0..13 {
                                let at = (y + 61) * info.row_bytes() + 509 * info.bytes_per_pixel();
                                assert_eq!(&crop[y * rb..][..rb], &reference[at..at + rb]);
                            }
                        }
                        let (_, color, body, features) =
                            crate::v1::lossy_body(&after).unwrap().unwrap();
                        assert!(features.contains(crate::Feature::LocalAnsContexts));
                        assert!(
                            parse_lossy_as(
                                info.clone(),
                                3,
                                color,
                                features.without(crate::Feature::LocalAnsContexts),
                                body
                            )
                            .is_err()
                        );
                    }
                }
            }
        }
        assert!(local_units > 0, "the local-model decoder must be exercised");
        assert!(
            global_units > 0,
            "the rate-priced global fallback must be exercised"
        );
    }

    #[test]
    fn local_ans_envelopes_reject_oversized_truncated_and_noncanonical_models() {
        for bytes in [&[][..], &[0x80][..], &[2][..], &[4, 0][..]] {
            assert!(split(bytes).is_err());
        }
        let mut huge = Vec::new();
        put_varint(&mut huge, ((MAX_TABLE_BYTES as u64 + 1) * 2) | 1);
        assert!(matches!(split(&huge), Err(Error::TooLarge)));
        assert!(read(&[0, 0]).is_err());
        let info = ImageInfo::new(33, 17, crate::Channels::Rgb, crate::SampleType::U8);
        let source: Vec<_> = (0..info.row_bytes() * 17).map(|x| (x * 31) as u8).collect();
        let cfg = LossyConfig::new(40.0).with_effort(0.0);
        let encoded =
            encode_lossy(&source, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let p = parse_lossy(&encoded).unwrap();
        let mut explicit = p.tables.clone();
        explicit.seeds = None;
        let mut bits = BitWriter::new();
        explicit.write(&mut bits);
        let mut tables = bits.finish();
        let parsed = read(&tables);
        assert!(
            parsed.is_ok(),
            "explicit local model did not round-trip: {:?}",
            parsed.err()
        );
        tables.push(0);
        assert!(
            read(&tables).is_err(),
            "declared table bytes cannot hide trailing bytes"
        );
        let mut bad = cfg.clone();
        bad.local_ans_contexts = true;
        bad.entropy = LossyEntropy::SplitHuf;
        assert!(
            encode_lossy(&source, info.row_bytes(), &info, &Metadata::default(), &bad).is_err()
        );
    }
}
