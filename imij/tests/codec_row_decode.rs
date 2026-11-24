//! Native row pull preserves allocating-decode pixels and checksum lifecycle.
#[path = "common/wire.rs"]
mod common;
use imij::codec::{DecodeConfig, DecodeError};
use imij::{Channels, Error, ImageInfo, Metadata, PixelChecksumStatus, SampleType};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use zencodec::decode::{DecodeJob, DecoderConfig, StreamingDecode};
use zencodec::{ResourceLimits, StopToken};
use zenpixels::PixelDescriptor;

trait OpenRows {
    fn test_rows<'a>(
        self,
        owned: bool,
        file: &'a [u8],
        preferred: &[PixelDescriptor],
    ) -> Result<imij::codec::RowDecoder<'a>, DecodeError>;
}
impl OpenRows for imij::codec::DecodeRequest {
    fn test_rows<'a>(
        self,
        owned: bool,
        file: &'a [u8],
        preferred: &[PixelDescriptor],
    ) -> Result<imij::codec::RowDecoder<'a>, DecodeError> {
        if owned {
            self.owned_row_decoder(file.to_vec(), preferred)
        } else {
            self.row_decoder(file, preferred)
        }
    }
}

fn fixture(sample: SampleType, checksum: bool) -> (Vec<u8>, ImageInfo, Vec<u8>, Metadata) {
    let info = ImageInfo::new(19, 17, Channels::Rgba, sample);
    let row = info.row_bytes();
    let stride = row + 3 * info.bytes_per_pixel();
    let mut padded = vec![0xee; stride * info.height as usize];
    let mut tight = Vec::new();
    let floats = [
        0u32, 0x80000000, 1, 0x3f800000, 0x7f800000, 0xff800000, 0x7f800001, 0x7fc12abc,
    ];
    let half = [0u16, 0x8000, 1, 0x3c00, 0x7c00, 0xfc00, 0x7c01, 0x7eab];
    for y in 0..info.height as usize {
        let bytes: Vec<_> = match sample {
            SampleType::F32 => (0..row / 4)
                .flat_map(|x| floats[(x + y) % 8].to_ne_bytes())
                .collect(),
            SampleType::F16 => (0..row / 2)
                .flat_map(|x| half[(x + y) % 8].to_ne_bytes())
                .collect(),
            _ => (0..row).map(|x| (x * 71 + y * 37) as u8).collect(),
        };
        padded[y * stride..y * stride + row].copy_from_slice(&bytes);
        tight.extend_from_slice(&bytes);
    }
    let metadata = Metadata {
        orientation: 1,
        exif: Some(b"native exif".to_vec()),
        xmp: Some(b"<rows/>".to_vec()),
        ..Default::default()
    };
    let file = {
        let mut cfg = imij::ImiEncoderConfig::default();
        cfg = cfg.with_effort(0.0);
        cfg = cfg.tune_lossless(|t| {
            t.stripe_rows = Some(3);
        });
        cfg = cfg.with_column_groups(Some(7));
        cfg = cfg.with_checksum(checksum);
        cfg
    }
    .encode_samples(imij::ImageRef::new(&padded, stride, &info), &metadata)
    .unwrap();
    for y in 0..info.height as usize {
        assert!(
            padded[y * stride + row..(y + 1) * stride]
                .iter()
                .all(|&b| b == 0xee)
        );
    }
    (
        imij::add_pixel_checksum(file).unwrap(),
        info,
        tight,
        metadata,
    )
}

