use super::*;
use crate::Error;
use alloc::{vec, vec::Vec};

fn fixture(width: u32, height: u32, sampled: bool) -> CoefficientImage {
    let factors = if sampled {
        [(2, 2), (1, 1), (1, 1)]
    } else {
        [(1, 1); 3]
    };
    let components = factors
        .into_iter()
        .enumerate()
        .map(|(i, (hs, vs))| {
            let (w, h) = if sampled && i > 0 {
                (width.div_ceil(2), height.div_ceil(2))
            } else {
                (width, height)
            };
            let (bw, bh) = (w.div_ceil(8), h.div_ceil(8));
            let mut coefficients = vec![0i16; bw as usize * bh as usize * 64];
            for (b, q) in coefficients.as_chunks_mut::<64>().0.iter_mut().enumerate() {
                q[0] = ((b * 7 + i * 11) % 100) as i16 - 50;
                q[1] = if b % 2 == 0 { 250 } else { -250 };
                q[8] = 33;
            }
            QuantizedComponent {
                info: ComponentInfo {
                    id: i as u8 + 1,
                    horizontal_sampling: hs,
                    vertical_sampling: vs,
                    quant_table: i as u8,
                    blocks_wide: bw,
                    blocks_high: bh,
                },
                coefficients,
            }
        })
        .collect();
    CoefficientImage {
        width,
        height,
        color: ComponentColor::YCbCr,
        siting: ChromaSiting::Centered,
        tables: [Some([17; 64]), Some([31; 64]), Some([47; 64]), None],
        components,
    }
}

#[test]
fn native_components_and_rgb_support_strided_rows_and_keep_input() {
    for sampled in [false, true] {
        for (width, height) in [(1, 1), (7, 9), (17, 19), (33, 31)] {
            let image = fixture(width, height, sampled);
            let saved = image.clone();
            let (w, h) = image.component_size(1).unwrap();
            let mut plane = vec![f32::NAN; (w + 3) * h];
            image
                .reconstruct_component_into(1, &mut plane, w + 3)
                .unwrap();
            for row in plane.chunks(w + 3) {
                assert!(row[..w].iter().all(|v| (-128.0..=127.0).contains(v)));
                assert!(row[w..].iter().all(|v| v.is_nan()));
            }
            let (w, h) = (width as usize, height as usize);
            let mut tight = vec![0; w * h * 3];
            image.reconstruct_into(&mut tight, w * 3).unwrap();
            let stride = w * 3 + 11;
            let mut padded = vec![0xA5; stride * h];
            image.reconstruct_into(&mut padded, stride).unwrap();
            for y in 0..h {
                assert_eq!(
                    &padded[y * stride..y * stride + w * 3],
                    &tight[y * w * 3..(y + 1) * w * 3]
                );
                assert!(
                    padded[y * stride + w * 3..(y + 1) * stride]
                        .iter()
                        .all(|&v| v == 0xA5)
                );
            }
            assert_eq!(image, saved);
        }
    }
}

#[test]
fn validate_keeps_quantizers_ids_sampling_and_mcu_padding_explicit() {
    let mut image = fixture(17, 19, true);
    let mut padded = Vec::new();
    for row in image.components[0].coefficients.chunks(3 * 64) {
        padded.extend_from_slice(row);
        padded.extend_from_slice(&[123i16; 64]);
    }
    padded.extend_from_slice(&[456i16; 4 * 64]);
    image.components[0].info.blocks_wide = 4;
    image.components[0].info.blocks_high = 4;
    image.components[0].coefficients = padded;
    image.validate().unwrap();
    assert_eq!(image.component_size(0).unwrap(), (17, 19));
    image.components[0].info.blocks_wide = u32::MAX;
    assert!(image.validate().is_err());
    let mut image = fixture(17, 19, true);
    image.tables[1].as_mut().unwrap()[9] = 0;
    assert!(image.validate().is_err());
    image.tables[1] = Some([65535; 64]);
    image.validate().unwrap();
    image.components[1].info.id = image.components[0].info.id;
    assert!(image.validate().is_err());
    image.components[1].info.id = 2;
    image.components[0].info.vertical_sampling = 1;
    assert!(matches!(image.validate(), Err(Error::Unsupported(_))));
}

