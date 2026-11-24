//! Pixel oracle: native V3 entropy/parameters through the V4 work path.

use super::super::*;
use super::*;
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::{ImageRef, ImiEncoderConfig, Metadata};
use alloc::{sync::Arc, vec, vec::Vec};

fn number(out: &mut Vec<u8>, value: u64) {
    crate::put_varint(out, value);
}
fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    number(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}
fn definition(role: u64, head: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in [role, 0, 0, 0] {
        number(&mut out, value);
    }
    blob(&mut out, head);
    out
}
fn numeric(head: &[u8]) -> Vec<u8> {
    let mut r = ByteReader::new(head);
    let present = r.varint().unwrap();
    r.u8().unwrap();
    r.varint().unwrap();
    r.varint().unwrap();
    let mut out = Vec::new();
    number(&mut out, present);
    out.extend_from_slice(&head[r.pos..]);
    out
}
fn unit(role: u64, id: u64, compact: bool, coder: LossyEntropy, entropy: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    number(&mut out, id);
    if role == 1 {
        number(&mut out, 0);
    }
    number(&mut out, 0);
    if role == 1 && coder == LossyEntropy::RunLevel {
        number(&mut out, u64::from(compact));
    }
    out.extend_from_slice(entropy);
    out
}

fn setup<'a>(
    file: &'a [u8],
    session: &DecodeSession,
) -> (
    LossyParsed<'a>,
    Arc<LossySetup>,
    Vec<(u64, Vec<u8>)>,
    Vec<u8>,
) {
    let f = crate::v1::parse_file(
        file,
        &crate::parse::limits::Unrestricted,
        crate::checksum::Verify::Now(1),
    )
    .unwrap();
    let p = parse_lossy(file).unwrap();
    assert!(p.model_refs.is_none() && !p.local_ans_contexts);
    let mut r = ByteReader::new(f.data);
    r.u8().unwrap();
    r.varint().unwrap();
    let params = envelope::read(
        &mut r,
        p.color,
        InputKind::of(&p.info).unwrap(),
        f.declared,
        true,
    )
    .unwrap();
    let ac_length = r.var_usize().unwrap();
    let ac = r.bytes(ac_length).unwrap();
    let records = if params.records {
        let n = r.var_usize().unwrap();
        r.bytes(n).unwrap()
    } else {
        &[]
    };
    let mut remaining = Vec::new();
    let mut aq_declared = false;
    crate::parse::records::head_records(records, |id, body| {
        aq_declared |= id == H_AQ;
        if [H_EXPLICIT, H_BIAS, H_TRANSFORM].contains(&id) {
            crate::parse::records::put_record(&mut remaining, id, body);
        }
        Ok(())
    })
    .unwrap();
    let dc_length = r.var_usize().unwrap();
    let dc = r.bytes(dc_length).unwrap();
    let alpha = if p.alpha.is_some() {
        let n = r.var_usize().unwrap();
        Some(r.bytes(n).unwrap())
    } else {
        None
    };
    let exact_head = if let Some(exact) = &p.exact {
        let n = r.var_usize().unwrap();
        let exact_body = r.bytes(n).unwrap();
        let mut er = ByteReader::new(exact_body);
        assert_eq!(er.var_usize().unwrap(), exact.row_blocks());
        for &(_, count) in &exact.ranges {
            assert_eq!(er.var_usize().unwrap(), count);
        }
        let start = er.pos;
        crate::v1::parse_head_nested(
            exact.img.info.clone(),
            &mut er,
            &crate::parse::limits::Unrestricted,
            None,
            crate::Features::EMPTY,
        )
        .unwrap();
        Some(numeric(&exact_body[start..er.pos]))
    } else {
        None
    };
    let mut q = Vec::new();
    number(&mut q, 31);
    q.extend_from_slice(&params.qp.scale.to_le_bytes());
    number(&mut q, u64::from(params.qp.matrix));
    for value in params.qp.dc {
        number(&mut q, u64::from(value));
    }
    for value in [params.qp.floor]
        .into_iter()
        .chain(params.scale)
        .chain(params.qp.mul)
    {
        q.extend_from_slice(&value.to_le_bytes());
    }
    let mut out = Vec::new();
    for value in [
        p.g.sb as u64,
        p.gcols as u64,
        p.ngroups() as u64,
        p.stripe_count() as u64,
        0,
        0,
        p.tables.mode as u64,
    ] {
        number(&mut out, value);
    }
    blob(&mut out, &q);
    number(
        &mut out,
        if p.exact.is_some() {
            2
        } else if p.skip {
            if aq_declared { 4 } else { 3 }
        } else {
            u64::from(p.aq)
        },
    );
    number(
        &mut out,
        u64::from(p.dc.as_ref().unwrap().info.sample == SampleType::I32),
    );
    blob(&mut out, &[0, 0]);
    let mut coverage = Vec::new();
    if p.dc_coverage.is_some() {
        number(&mut coverage, 1);
        number(&mut coverage, p.ngroups() as u64);
        for gi in 0..p.ngroups() {
            let mut lengths = Vec::new();
            let mut stripe = 0;
            while stripe < p.stripe_count() {
                let span = p.dc_span(stripe, gi);
                lengths.push(span.end - span.start);
                stripe = span.end;
            }
            number(&mut coverage, lengths.len() as u64);
            for length in lengths {
                number(&mut coverage, length as u64);
            }
        }
    } else {
        number(&mut coverage, 0);
        number(&mut coverage, p.dc_k() as u64);
    }
    blob(&mut out, &coverage);
    number(&mut out, 0); // explicit unseeded definitions
    number(
        &mut out,
        1 + u64::from(alpha.is_some()) + u64::from(p.exact.is_some()),
    );
    number(&mut out, 0);
    number(&mut out, p.dc.as_ref().unwrap().lanes as u64);
    if let Some(a) = &p.alpha {
        number(&mut out, 2);
        number(&mut out, a.lanes as u64);
    }
    if let Some(exact) = &p.exact {
        for value in [
            3,
            exact.img.lanes as u64,
            exact.row_blocks() as u64,
            exact.img.hs as u64,
        ] {
            number(&mut out, value);
        }
    }
    blob(&mut out, &remaining);
    let declared = crate::Features::from_bits(f.declared.bits() & ((1 << 26) - 1)).unwrap();
    let checked =
        Arc::new(LossySetup::read(p.info.clone(), p.color, declared, &out, session).unwrap());
    let mut definitions = vec![(0, definition(0, &numeric(dc))), (1, definition(1, ac))];
    if let Some(a) = alpha {
        definitions.push((2, definition(2, &numeric(a))));
    }
    if let Some(head) = exact_head {
        definitions.push((3, definition(3, &head)));
    }
    (p, checked, definitions, out)
}