fn compare_rows(file: &[u8], owned: bool) {
    let (info, expected) = imij::decode(file).unwrap();
    let mut decoder = DecodeConfig::default()
        .job()
        .test_rows(owned, file, &[])
        .unwrap();
    assert_eq!(decoder.rows_decoded(), 0);
    assert_eq!(decoder.native_metadata().image, info);
    assert_eq!(
        decoder.pixel_checksum_status(),
        PixelChecksumStatus::Pending
    );
    let usage = decoder.usage();
    assert!(usage.work_units > 0);
    let mut descriptor = None;
    let mut context = None;
    for y in 0..info.height {
        let (actual_y, pixels) = decoder.next_batch().unwrap().unwrap();
        assert_eq!(actual_y, y);
        assert_eq!((pixels.width(), pixels.rows()), (info.width, 1));
        assert_eq!(
            pixels.row(0),
            &expected[y as usize * info.row_bytes()..(y as usize + 1) * info.row_bytes()]
        );
        if let Some(first) = descriptor {
            assert_eq!(pixels.descriptor(), first);
        } else {
            descriptor = Some(pixels.descriptor());
        }
        let color = pixels.color_context().cloned();
        assert!(color.is_some());
        if y == 0 {
            context = color;
        } else {
            assert_eq!(color, context);
        }
        assert_eq!(decoder.rows_decoded(), y + 1);
        assert_eq!(
            decoder.pixel_checksum_status(),
            if y + 1 == info.height {
                PixelChecksumStatus::Verified
            } else {
                PixelChecksumStatus::Pending
            }
        );
        // Construction admits the complete request; pulling rows does not
        // repeatedly charge the container or full planned reconstruction.
        assert_eq!(decoder.usage().work_units, usage.work_units);
        assert_eq!(decoder.usage().input_bytes, usage.input_bytes);
    }
    assert!(decoder.next_batch().unwrap().is_none());
    assert!(decoder.next_batch().unwrap().is_none());
}

#[test]
fn baseline_raw_bits_metadata_and_psum_survive_one_row_batches() {
    for owned in [false, true] {
        for sample in [
            SampleType::U8,
            SampleType::U16,
            SampleType::F16,
            SampleType::F32,
        ] {
            let (file, info, tight, metadata) = fixture(sample, true);
            assert_eq!(imij::decode(&file).unwrap(), (info.clone(), tight));
            let decoder = DecodeConfig::default()
                .job()
                .test_rows(owned, &file, &[])
                .unwrap();
            assert_eq!(decoder.native_metadata().metadata, metadata);
            assert_eq!(
                decoder.info().embedded_metadata.exif.as_deref(),
                metadata.exif.as_deref()
            );
            compare_rows(&file, owned);
        }
    }
}

#[test]
fn lossy_and_native_jpeg_rows_use_the_allocating_reconstruction() {
    for owned in [false, true] {
        let (_, info, tight, metadata) = fixture(SampleType::U8, false);
        let lossy = imij::ImiEncoderConfig::lossy(60.0)
            .with_effort(0.0)
            .encode_samples(
                imij::ImageRef::new(&tight, info.row_bytes(), &info),
                &metadata,
            )
            .unwrap();
        let expected = imij::decode(&lossy).unwrap().1;
        assert_ne!(expected, tight);
        let (decoded_pixels, decoded_tail) = expected.as_chunks::<4>();
        let (source_pixels, source_tail) = tight.as_chunks::<4>();
        assert!(decoded_tail.is_empty() && source_tail.is_empty());
        for (actual, source) in decoded_pixels.iter().zip(source_pixels) {
            assert_eq!(actual[3], source[3]);
        }
        compare_rows(&imij::add_pixel_checksum(lossy).unwrap(), owned);
        #[cfg(feature = "native-jpeg")]
        {
            let native = imij::jpeg::CoefficientImage {
                width: 9,
                height: 17,
                color: imij::jpeg::ComponentColor::Gray,
                siting: imij::jpeg::ChromaSiting::Unspecified,
                tables: [Some([1; 64]), None, None, None],
                components: vec![imij::jpeg::QuantizedComponent {
                    info: imij::jpeg::ComponentInfo {
                        id: 3,
                        horizontal_sampling: 1,
                        vertical_sampling: 1,
                        quant_table: 0,
                        blocks_wide: 2,
                        blocks_high: 3,
                    },
                    coefficients: vec![0; 2 * 3 * 64],
                }],
            };
            let file = native
                .encode(&Metadata::default(), 1, imij::LossyEntropy::RunLevel, 7.5)
                .unwrap();
            compare_rows(&imij::add_pixel_checksum(file).unwrap(), owned);
        }
    }
}