#[test]
fn cosited_source_survives_native_access_but_is_not_rendered_as_centered() {
    let mut image = fixture(17, 19, true);
    image.siting = ChromaSiting::Cosited;
    image.validate().unwrap();
    let mut component = vec![0.0; 17 * 19];
    image
        .reconstruct_component_into(0, &mut component, 17)
        .unwrap();
    let mut rgb = vec![0xA5; 17 * 19 * 3];
    assert!(matches!(
        image.reconstruct_into(&mut rgb, 17 * 3),
        Err(Error::Unsupported(_))
    ));
    assert!(rgb.iter().all(|&v| v == 0xA5));
}

#[test]
fn native_entropy_carriers_preserve_all_signed_frequencies_and_u16_tables() {
    for entropy in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
        for (bw, bh) in [(1, 1), (3, 5), (2, 73)] {
            let mut q = vec![0i16; bw * bh * 64];
            for (i, v) in q.iter_mut().enumerate() {
                *v = (i % 251) as i16 - 125;
            }
            for (i, block) in q.as_chunks_mut::<64>().0.iter_mut().enumerate() {
                block[0] = if i % 2 == 0 { i16::MIN } else { i16::MAX };
                block[1] = i16::MIN;
                block[63] = i16::MAX;
            }
            let table = core::array::from_fn(|i| if i % 2 == 0 { 1 } else { 65535 });
            let (w, h) = ((bw * 8) as u32, (bh * 8) as u32);
            let file =
                crate::lossy::encode_native_component(w, h, &table, &q, entropy, 7.5).unwrap();
            let back = crate::lossy::decode_native_component(&file, w, h, &table).unwrap();
            assert_eq!(back, q);
            assert!(crate::lossy::decode_native_component(&file, w + 8, h, &table).is_err());
            let mut wrong = table;
            wrong[0] = 2;
            assert!(crate::lossy::decode_native_component(&file, w, h, &wrong).is_err());
        }
    }
}

#[cfg(feature = "lab")]
#[test]
fn native_carrier_rejects_hidden_channels_and_explicit_color_matrices() {
    let info = crate::ImageInfo::new(8, 8, crate::Channels::Gray, crate::SampleType::U8);
    let table = [17u16; 64];
    let mut cfg = crate::LossyConfig::new(50.0);
    cfg.channel_mul = Some([1.0; 3]);
    cfg.column_groups = Some(0);
    cfg.exact_blocks = Some(0.0);
    cfg.lab.dc_step = Some([64, 64]);
    cfg.lab.explicit_matrix = Some(alloc::boxed::Box::new([[17.0; 64]; 3]));
    let mut q = [0i32; 192];
    q[64] = 1;
    q[129] = 17;
    let file =
        crate::lab::encode_lossy_quantized_lowered(&info, &crate::Metadata::default(), &cfg, &q)
            .unwrap();
    assert!(matches!(
        crate::lossy::decode_native_component(&file, 8, 8, &table),
        Err(Error::Corrupt(_))
    ));
    q.fill(0);
    cfg.lab.color_transform = Some(crate::ColorTransform::Explicit([
        [1.0, 0.0, 0.0, 128.0],
        [0.0, 1.0, 0.0, 128.0],
        [0.0, 0.0, 1.0, 128.0],
    ]));
    let file =
        crate::lab::encode_lossy_quantized_lowered(&info, &crate::Metadata::default(), &cfg, &q)
            .unwrap();
    assert!(matches!(
        crate::lossy::decode_native_component(&file, 8, 8, &table),
        Err(Error::Corrupt(_))
    ));
}

#[cfg(feature = "lab")]
#[test]
fn native_carrier_rejects_equivalent_non_unit_quantizer() {
    let info = crate::ImageInfo::new(8, 8, crate::Channels::Gray, crate::SampleType::U8);
    let cfg = crate::LossyConfig {
        quality: 25.0,
        color: crate::LossyColor::YCoCg,
        channel_mul: Some([1.0; 3]),
        column_groups: Some(0),
        exact_blocks: Some(0.0),
        lab: crate::LossyLabSwitches {
            explicit_matrix: Some(alloc::boxed::Box::new([[8.5; 64]; 3])),
            dc_step: Some([64; 2]),
            superblocks: Some(false),
            dering: Some(false),
            ..crate::LossyLabSwitches::defaults()
        },
        ..crate::LossyConfig::default()
    };
    let file =
        crate::lossy::encode_lossy_coefs(&info, &crate::Metadata::default(), &cfg, &[0; 192])
            .unwrap();
    assert!(matches!(
        crate::lossy::decode_native_component(&file, 8, 8, &[17; 64]),
        Err(Error::Corrupt(_))
    ));
}