fn pixels(info: &ImageInfo) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..info.height {
        for x in 0..info.width {
            for c in 0..info.channels.count() {
                let n = (x * 19 + y * 37 + c as u32 * 71 + x * y) % 251;
                match info.sample {
                    SampleType::U8 => out.push(n as u8),
                    SampleType::U16 => out.extend_from_slice(&((n * 257) as u16).to_ne_bytes()),
                    SampleType::U32 => out.extend_from_slice(&(n * 16_843_009).to_ne_bytes()),
                    SampleType::F16 => out
                        .extend_from_slice(&color::f32_to_f16_rne(n as f32 / 250.0).to_ne_bytes()),
                    SampleType::F32 => out.extend_from_slice(&(n as f32 / 250.0).to_ne_bytes()),
                    _ => unreachable!(),
                }
            }
        }
    }
    out
}

fn altered_definition(root: &[u8], id: u64, target: Option<u64>, bank: u64) -> Vec<u8> {
    let mut r = ByteReader::new(root);
    let role = r.varint().unwrap();
    assert_eq!(r.varint().unwrap(), 0);
    assert_eq!(r.varint().unwrap(), 0);
    r.varint().unwrap();
    let length = r.var_usize().unwrap();
    let body = r.bytes(length).unwrap();
    let mut out = Vec::new();
    for value in [role, id, u64::from(target.is_some())] {
        number(&mut out, value);
    }
    if let Some(target) = target {
        number(&mut out, target);
    }
    number(&mut out, bank);
    blob(&mut out, body);
    out
}

fn selected_unit(id: u64, model: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for value in [id, 0, model, 0] {
        number(&mut out, value);
    }
    out
}

#[test]
fn typed_models_enforce_roots_gaps_expiry_and_immediate_main_targets() {
    let info = ImageInfo::new(41, 29, Channels::Rgb, SampleType::U8);
    let source = pixels(&info);
    let config = ImiEncoderConfig::lossy(73.0)
        .with_column_groups(Some(16))
        .tune_lossy(|t| {
            t.color = LossyColor::YCoCg;
            t.exact_blocks = Some(0.0);
            t.aq_gamma = 0.0;
            t.stripe_block_rows = 2;
            t.lab.seed_bank = 0;
        });
    let encoded = config
        .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
        .unwrap();
    let session = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(8 << 20));
    let (_, setup, roots, _) = setup(&encoded, &session);
    let mut models = LossyModels::new(Arc::clone(&setup), &session);
    assert!(
        models
            .define(&altered_definition(&roots[1].1, 17, None, 0))
            .is_err()
    );
    assert!(
        models
            .define(&altered_definition(&roots[1].1, 0, Some(0), 0))
            .is_err()
    );
    models.define(&roots[0].1).unwrap();
    assert!(models.unit(0, &[0, 0]).is_err()); // AC root still absent.
    models.define(&roots[1].1).unwrap();
    assert!(models.define(&roots[1].1).is_err());
    assert!(
        models
            .define(&altered_definition(&roots[1].1, 1, None, 1))
            .is_err()
    );
    models
        .define(&altered_definition(&roots[1].1, 17, Some(1), 0))
        .unwrap();
    assert!(models.finish().is_err());
    assert!(
        models
            .define(&altered_definition(&roots[1].1, 18, Some(1), 0))
            .is_err()
    );
    assert!(models.unit(0, &[0, 0]).is_err());
    assert!(models.unit(1, &selected_unit(0, 17)).is_err());
    assert!(models.unit(1, &selected_unit(1, 0)).is_err());
    let body = selected_unit(1, 17);
    let captured = models.unit(1, &body).unwrap();
    assert_eq!((captured.role(), captured.id()), (1, 1));
    assert!(models.unit(1, &selected_unit(2, 17)).is_err()); // expired, not inherited.
    models
        .define(&altered_definition(&roots[1].1, u64::MAX, None, 0))
        .unwrap();
    let max_body = selected_unit(2, u64::MAX);
    assert_eq!(models.unit(1, &max_body).unwrap().id(), 2);
    assert!(
        models
            .define(&altered_definition(&roots[1].1, u64::MAX - 1, None, 0))
            .is_err()
    );
    assert_eq!(models.unit(1, &selected_unit(3, 0)).unwrap().id(), 3);
    models.finish().unwrap();
    drop(models);
    assert_eq!(captured.setup().info(), &info); // ownership survives registry expiry/drop.
}