#[test]
fn memory_limit_can_admit_rows_while_excluding_the_full_raster() {
    for owned in [false, true] {
        let info = ImageInfo::new(129, 8193, Channels::Rgb, SampleType::U8);
        let pixels = vec![37; info.checked_buffer_len().unwrap()];
        let file = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(0.0);
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(1);
            });
            cfg = cfg.tune_lossless(|t| {
                t.palette = false;
            });
            cfg
        }
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
        let cap = 1 << 20;
        assert!(pixels.len() as u64 > cap);
        let mut decoder = DecodeConfig::default()
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .test_rows(owned, &file, &[])
            .unwrap_or_else(|error| {
                let probe = DecodeConfig::default()
                    .job()
                    .test_rows(owned, &file, &[])
                    .unwrap();
                panic!(
                    "{error:?}: owned={owned}, encoded={}, admission={:?}",
                    file.len(),
                    probe.usage()
                )
            });
        assert_eq!(decoder.rows_decoded(), 0);
        assert!(decoder.usage().peak_planned_working_bytes <= cap);
        for y in 0..info.height {
            let (actual_y, row) = decoder.next_batch().unwrap().unwrap();
            assert_eq!(actual_y, y);
            assert!(row.row(0).iter().all(|&b| b == 37));
        }
        assert!(decoder.next_batch().unwrap().is_none());
    }
}

#[test]
// Also checks the legacy job `with_stop`, deprecated in zencodec 0.2; imij keeps it.
#[allow(deprecated)]
fn cancellation_after_a_delivered_row_is_sticky() {
    for owned in [false, true] {
        #[derive(Clone)]
        struct Flag(Arc<AtomicBool>);
        impl zencodec::enough::Stop for Flag {
            fn check(&self) -> Result<(), zencodec::enough::StopReason> {
                if self.0.load(Ordering::SeqCst) {
                    Err(zencodec::enough::StopReason::Cancelled)
                } else {
                    Ok(())
                }
            }
        }
        let (file, info, pixels, _) = fixture(SampleType::U8, false);
        let flag = Flag(Arc::new(AtomicBool::new(false)));
        let mut decoder = DecodeConfig::default()
            .job()
            .with_stop(StopToken::new(flag.clone()))
            .test_rows(owned, &file, &[])
            .unwrap();
        let (y, row) = decoder.next_batch().unwrap().unwrap();
        assert_eq!(y, 0);
        assert_eq!(row.row(0), &pixels[..info.row_bytes()]);
        flag.0.store(true, Ordering::SeqCst);
        assert!(matches!(
            decoder.next_batch(),
            Err(DecodeError::Codec(Error::Cancelled))
        ));
        flag.0.store(false, Ordering::SeqCst);
        assert!(decoder.next_batch().is_err());
        assert_eq!(decoder.rows_decoded(), 1);
        assert_eq!(
            decoder.pixel_checksum_status(),
            PixelChecksumStatus::Pending
        );
    }
}

#[test]
fn incorrect_psum_fails_on_final_row_and_remains_failed() {
    for owned in [false, true] {
        let (mut file, info, pixels, _) = fixture(SampleType::U8, false);
        let (_, _, end) = common::chunks(&file)
            .into_iter()
            .find(|c| c.0 == *b"pSUM")
            .unwrap();
        file[end - 1] ^= 1;
        let mut decoder = DecodeConfig::default()
            .job()
            .test_rows(owned, &file, &[])
            .unwrap();
        for y in 0..info.height - 1 {
            let (actual_y, row) = decoder.next_batch().unwrap().unwrap();
            assert_eq!(actual_y, y);
            assert_eq!(
                row.row(0),
                &pixels[y as usize * info.row_bytes()..(y as usize + 1) * info.row_bytes()]
            );
        }
        assert!(matches!(
            decoder.next_batch(),
            Err(DecodeError::Codec(Error::Corrupt(
                "pixel checksum mismatch"
            )))
        ));
        assert!(matches!(
            decoder.next_batch(),
            Err(DecodeError::Codec(Error::Corrupt(
                "pixel checksum mismatch"
            )))
        ));
        assert_eq!(
            decoder.pixel_checksum_status(),
            PixelChecksumStatus::Pending
        );
    }
}