#[test]
fn native_bands_keep_coefficients_and_plane_wide_halos_on_every_route() {
    for sampled in [false, true] {
        for entropy in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
            let mut image = fixture(17, 49, sampled);
            // Preserve actual coded MCU padding, including nonvisible spectra.
            if sampled {
                let p = &mut image.components[0];
                p.info.blocks_high = 8;
                p.coefficients.resize(3 * 8 * 64, -19);
            }
            image.tables[3] = Some([65535; 64]);
            let info = image.image_info();
            let mut expected = vec![0; info.checked_buffer_len().unwrap()];
            image
                .reconstruct_into(&mut expected, info.row_bytes())
                .unwrap();
            for rows in [1, 2, 256] {
                let file = image
                    .encode(&crate::Metadata::default(), rows, entropy, 7.5)
                    .unwrap();
                assert_eq!(decode_coefficients(&file).unwrap(), image);
                assert_eq!(crate::decode(&file).unwrap().1, expected);
                let stride = info.row_bytes() + 5;
                let mut out = vec![0xA5; stride * info.height as usize];
                crate::decode_into(&file, &mut out, stride).unwrap();
                for y in 0..info.height as usize {
                    assert_eq!(
                        &out[y * stride..y * stride + info.row_bytes()],
                        &expected[y * info.row_bytes()..(y + 1) * info.row_bytes()]
                    );
                    assert_eq!(
                        &out[y * stride + info.row_bytes()..(y + 1) * stride],
                        &[0xA5; 5]
                    );
                }
                let mut stream = crate::StreamDecoder::new(&file).unwrap();
                let mut streamed = vec![0; expected.len()];
                for row in streamed.chunks_mut(info.row_bytes()) {
                    assert!(stream.next_row(row).unwrap());
                }
                assert!(!stream.next_row(&mut vec![0; info.row_bytes()]).unwrap());
                assert_eq!(streamed, expected);
                let mut session = crate::decode_resources::DecodeSession::new(
                    crate::decode_resources::DecodePolicy::default(),
                );
                assert_eq!(session.decode(&file).unwrap().1, expected);
                let rect = crate::planes::Rect::new(2, 15, 9, 19);
                let mut region = vec![0; 9 * 19 * 3];
                crate::region::decode_region(&file, rect, &mut region, 27).unwrap();
                for y in 0..19 {
                    assert_eq!(
                        &region[y * 27..(y + 1) * 27],
                        &expected
                            [(y + 15) * info.row_bytes() + 6..(y + 15) * info.row_bytes() + 33]
                    );
                }
                #[cfg(feature = "std")]
                {
                    for threads in [1, 2, 4, 8] {
                        let mut out = vec![0; expected.len()];
                        crate::decode_into_threads(&file, &mut out, info.row_bytes(), threads)
                            .unwrap();
                        assert_eq!(out, expected);
                    }
                    let mut read = Vec::new();
                    crate::decode_reader(
                        std::io::Cursor::new(&file),
                        &crate::Limits::DEFAULT,
                        |_, _, row| {
                            read.extend_from_slice(row);
                            Ok(())
                        },
                    )
                    .unwrap();
                    assert_eq!(read, expected);
                    let mut read = Vec::new();
                    let mut session = crate::decode_resources::DecodeSession::new(
                        crate::decode_resources::DecodePolicy::default(),
                    );
                    session
                        .decode_reader(std::io::Cursor::new(&file), |_, _, row| {
                            read.extend_from_slice(row);
                            Ok(())
                        })
                        .unwrap();
                    assert_eq!(read, expected);
                }
            }
        }
    }
}