#[test]
fn transient_capacity_follows_the_final_job_owner_and_can_be_reused() {
    let info = ImageInfo::new(41, 29, Channels::Rgb, SampleType::U8);
    let source = pixels(&info);
    let config = ImiEncoderConfig::lossy(73.0)
        .with_column_groups(Some(16))
        .tune_lossy(|t| {
            t.color = LossyColor::YCoCg;
            t.exact_blocks = Some(0.0);
            t.aq_gamma = 0.0;
            t.stripe_block_rows = 2;
            t.lab.seed_bank = 0;
        });
    let encoded = config
        .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
        .unwrap();
    let bound = u64::try_from(3 * decoder_table_bytes() + (1 << 20)).unwrap();
    let session = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(bound));
    let (_, setup, roots, _) = setup(&encoded, &session);
    let mut models = LossyModels::new(setup, &session);
    for (_, root) in &roots {
        models.define(root).unwrap();
    }
    for id in 1..=128 {
        models
            .define(&altered_definition(&roots[1].1, id, Some(0), 0))
            .unwrap();
        let captured = models.take(1, 0, id).unwrap();
        // A worker can own and release the immutable model without borrowing
        // the single-threaded loader/session or carrying another unit's state.
        #[cfg(feature = "std")]
        std::thread::spawn(move || drop(captured)).join().unwrap();
        #[cfg(not(feature = "std"))]
        drop(captured);
    }
    let mut held = Vec::new();
    let mut denied = None;
    for id in 129..=256 {
        match models.define(&altered_definition(&roots[1].1, id, Some(0), 0)) {
            Ok(()) => held.push(models.take(1, 0, id).unwrap()),
            Err(Error::TooLarge) => {
                denied = Some(id);
                break;
            }
            Err(error) => panic!("unexpected refusal: {error:?}"),
        }
    }
    let retry = denied.expect("outstanding job tables must exhaust the working allowance");
    drop(held);
    models
        .define(&altered_definition(&roots[1].1, retry, Some(0), 0))
        .unwrap();
    drop(models.take(1, 0, retry).unwrap());
    models.finish().unwrap();
    assert!(session.usage().peak_planned_working_bytes <= bound);
}

#[test]
fn v4_srgb_power_transfer_preserves_legacy_and_agrees_across_decode_routes() {
    for sample in [SampleType::U8, SampleType::U16] {
        let info = ImageInfo::new(97, 81, Channels::Rgba, sample);
        let mut source = pixels(&info);
        if sample == SampleType::U8 {
            for (i, px) in source.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                px[0] = (i % 256) as u8;
                px[1] = ((i / 3) % 256) as u8;
            }
        } else {
            for (i, px) in source.as_chunks_mut::<8>().0.iter_mut().enumerate() {
                let r = ((i * 257) % 65536) as u16;
                let g = (((i / 3) * 257) % 65536) as u16;
                px[0..2].copy_from_slice(&r.to_ne_bytes());
                px[2..4].copy_from_slice(&g.to_ne_bytes());
            }
        }
        let image = ImageRef::packed(&source, &info);
        let metadata = Metadata::default();
        for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let config = ImiEncoderConfig::lossy(91.0)
                .with_column_groups(Some(32))
                .tune_lossy(|t| {
                    t.color = LossyColor::Xyb;
                    t.entropy = coder;
                    t.stripe_block_rows = 2;
                });
            let legacy = config.encode_legacy_samples(image, &metadata).unwrap();
            let migrated = complete_file(&legacy);
            let encoded = config.encode_samples(image, &metadata).unwrap();
            let session = DecodeSession::new(DecodePolicy::default());
            for (file, power) in [(&migrated, false), (&encoded, true)] {
                let codec = LossyFile::open(file, tile_limits(), &session).unwrap();
                assert_eq!(codec.inspection_setup().parameters.xyb_power, power);
                if power {
                    assert!(codec.inspection_setup().parameters.xfer16.is_none());
                }
            }
            let old = crate::decode(&legacy).unwrap().1;
            assert_eq!(crate::decode(&migrated).unwrap().1, old);
            let expected = crate::decode(&encoded).unwrap().1;
            if sample == SampleType::U8 {
                for (a, b) in old
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(expected.as_chunks::<4>().0)
                {
                    assert_eq!(a[3], b[3]);
                    for c in 0..3 {
                        assert!(a[c].abs_diff(b[c]) <= 1, "{a:?} versus {b:?}");
                    }
                }
            } else {
                for (a, b) in old
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .zip(expected.as_chunks::<8>().0)
                {
                    assert_eq!(&a[6..8], &b[6..8]);
                    for c in 0..3 {
                        let va = u16::from_ne_bytes([a[c * 2], a[c * 2 + 1]]);
                        let vb = u16::from_ne_bytes([b[c * 2], b[c * 2 + 1]]);
                        assert!(va.abs_diff(vb) <= 1, "u16 ch {c}: {va} versus {vb}");
                    }
                }
            }
            for workers in [1, 2, 4, 8] {
                let mut full = vec![0; expected.len()];
                crate::decode_into_threads(&encoded, &mut full, info.row_bytes(), workers).unwrap();
                assert_eq!(full, expected);
            }
            #[cfg(feature = "std")]
            crate::v4::feed_tests::check_stream(&encoded, &expected, &info);
            let rect = crate::planes::Rect::new(27, 14, 45, 39);
            let bpp = info.bytes_per_pixel();
            let row = rect.width() as usize * bpp;
            let stride = row + 11;
            let mut cropped = vec![0xa7; stride * rect.height() as usize];
            crate::region::decode_region(&encoded, rect, &mut cropped, stride).unwrap();
            for (y, out) in cropped.chunks_exact(stride).enumerate() {
                let offset = (rect.y() as usize + y) * info.row_bytes() + rect.x() as usize * bpp;
                assert_eq!(&out[..row], &expected[offset..offset + row]);
                assert!(out[row..].iter().all(|&byte| byte == 0xa7));
            }
        }
    }
}