#[test]
fn complete_container_checks_and_limits_precede_pixels() {
    for owned in [false, true] {
        let (file, info, _, _) = fixture(SampleType::U8, true);
        let mut bad_checksum = file.clone();
        let (at, _) = common::chunks(&file)
            .into_iter()
            .find_map(|(tag, at, end)| (tag == *b"DATA").then_some((at, end)))
            .unwrap();
        let payload = at + 4 + common::varint(&file[at + 4..]).1;
        bad_checksum[payload + 1] ^= 1;
        for bad in [Vec::new(), file[..file.len() / 2].to_vec(), bad_checksum] {
            assert!(
                DecodeConfig::default()
                    .job()
                    .test_rows(owned, &bad, &[])
                    .is_err()
            );
        }
        for limits in [
            ResourceLimits::none().with_max_input_bytes(file.len() as u64 - 1),
            ResourceLimits::none().with_max_pixels(u64::from(info.width * info.height) - 1),
            ResourceLimits::none().with_max_memory(1),
        ] {
            assert!(matches!(
                DecodeConfig::default()
                    .job()
                    .with_limits(limits)
                    .test_rows(owned, &file, &[]),
                Err(DecodeError::Codec(Error::TooLarge))
            ));
        }
        assert!(matches!(
            DecodeConfig::default()
                .with_admission(
                    imij::decode_resources::DecodePolicy::default().with_max_work_units(0)
                )
                .job()
                .test_rows(owned, &file, &[]),
            Err(DecodeError::Codec(Error::TooLarge))
        ));
        let mut preferred = DecodeConfig::default()
            .job()
            .test_rows(owned, &file, &[PixelDescriptor::GRAY8_SRGB])
            .unwrap();
        let (_, row) = preferred.next_batch().unwrap().unwrap();
        assert_eq!(row.descriptor(), PixelDescriptor::RGBA8_SRGB);
    }
}

#[test]
fn owned_input_can_outlive_the_caller_and_reserves_actual_capacity() {
    fn create() -> imij::codec::RowDecoder<'static> {
        let (file, _, _, _) = fixture(SampleType::U8, true);
        DecodeConfig::default()
            .job()
            .owned_row_decoder(file, &[])
            .unwrap()
    }
    fn send_static<T: Send + 'static>(_: &T) {}
    let mut decoder = create();
    send_static(&decoder);
    assert!(decoder.next_batch().unwrap().is_some());
    let (mut file, _, _, _) = fixture(SampleType::U8, false);
    file.reserve_exact(2 << 20);
    let cap = file.capacity() as u64 - 1;
    assert!(matches!(
        DecodeConfig::default()
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .owned_row_decoder(file, &[]),
        Err(DecodeError::Codec(Error::TooLarge))
    ));
}

#[test]
fn exact_nested_records_and_merged_blocks_survive_owned_offsets() {
    let info = ImageInfo::new(192, 160, Channels::Rgb, SampleType::U8);
    let mut pixels = vec![0; info.checked_buffer_len().unwrap()];
    for y in 0..160usize {
        for x in 0..192usize {
            let px = &mut pixels[(y * 192 + x) * 3..][..3];
            if y < 80 {
                let ink = (x % 9 < 2 && y % 13 > 2) || (y % 13 == 7 && x % 17 < 11);
                px.copy_from_slice(if ink { &[20, 30, 90] } else { &[250, 250, 245] });
            } else {
                for (c, value) in px.iter_mut().enumerate() {
                    *value = (30.0 + x as f32 * 0.6 + y as f32 * 0.3 + c as f32 * 25.0) as u8;
                }
            }
        }
    }
    let cfg = imij::ImiEncoderConfig::lossy(40.0).with_decode_speed(imij::DecodeSpeed::Compact);
    let file = cfg
        .encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let features = imij::header(&file).unwrap().features;
    assert!(features.contains(imij::Feature::ExactBlocks));
    assert!(features.contains(imij::Feature::Superblocks));
    compare_rows(&imij::add_pixel_checksum(file).unwrap(), true);
}