#[test]
fn band_encoder_rejects_incomplete_input_and_sessions_admit_before_writing() {
    let image = fixture(17, 49, true);
    let header = image.native_header().unwrap();
    let mut out = Vec::new();
    let mut writer = BandEncoder::new(
        header,
        &crate::Metadata::default(),
        1,
        crate::LossyEntropy::RunLevel,
        7.5,
        |b: &[u8]| {
            out.extend_from_slice(b);
            Ok(())
        },
    )
    .unwrap();
    assert!(matches!(
        writer.push_band(&[&[], &[], &[]]),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(writer.finish(), Err(Error::InvalidInput(_))));
    let file = image
        .encode(
            &crate::Metadata::default(),
            1,
            crate::LossyEntropy::RunLevel,
            7.5,
        )
        .unwrap();
    for policy in [
        crate::decode_resources::DecodePolicy::default().with_max_units(0),
        crate::decode_resources::DecodePolicy::default().with_max_working_bytes(1),
        crate::decode_resources::DecodePolicy::default().with_max_work_units(1),
    ] {
        let mut session = crate::decode_resources::DecodeSession::new(policy);
        let mut out = vec![0xA5; image.image_info().checked_buffer_len().unwrap()];
        assert_eq!(
            session.decode_into(&file, &mut out, image.image_info().row_bytes()),
            Err(Error::TooLarge)
        );
        assert!(out.iter().all(|&v| v == 0xA5));
    }
    let mut session = crate::decode_resources::DecodeSession::with_cancel(
        crate::decode_resources::DecodePolicy::default(),
        || true,
    );
    assert_eq!(session.decode(&file), Err(Error::Cancelled));
}

#[test]
fn native_archive_copies_whole_spans_and_mixes_reencoded_bands() {
    let image = fixture(17, 49, true);
    let file = image
        .encode(
            &crate::Metadata::default(),
            1,
            crate::LossyEntropy::RunLevel,
            7.5,
        )
        .unwrap();
    let archive = NativeArchive::open(&file).unwrap();
    assert_eq!(archive.band_count(), 4);
    let mut output = Vec::new();
    let mut writer = BandEncoder::new(
        archive.header().clone(),
        &crate::Metadata::default(),
        1,
        crate::LossyEntropy::RunLevel,
        7.5,
        |b: &[u8]| {
            output.extend_from_slice(b);
            Ok(())
        },
    )
    .unwrap();
    for band in 0..archive.band_count() {
        if band != 1 {
            writer.copy_band(&archive, band).unwrap();
            continue;
        }
        let rows: Vec<&[i16]> = image
            .components
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (width, rows) = writer.next_component_grid(i).unwrap();
                &p.coefficients[rows.start as usize * width as usize * 64
                    ..rows.end as usize * width as usize * 64]
            })
            .collect();
        writer.push_band(&rows).unwrap();
    }
    assert_eq!(writer.bands_remaining(), 0);
    writer.finish().unwrap();
    assert_eq!(decode_coefficients(&output).unwrap(), image);
    let copied = NativeArchive::open(&output).unwrap();
    for band in [0, 2, 3] {
        assert_eq!(
            archive.band_record(band).unwrap(),
            copied.band_record(band).unwrap()
        );
        let r = copied.band_span(band).unwrap();
        assert_eq!(&output[r], copied.band_record(band).unwrap());
    }
    let mut bad = archive.header().clone();
    bad.tables[0] = Some([1; 64]);
    let mut writer = BandEncoder::new(
        bad,
        &crate::Metadata::default(),
        1,
        crate::LossyEntropy::RunLevel,
        7.5,
        |_: &[u8]| Ok(()),
    )
    .unwrap();
    assert!(matches!(
        writer.copy_band(&archive, 0),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn native_archive_v2_copy_preserves_payloads_but_upgrade_is_rejected() {
    for entropy in [crate::LossyEntropy::RunLevel, crate::LossyEntropy::SplitHuf] {
        let image = fixture(17, 33, true);
        let file = image
            .encode(&crate::Metadata::default(), 1, entropy, 7.5)
            .unwrap();
        let archive = NativeArchive::open(&file).unwrap();
        assert_eq!(archive.header_version(), 2);
        assert_eq!(decode_coefficients(&file).unwrap(), image);
        assert_eq!(
            crate::upgrade_image(&file, &crate::UpgradeLimits::default()).unwrap_err(),
            Error::Unsupported("native components cannot be upgraded")
        );
        for with_side in [false, true] {
            let mut output = Vec::new();
            let mut writer = BandEncoder::new(
                archive.header().clone(),
                &crate::Metadata::default(),
                1,
                entropy,
                7.5,
                |bytes: &[u8]| {
                    output.extend_from_slice(bytes);
                    Ok(())
                },
            )
            .unwrap();
            for band in 0..archive.band_count() {
                if with_side {
                    // An ignorable v2 side record with uninterpreted contents.
                    let mut record = vec![30, 3, 0xa5, 0x5a, 0xff];
                    record.extend_from_slice(archive.band_record(band).unwrap());
                    writer
                        .copy_band_record(archive.header(), 2, 1, band, &record)
                        .unwrap();
                } else {
                    writer.copy_band(&archive, band).unwrap();
                }
            }
            writer.finish().unwrap();
            let copied = NativeArchive::open(&output).unwrap();
            assert_eq!(copied.header_version(), 2);
            assert_eq!(decode_coefficients(&output).unwrap(), image);
            for band in 0..archive.band_count() {
                let mut record = copied.band_record(band).unwrap();
                if with_side {
                    assert_eq!(&record[..5], &[30, 3, 0xa5, 0x5a, 0xff]);
                    record = &record[5..];
                }
                assert_eq!(record[0], 31);
                assert_eq!(&record[1..], &archive.band_record(band).unwrap()[1..]);
            }
        }
    }
}

#[test]
fn native_band_copy_rejects_unknown_version_before_writing() {
    let image = fixture(8, 8, false);
    let file = image
        .encode(
            &crate::Metadata::default(),
            1,
            crate::LossyEntropy::RunLevel,
            7.5,
        )
        .unwrap();
    let archive = NativeArchive::open(&file).unwrap();
    let written = core::cell::Cell::new(0usize);
    let mut writer = BandEncoder::new(
        archive.header().clone(),
        &crate::Metadata::default(),
        1,
        crate::LossyEntropy::RunLevel,
        7.5,
        |bytes: &[u8]| {
            written.set(written.get() + bytes.len());
            Ok(())
        },
    )
    .unwrap();
    let before = written.get();
    for version in [0, 1, 5, 255] {
        assert_eq!(
            writer.copy_band_record(
                archive.header(),
                version,
                1,
                0,
                archive.band_record(0).unwrap()
            ),
            Err(Error::Unsupported("unknown header version")),
        );
        assert_eq!(written.get(), before);
        assert_eq!(writer.bands_remaining(), 1);
    }
    writer.copy_band(&archive, 0).unwrap();
    writer.finish().unwrap();
}

#[cfg(feature = "std")]
#[test]
fn native_reader_admits_record_capacity_and_cancels_between_output_rows() {
    use core::sync::atomic::{AtomicBool, Ordering};
    let image = fixture(17, 49, true);
    let info = image.image_info();
    let mut prefix = Vec::new();
    let writer = BandEncoder::new(
        image.native_header().unwrap(),
        &crate::Metadata::default(),
        1,
        crate::LossyEntropy::RunLevel,
        7.5,
        |b: &[u8]| {
            prefix.extend_from_slice(b);
            Ok(())
        },
    )
    .unwrap();
    drop(writer);
    crate::put_varint(&mut prefix, 31);
    crate::put_varint(&mut prefix, 1 << 30);
    let mut session = crate::decode_resources::DecodeSession::new(
        crate::decode_resources::DecodePolicy::default(),
    );
    assert_eq!(
        session.decode_reader(std::io::Cursor::new(&prefix), |_, _, _| panic!(
            "unadmitted native record emitted pixels"
        )),
        Err(Error::TooLarge)
    );
    let file = image
        .encode(
            &crate::Metadata::default(),
            1,
            crate::LossyEntropy::RunLevel,
            7.5,
        )
        .unwrap();
    let cancel = AtomicBool::new(false);
    let mut rows = 0;
    let mut session = crate::decode_resources::DecodeSession::with_cancel(
        crate::decode_resources::DecodePolicy::default(),
        || cancel.load(Ordering::Relaxed),
    );
    let mut row = vec![0; info.row_bytes()];
    assert_eq!(
        session.decode_rows(&file, &mut row, |_, _, _| {
            rows += 1;
            cancel.store(true, Ordering::Relaxed);
            Ok(())
        }),
        Err(Error::Cancelled)
    );
    assert_eq!(rows, 1);
}

#[test]
fn native_band_producer_starts_without_full_height_coefficients_or_index() {
    let header = NativeHeader {
        width: 1,
        height: 1_000_000_000,
        color: ComponentColor::Gray,
        siting: ChromaSiting::Unspecified,
        tables: [Some([1; 64]), None, None, None],
        components: vec![ComponentInfo {
            id: 7,
            horizontal_sampling: 1,
            vertical_sampling: 1,
            quant_table: 0,
            blocks_wide: 1,
            blocks_high: 125_000_000,
        }],
    };
    header.validate().unwrap();
    let mut written = 0;
    #[cfg(feature = "std")]
    let mut prefix = Vec::new();
    let mut encoder = BandEncoder::new(
        header,
        &crate::Metadata::default(),
        1,
        crate::LossyEntropy::RunLevel,
        7.5,
        |b: &[u8]| {
            written += b.len();
            #[cfg(feature = "std")]
            prefix.extend_from_slice(b);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(encoder.next_component_grid(0).unwrap(), (1, 0..1));
    encoder.push_band(&[&[0; 64]]).unwrap();
    assert_eq!(encoder.bands_remaining(), 124_999_999);
    drop(encoder);
    assert!(written > 0 && written < 8192);
    #[cfg(feature = "std")]
    {
        let mut limits = crate::Limits::DEFAULT;
        limits.max_pixels = u64::MAX;
        limits.max_bytes = u64::MAX;
        let policy = crate::decode_resources::DecodePolicy::default()
            .with_output_limits(limits)
            .with_max_units(u64::MAX)
            .with_max_work_units(u64::MAX)
            .with_max_working_bytes(8 << 20);
        let mut session = crate::decode_resources::DecodeSession::new(policy);
        assert_eq!(
            session.decode_reader(std::io::Cursor::new(&prefix), |_, _, _| panic!(
                "unadmitted native index emitted pixels"
            )),
            Err(Error::TooLarge)
        );
    }
}

#[cfg(feature = "std")]
#[test]
fn native_gray_bands_use_every_shared_unit_table_form() {
    let mut image = fixture(7, 25, false);
    image.color = ComponentColor::Gray;
    image.components.truncate(1);
    let info = image.image_info();
    let mut expected = vec![0; info.checked_buffer_len().unwrap()];
    image
        .reconstruct_into(&mut expected, info.row_bytes())
        .unwrap();
    for form in [
        crate::v1::UnitTableForm::Hint,
        crate::v1::UnitTableForm::None,
        crate::v1::UnitTableForm::Trailing,
        crate::v1::UnitTableForm::Reserved(4),
        crate::v1::UnitTableForm::ReservedEmpty(4),
        crate::v1::UnitTableForm::ReservedTail(4),
    ] {
        let file = crate::v1::with_unit_table(form, || {
            image.encode(
                &crate::Metadata::default(),
                1,
                crate::LossyEntropy::RunLevel,
                7.5,
            )
        })
        .unwrap();
        assert_eq!(decode_coefficients(&file).unwrap(), image);
        assert_eq!(crate::decode(&file).unwrap().1, expected);
        let archive = NativeArchive::open(&file).unwrap();
        assert_eq!(archive.band_count(), 4);
        let mut ranged =
            crate::region::RegionReader::open(crate::planes::SliceReader::new(&file)).unwrap();
        assert_eq!(ranged.native_header(), Some(archive.header()));
        assert_eq!(ranged.native_mcu_rows_per_band(), Some(1));
        for band in 0..4 {
            let span = ranged.native_band_span(band).unwrap();
            let expected = archive.band_span(band).unwrap();
            assert_eq!(span, expected.start as u64..expected.end as u64);
        }
        assert!(matches!(
            ranged.native_band_span(4),
            Err(Error::InvalidInput(_))
        ));
    }
}

#[test]
fn fragmented_native_head_preserves_peek_and_record_boundaries() {
    for sampled in [false, true] {
        let file = fixture(17, 49, sampled)
            .encode(
                &crate::Metadata::default(),
                1,
                crate::LossyEntropy::SplitHuf,
                7.5,
            )
            .unwrap();
        let hp =
            crate::parse::header::read_header_sized(&mut crate::ByteReader::new(&file)).unwrap();
        let data = crate::container::parse(&file, false, &crate::v1::payload_len)
            .unwrap()
            .data;
        let mut reader = crate::ByteReader::new(data);
        crate::parse::payload::read_head(
            &hp.header,
            hp.declared,
            &mut reader,
            &crate::parse::limits::Unrestricted,
        )
        .unwrap();
        let head_len = reader.pos;
        for n in 0..=head_len {
            for step in [1, 7, 64] {
                crate::parse::payload::fragmented_tests::compare(
                    &hp.header,
                    hp.declared,
                    &data[..n],
                    step,
                );
            }
        }
        // Complete payload contains records after the head; they must remain unread.
        crate::parse::payload::fragmented_tests::compare(&hp.header, hp.declared, data, 3);
        #[cfg(feature = "std")]
        crate::parse::payload::fragmented_tests::decoded_pixels(&file);
    }
}