#[test]
fn v4_lossy_groups_match_native_pixels_across_samples_alpha_coders_and_restarts() {
    for (sample, channels, color) in [
        (SampleType::U8, Channels::Rgb, LossyColor::YCoCg),
        (SampleType::U8, Channels::GrayAlpha, LossyColor::Xyb),
        (SampleType::U16, Channels::Rgba, LossyColor::YCoCg),
        (SampleType::U16, Channels::Rgba, LossyColor::Xyb),
        (SampleType::F16, Channels::Rgba, LossyColor::Xyb),
        (SampleType::F32, Channels::Rgb, LossyColor::Xyb),
        (SampleType::U32, Channels::Rgba, LossyColor::YCoCg),
    ] {
        let info = ImageInfo::new(41, 29, channels, sample);
        let source = pixels(&info);
        for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for compact in if coder == LossyEntropy::RunLevel {
                &[false, true][..]
            } else {
                &[false][..]
            } {
                for aq in [false, true] {
                    let config = ImiEncoderConfig::lossy(73.0)
                        .with_column_groups(Some(16))
                        .tune_lossy(|t| {
                            t.color = color;
                            t.entropy = coder;
                            t.compact_streams = *compact;
                            t.stripe_block_rows = 2;
                            t.aq_gamma = if aq { 1.0 } else { 0.0 };
                            t.exact_blocks = Some(0.0);
                            t.small_transforms = false;
                            t.adaptive_partitions = false;
                            t.freq_zero_bias = false;
                            t.lab.seed_bank = 0;
                        });
                    let encoded = config
                        .encode_legacy_samples(
                            ImageRef::packed(&source, &info),
                            &Metadata::default(),
                        )
                        .unwrap();
                    let expected = crate::decode(&encoded).unwrap().1;
                    let session = DecodeSession::new(DecodePolicy::default());
                    let (p, setup, definitions, _) = setup(&encoded, &session);
                    let v4 = complete_file(&encoded);
                    #[cfg(feature = "std")]
                    crate::v4::feed_tests::check_stream(&v4, &expected, &info);
                    let file_session = DecodeSession::new(DecodePolicy::default());
                    let file = LossyFile::open(&v4, tile_limits(), &file_session).unwrap();
                    let row = info.width as usize * info.bytes_per_pixel();
                    let stride = row + 17;
                    let mut output = vec![0x9B; stride * info.height as usize];
                    file.decode_into(&mut output, stride).unwrap();
                    for y in 0..info.height as usize {
                        assert_eq!(&output[y * stride..][..row], &expected[y * row..][..row]);
                        assert!(
                            output[y * stride + row..(y + 1) * stride]
                                .iter()
                                .all(|&b| b == 0x9B)
                        );
                    }
                    // Retain every job past full traversal, tile close and registry drop.
                    let file_session = DecodeSession::new(DecodePolicy::default());
                    let file = LossyFile::open(&v4, tile_limits(), &file_session).unwrap();
                    let mut jobs = Vec::new();
                    file.visit_groups(|work| {
                        jobs.push(work);
                        Ok(())
                    })
                    .unwrap();
                    for work in jobs {
                        let rect = work.rect().unwrap();
                        let row = rect.width() as usize * info.bytes_per_pixel();
                        let mut result = vec![0; row * rect.height() as usize];
                        work.decode_into(
                            &mut result,
                            row,
                            &DecodeSession::new(DecodePolicy::default()),
                        )
                        .unwrap();
                        for y in 0..rect.height() as usize {
                            let at = ((rect.y() as usize + y) * info.width as usize
                                + rect.x() as usize)
                                * info.bytes_per_pixel();
                            assert_eq!(&result[y * row..][..row], &expected[at..][..row]);
                        }
                    }
                    let mut models = LossyModels::new(Arc::clone(&setup), &session);
                    for (_, d) in &definitions {
                        models.define(d).unwrap();
                    }
                    let mut dc_cache: Vec<(Vec<u8>, usize, usize)> = Vec::new();
                    for gi in 0..p.ngroups() {
                        let (id, rect) = setup.dc_for_group(gi as u64).unwrap();
                        let bytes = unit(0, id, false, coder, p.dc.as_ref().unwrap().stripes[gi]);
                        let work = models.unit(0, &bytes).unwrap();
                        let stride = rect.width() as usize
                            * p.dc.as_ref().unwrap().info.bytes_per_pixel()
                            + 9;
                        let mut dc = vec![0x65; stride * rect.height() as usize];
                        work.decode_numeric_into(
                            rect,
                            &mut dc,
                            stride,
                            &DecodeSession::new(DecodePolicy::default()),
                        )
                        .unwrap();
                        dc_cache.push((dc, stride, rect.y() as usize));
                    }
                    for u in 0..p.stripes.len() {
                        let gi = u % p.ngroups();
                        let s = u / p.ngroups();
                        let (_, g) = p.group(gi);
                        let rect = setup.group_rect(u as u64).unwrap();
                        let (by0, by1) = g.rows(s);
                        let (dc, dc_stride, dc_y) = &dc_cache[gi];
                        let mut dc_info = setup.numeric_geometry(0).unwrap().info.clone();
                        dc_info.width = g.bw as u32;
                        dc_info.height = (by1 - by0) as u32;
                        let alpha_stride = rect.width() as usize * sample.bytes() + 5;
                        let mut alpha = vec![0x33; alpha_stride * rect.height() as usize];
                        let mut alpha_info = info.clone();
                        if let Some(a) = &p.alpha {
                            let raw = unit(2, u as u64, false, coder, a.stripes[u]);
                            models
                                .unit(2, &raw)
                                .unwrap()
                                .decode_numeric_into(
                                    rect,
                                    &mut alpha,
                                    alpha_stride,
                                    &DecodeSession::new(DecodePolicy::default()),
                                )
                                .unwrap();
                            alpha_info = setup.numeric_geometry(2).unwrap().info.clone();
                            alpha_info.width = rect.width();
                            alpha_info.height = rect.height();
                        }
                        let (_, entropy) = p.record(&g, s, p.stripes[u]).unwrap();
                        let bytes = unit(1, u as u64, p.compact_states, coder, entropy);
                        let work = models.unit(1, &bytes).unwrap();
                        let row = rect.width() as usize * info.bytes_per_pixel();
                        let stride = row + 13;
                        let mut actual = vec![0xA7; stride * rect.height() as usize];
                        let dependencies = LossyGroupInputs {
                            dc: ImageRef::new(
                                &dc[(by0 - dc_y) * dc_stride..],
                                *dc_stride,
                                &dc_info,
                            ),
                            alpha: p
                                .alpha
                                .as_ref()
                                .map(|_| ImageRef::new(&alpha, alpha_stride, &alpha_info)),
                            exact: None,
                            reference: None,
                        };
                        work.decode_into(
                            dependencies,
                            &mut actual,
                            stride,
                            &DecodeSession::new(DecodePolicy::default()),
                        )
                        .unwrap();
                        #[cfg(feature = "std")]
                        for workers in [1, 2, 4, 8] {
                            std::thread::scope(|scope| {
                                let mut joins = Vec::new();
                                for _ in 0..workers {
                                    let work = &work;
                                    joins.push(scope.spawn(move || {
                                        let mut pixels =
                                            vec![0xA7; stride * rect.height() as usize];
                                        let worker = DecodeSession::new(
                                            DecodePolicy::default().with_max_working_bytes(8 << 20),
                                        );
                                        work.decode_into(
                                            dependencies,
                                            &mut pixels,
                                            stride,
                                            &worker,
                                        )
                                        .unwrap();
                                        pixels
                                    }));
                                }
                                for join in joins {
                                    assert_eq!(join.join().unwrap(), actual);
                                }
                            });
                        }

                        for y in 0..rect.height() as usize {
                            let start = ((rect.y() as usize + y) * info.width as usize
                                + rect.x() as usize)
                                * info.bytes_per_pixel();
                            assert_eq!(
                                &actual[y * stride..][..row],
                                &expected[start..][..row],
                                "{sample:?} {channels:?} {coder:?} aq{aq} compact{compact} group{u} row{y}"
                            );
                            assert!(
                                actual[y * stride + row..(y + 1) * stride]
                                    .iter()
                                    .all(|&v| v == 0xA7)
                            );
                        }
                    }
                    models.finish().unwrap();
                }
            }
        }
    }
}