#[test]
#[cfg(feature = "native-jpeg")]
fn owned_native_420_crosses_band_boundaries_with_both_coders() {
    let mut components = Vec::new();
    for i in 0..3 {
        let (sx, sy, bw, bh) = if i == 0 { (2, 2, 4, 10) } else { (1, 1, 2, 5) };
        let mut coefficients = vec![0; bw * bh * 64];
        for (n, block) in coefficients.as_chunks_mut::<64>().0.iter_mut().enumerate() {
            block[0] = (n as i16 * 17 + i as i16 * 29) % 151 - 70;
            block[1] = 2;
        }
        components.push(imij::jpeg::QuantizedComponent {
            info: imij::jpeg::ComponentInfo {
                id: i as u8 + 1,
                horizontal_sampling: sx,
                vertical_sampling: sy,
                quant_table: 0,
                blocks_wide: bw as u32,
                blocks_high: bh as u32,
            },
            coefficients,
        });
    }
    let image = imij::jpeg::CoefficientImage {
        width: 19,
        height: 65,
        color: imij::jpeg::ComponentColor::YCbCr,
        siting: imij::jpeg::ChromaSiting::Centered,
        tables: [Some([3; 64]), None, None, None],
        components,
    };
    for entropy in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf] {
        let file = image.encode(&Metadata::default(), 1, entropy, 7.5).unwrap();
        compare_rows(&imij::add_pixel_checksum(file).unwrap(), true);
    }
}

#[test]
fn offset_conversion_work_is_admitted_before_owned_rows() {
    // V3 borrowed parsing uses slices; its owned adapter converts their offsets.
    // V4 uses offsets in both forms and has no such additional conversion.
    let (_, info, pixels, metadata) = fixture(SampleType::U8, false);
    let file = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(imij::ImageRef::packed(&pixels, &info), &metadata)
        .unwrap();
    let borrowed = DecodeConfig::default()
        .job()
        .row_decoder(&file, &[])
        .unwrap();
    let work = borrowed.usage().work_units;
    let config = DecodeConfig::default()
        .with_admission(imij::decode_resources::DecodePolicy::default().with_max_work_units(work));
    assert!(config.clone().job().row_decoder(&file, &[]).is_ok());
    assert!(matches!(
        config.job().owned_row_decoder(file.clone(), &[]),
        Err(DecodeError::Codec(Error::TooLarge))
    ));
    let owned = DecodeConfig::default()
        .job()
        .owned_row_decoder(file, &[])
        .unwrap();
    assert!(owned.usage().work_units > work);
    assert_eq!(owned.rows_decoded(), 0);
}

#[test]
fn owned_input_reservation_applies_during_metadata_admission() {
    let info = ImageInfo::new(8, 8, Channels::Gray, SampleType::U8);
    let metadata = Metadata {
        xmp: Some(vec![b'x'; 64 << 10]),
        ..Default::default()
    };
    let mut file = imij::ImiEncoderConfig::default()
        .encode_samples(imij::ImageRef::new(&[7; 64], 8, &info), &metadata)
        .unwrap();
    file.reserve_exact(1 << 20);
    // Borrowed input leaves room for the metadata. Owned input consumes nearly
    // all of this same limit before metadata parsing/expansion begins.
    let cap = file.capacity() as u64 + 1024;
    let config = DecodeConfig::default();
    assert!(
        config
            .clone()
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .row_decoder(&file, &[])
            .is_ok()
    );
    assert!(matches!(
        config
            .job()
            .with_limits(ResourceLimits::none().with_max_memory(cap))
            .owned_row_decoder(file, &[]),
        Err(DecodeError::Codec(Error::TooLarge))
    ));
}