fn assert_concise_settings(native: &[u8], file: &[u8]) {
    let source = parse_lossy(native).unwrap();
    let session = DecodeSession::new(DecodePolicy::default());
    let parsed = crate::v4::file::parse_file(file, &session).unwrap();
    let (mut decoder, mut at) = LossyGroupDecoder::new(
        source.info.clone(),
        source.color,
        parsed.declared,
        parsed.data,
        tile_limits(),
        &session,
    )
    .unwrap();
    if source.coding_defaults.is_none() && source.map == MapKind::None && source.adaptive_mask == 0
    {
        let plans = decoder.setup().settings().image().plane_plans();
        for parent in plans.as_chunks::<3>().0 {
            for (c, &plan) in parent.iter().enumerate() {
                let expected = source.layouts[c].layout(false);
                assert_eq!(
                    plan,
                    if expected == subblock::Layout::FULL8 {
                        0
                    } else {
                        2 + expected.byte()
                    }
                );
            }
        }
    }
    while at < parsed.data.len() {
        let event = decoder.read(&parsed.data[at..]).unwrap();
        let length = event.entry().entry.record_len as usize;
        if let LossyEvent::Unit { work, .. } = event
            && work.role() == 1
        {
            let id = work.id() as usize;
            let mut raw = ByteReader::new(&parsed.data[at..at + length]);
            assert_eq!(raw.varint().unwrap(), 55);
            raw.varint().unwrap();
            assert_eq!(raw.varint().unwrap(), work.id());
            let n = raw.var_usize().unwrap();
            let settings = raw.bytes(n).unwrap();
            if source.coding_defaults.is_none()
                && source.map == MapKind::None
                && source.adaptive_mask == 0
            {
                assert!(
                    settings.is_empty(),
                    "uniform image plans must not repeat in every group"
                );
            }
            if let Some(defaults) = &source.coding_defaults {
                let (s, gi) = (id / source.ngroups(), id % source.ngroups());
                let (_, group) = source.group(gi);
                let (a, b) = group.rows(s);
                let mut expected = Vec::new();
                defaults
                    .expand(source.stripes[id], b - a, group.bw, |_, state| {
                        expected.push(state);
                        Ok(())
                    })
                    .unwrap();
                let mut count = 0;
                decoder
                    .setup()
                    .settings()
                    .visit_unit(
                        settings,
                        (b - a) as u32,
                        group.bw as u32,
                        parsed.declared,
                        &session,
                        |_, _, actual| {
                            let state = expected[count];
                            assert_eq!(actual.partition(), state.partition);
                            assert_eq!(actual.ac_codes(), state.scale);
                            for (i, plan) in actual.plane_plans().iter().enumerate() {
                                assert_eq!(*plan, (state.small >> i & 1) as u8);
                            }
                            count += 1;
                            Ok(())
                        },
                    )
                    .unwrap();
                assert_eq!(count, expected.len());
                let (old, _) = coding_defaults::Defaults::split(source.stripes[id]).unwrap();
                if old.is_empty() {
                    assert!(settings.is_empty());
                } else {
                    let mut old = ByteReader::new(old);
                    if old.varint().unwrap() == 2 {
                        let mut new = ByteReader::new(settings);
                        assert_eq!(new.varint().unwrap(), 2);
                        assert_eq!(new.varint().unwrap(), old.varint().unwrap());
                    }
                }
            }
        }
        at += length;
    }
    decoder.finish_structure().unwrap();
}

fn complete_file(native: &[u8]) -> Vec<u8> {
    let file = crate::v4::upgrade_image(native, &crate::UpgradeLimits::default()).unwrap();
    assert_concise_settings(native, &file);
    if !parse_lossy(native).unwrap().skip {
        let (info, expected) = crate::decode(native).unwrap();
        #[cfg(feature = "std")]
        crate::v4::region_tests::check(&file, &expected, &info);
        #[cfg(feature = "std")]
        crate::frames::v4::check_single(&file, &expected, &info);
        let mut row = vec![0; info.row_bytes()];
        let mut delivered = 0;
        DecodeSession::new(DecodePolicy::default())
            .decode_rows(&file, &mut row, |got, y, pixels| {
                assert_eq!(got, &info);
                assert_eq!(y, delivered);
                assert_eq!(
                    pixels,
                    &expected[y as usize * info.row_bytes()..][..info.row_bytes()]
                );
                delivered += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(delivered, info.height);
    }
    file
}

fn tile_limits() -> crate::v4::TileLimits {
    crate::v4::TileLimits {
        max_object_bytes: 8 << 20,
        max_tile_bytes: 64 << 20,
    }
}

#[test]
fn complete_lossy_files_reject_late_damage_after_provisional_groups() {
    let info = ImageInfo::new(41, 29, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    let config = ImiEncoderConfig::lossy(73.0)
        .with_column_groups(Some(16))
        .tune_lossy(|t| {
            t.color = LossyColor::YCoCg;
            t.exact_blocks = Some(0.0);
            t.stripe_block_rows = 2;
            t.lab.seed_bank = 0;
        });
    let native = config
        .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
        .unwrap();
    let session = DecodeSession::new(DecodePolicy::default());
    let p = parse_lossy(&native).unwrap();
    let file = complete_file(&native);
    let parsed = crate::v4::file::parse_file(&file, &session).unwrap();
    let (_, prefix, _) = LossyGroupDecoder::with_directory(
        info.clone(),
        p.color,
        parsed.declared,
        parsed.data,
        tile_limits(),
        &session,
    )
    .unwrap();
    let object_start = parsed.data.as_ptr() as usize - file.as_ptr() as usize + prefix;
    let mut reader = LossyGroupDecoder::new(
        info,
        p.color,
        parsed.declared,
        parsed.data,
        tile_limits(),
        &session,
    )
    .unwrap()
    .0;
    let mut at = object_start;
    let end = loop {
        let event = reader.read(&file[at..]).unwrap();
        let entry = event.entry();
        if matches!(event, LossyEvent::End { .. }) {
            break at;
        }
        at += entry.entry.record_len as usize;
    };
    let mut damaged = file.clone();
    // End body starts after the one-byte tag and one-byte length in this fixture.
    assert_eq!(damaged[end], 67);
    assert!(damaged[end + 1] < 128);
    damaged[end + 2] ^= 1;
    let check = DecodeSession::new(DecodePolicy::default());
    let opened = LossyFile::open(&damaged, tile_limits(), &check).unwrap();
    let mut delivered = 0;
    assert!(
        opened
            .visit_groups(|_| {
                delivered += 1;
                Ok(())
            })
            .is_err()
    );
    assert_eq!(delivered, p.stripes.len());
    for cut in [0, 1, object_start, end, file.len() - 1] {
        let check = DecodeSession::new(DecodePolicy::default());
        let mut out = vec![0; p.info.checked_buffer_len().unwrap()];
        assert!(
            LossyFile::open(&file[..cut], tile_limits(), &check)
                .and_then(|f| f.decode_into(&mut out, p.info.row_bytes()))
                .is_err()
        );
    }
    let cancelled = DecodeSession::with_cancel(DecodePolicy::default(), || true);
    assert!(matches!(
        LossyFile::open(&file, tile_limits(), &cancelled),
        Err(Error::Cancelled)
    ));
    let denied = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(1));
    assert!(matches!(
        LossyFile::open(&file, tile_limits(), &denied),
        Err(Error::TooLarge)
    ));
}

#[test]
fn complete_lossy_files_retain_channel_plans_and_mixed_partitions() {
    use crate::{
        ChannelSampling as Sampling, LossyChannelLayout as Layout, LossyTransform as Transform,
    };
    let info = ImageInfo::new(41, 29, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    let mut cases = Vec::new();
    for transform in [
        Transform::Dct8,
        Transform::Dct4,
        Transform::Dct2,
        Transform::Adaptive4,
    ] {
        for sampling in [
            Sampling::Full,
            Sampling::HalfHorizontal,
            Sampling::HalfVertical,
            Sampling::HalfBoth,
        ] {
            cases.push((Some([Layout::new(transform, sampling); 3]), false));
        }
    }
    cases.extend([(None, false), (None, true)]);
    for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for (layouts, defaults) in &cases {
            let config = ImiEncoderConfig::lossy(58.0)
                .with_column_groups(Some(16))
                .with_coding_defaults(*defaults)
                .tune_lossy(|t| {
                    t.color = LossyColor::YCoCg;
                    t.entropy = coder;
                    t.stripe_block_rows = 2;
                    t.exact_blocks = Some(0.0);
                    t.lab.seed_bank = 0;
                    t.aq_gamma = 1.0;
                    t.freq_zero_bias = true;
                    if let Some(layouts) = layouts {
                        t.channel_layouts = *layouts;
                    } else {
                        t.adaptive_partitions = true;
                    }
                });
            let native = config
                .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
                .unwrap();
            let expected = crate::decode(&native).unwrap().1;
            let v4 = complete_file(&native);
            #[cfg(feature = "std")]
            crate::v4::feed_tests::check_stream(&v4, &expected, &info);
            let session = DecodeSession::new(DecodePolicy::default());
            let mut actual = vec![0; expected.len()];
            LossyFile::open(&v4, tile_limits(), &session)
                .unwrap()
                .decode_into(&mut actual, info.row_bytes())
                .unwrap();
            assert_eq!(
                actual, expected,
                "{coder:?} layouts={layouts:?} defaults={defaults}"
            );
        }
    }
}

#[test]
fn exact_packing_shares_stripes_between_groups_without_delaying_unrelated_jobs() {
    let info = ImageInfo::new(41, 77, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        let config = ImiEncoderConfig::lossy(93.0)
            .with_column_groups(Some(16))
            .tune_lossy(|t| {
                t.color = LossyColor::YCoCg;
                t.entropy = coder;
                t.stripe_block_rows = 2;
                t.exact_blocks = Some(1000.0);
                t.aq_gamma = 0.0;
                t.lab.seed_bank = 0;
            });
        let native = config
            .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
            .unwrap();
        let expected = crate::decode(&native).unwrap().1;
        let p = parse_lossy(&native).unwrap();
        let exact = p.exact.as_ref().expect("fixture must select exact blocks");
        assert!(
            exact
                .ranges
                .iter()
                .any(|&(start, n)| n > 0 && !start.is_multiple_of(exact.per_stripe()))
        );
        let v4 = complete_file(&native);
        #[cfg(feature = "std")]
        crate::v4::feed_tests::check_stream(&v4, &expected, &info);
        let check = DecodeSession::new(DecodePolicy::default());
        let mut out = vec![0; expected.len()];
        LossyFile::open(&v4, tile_limits(), &check)
            .unwrap()
            .decode_into(&mut out, info.row_bytes())
            .unwrap();
        assert_eq!(out, expected);
        let check = DecodeSession::new(DecodePolicy::default());
        let mut jobs = Vec::new();
        LossyFile::open(&v4, tile_limits(), &check)
            .unwrap()
            .visit_groups(|work| {
                jobs.push(work);
                Ok(())
            })
            .unwrap();
        assert_eq!(jobs.len(), p.stripes.len());
        for work in jobs.into_iter().rev() {
            let rect = work.rect().unwrap();
            let row = rect.width() as usize * info.bytes_per_pixel();
            let mut output = vec![0; row * rect.height() as usize];
            work.decode_into(
                &mut output,
                row,
                &DecodeSession::new(DecodePolicy::default()),
            )
            .unwrap();
            for y in 0..rect.height() as usize {
                let at = ((rect.y() as usize + y) * info.width as usize + rect.x() as usize)
                    * info.bytes_per_pixel();
                assert_eq!(&output[y * row..][..row], &expected[at..][..row]);
            }
        }
    }
}

#[test]
fn nonuniform_dc_and_keep_controls_match_native_reference_output() {
    let info = ImageInfo::new(41, 77, Channels::Rgb, SampleType::U8);
    let reference = pixels(&info);
    let mut current = reference.clone();
    for y in 24..40 {
        for x in 8..29 {
            current[(y * info.width as usize + x) * 3] ^= 0x5A;
        }
    }
    for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for aq in [false, true] {
            for keep in [false, true] {
                let config = ImiEncoderConfig::lossy(67.0)
                    .with_column_groups(Some(16))
                    .with_coding_defaults(true)
                    .with_dc_coverage(vec![vec![1, 3, 1], vec![2, 1, 2], vec![3, 2]])
                    .tune_lossy(|t| {
                        t.color = LossyColor::YCoCg;
                        t.entropy = coder;
                        t.stripe_block_rows = 2;
                        t.exact_blocks = Some(0.0);
                        t.aq_gamma = if aq { 1.0 } else { 0.0 };
                        t.lab.seed_bank = 0;
                    });
                let cfg = config.lossy_config();
                let native = encode_lossy_frame(
                    &current,
                    info.row_bytes(),
                    &info,
                    &Metadata::default(),
                    &cfg,
                    keep.then_some((reference.as_slice(), info.row_bytes())),
                )
                .unwrap();
                let p = parse_lossy(&native).unwrap();
                assert!(p.dc_coverage.is_some());
                assert_eq!(p.skip, keep);
                let mut expected = reference.clone();
                let source = crate::v1::parse_file(
                    &native,
                    &crate::parse::limits::Unrestricted,
                    crate::checksum::Verify::Now(1),
                )
                .unwrap();
                decode_lossy_frame(
                    info.clone(),
                    3,
                    p.color,
                    source.declared,
                    source.data,
                    &mut expected,
                    info.row_bytes(),
                    1,
                    keep,
                )
                .unwrap();
                let v4 = complete_file(&native);
                let check = DecodeSession::new(DecodePolicy::default());
                let mut out = vec![0; expected.len()];
                LossyFile::open(&v4, tile_limits(), &check)
                    .unwrap()
                    .decode_with_reference(
                        &mut out,
                        info.row_bytes(),
                        keep.then(|| ImageRef::packed(&reference, &info)),
                    )
                    .unwrap();
                assert_eq!(out, expected, "{coder:?} aq{aq} keep{keep}");
                if keep {
                    let check = DecodeSession::new(DecodePolicy::default());
                    assert!(
                        LossyFile::open(&v4, tile_limits(), &check)
                            .unwrap()
                            .decode_into(&mut out, info.row_bytes())
                            .is_err()
                    );
                }
            }
        }
    }
}

#[test]
fn file_seed_policy_preserves_trellis_coefficients_and_pixels() {
    let info = ImageInfo::new(65, 49, Channels::Rgb, SampleType::U8);
    let mut random = 19u32;
    let source = (0..info.row_bytes() * info.height as usize)
        .map(|_| {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            (random >> 24) as u8
        })
        .collect::<Vec<_>>();
    let image = ImageRef::packed(&source, &info);
    let meta = Metadata::default();
    for (quality, effort) in [(5.0, 9.0), (50.0, 10.0)] {
        for (coder, tier) in [
            (LossyEntropy::RunLevel, None),
            (LossyEntropy::SplitHuf, None),
            (LossyEntropy::RunLevel, Some(crate::DecodeSpeed::Compact)),
        ] {
            let config = ImiEncoderConfig::lossy(quality)
                .with_effort(effort)
                .with_decode_speed(tier)
                .with_threads(1)
                .with_column_groups(Some(u32::MAX))
                .tune_lossy(|t| {
                    t.color = LossyColor::Xyb;
                    t.entropy = coder;
                    t.stripe_block_rows = 8;
                    t.exact_blocks = Some(0.0);
                });
            let seeded = config.encode_samples(image, &meta).unwrap();
            let explicit = config
                .tune_lossy(|t| t.lab.seed_bank = 0)
                .encode_samples(image, &meta)
                .unwrap();
            let ctx = (quality, effort, coder, tier);
            assert_eq!(crate::parse::header::header_version(&seeded).unwrap(), 4);
            assert_eq!(inspect_models(&explicit).unwrap().1.bank, 0);
            let (_, _, seeded_q, _, _) = inspect_quantized(&seeded).unwrap();
            let (_, _, explicit_q, _, _) = inspect_quantized(&explicit).unwrap();
            assert!(seeded_q == explicit_q, "coefficient disagreement: {ctx:?}");
            assert!(
                crate::decode(&seeded).unwrap().1 == crate::decode(&explicit).unwrap().1,
                "pixel disagreement: {ctx:?}",
            );
        }
    }
}

#[test]
fn migrated_seed_local_and_shared_model_files_keep_native_pixels_and_metadata() {
    let info = ImageInfo::new(137, 129, Channels::Rgba, SampleType::U8);
    let source = pixels(&info);
    for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        for bank in [0, 1] {
            for mode in 0..3 {
                if mode == 1 && coder == LossyEntropy::SplitHuf {
                    continue;
                }
                let config = ImiEncoderConfig::lossy(83.0)
                    .with_column_groups(Some(64))
                    .with_coding_defaults(true)
                    .with_local_ans_contexts(mode == 1)
                    .with_model_references(mode == 2)
                    .tune_lossy(|t| {
                        t.color = LossyColor::YCoCg;
                        t.entropy = coder;
                        t.stripe_block_rows = 2;
                        t.exact_blocks = Some(0.0);
                        t.lab.seed_bank = bank;
                        t.lab.seed_exact = true;
                        t.aq_gamma = 0.5;
                    });
                let mut native = config
                    .encode_legacy_samples(ImageRef::packed(&source, &info), &Metadata::default())
                    .unwrap();
                // An unknown ancillary chunk must survive migration verbatim.
                crate::container::put_chunk(&mut native, b"xTRA", b"retained ancillary bytes");
                let expected = crate::decode(&native).unwrap().1;
                let migrated = complete_file(&native);
                #[cfg(feature = "std")]
                crate::v4::feed_tests::check_stream(&migrated, &expected, &info);
                assert!(
                    migrated
                        .windows(b"retained ancillary bytes".len())
                        .any(|w| w == b"retained ancillary bytes")
                );
                let check = DecodeSession::new(DecodePolicy::default());
                let mut out = vec![0; expected.len()];
                LossyFile::open(&migrated, tile_limits(), &check)
                    .unwrap()
                    .decode_into(&mut out, info.row_bytes())
                    .unwrap();
                assert_eq!(out, expected, "{coder:?} bank{bank} mode{mode}");
                let limits = crate::UpgradeLimits {
                    max_encoded_bytes: native.len() - 1,
                    ..Default::default()
                };
                assert!(matches!(
                    crate::v4::upgrade_image(&native, &limits),
                    Err(Error::TooLarge)
                ));
            }
        }
    }
}
