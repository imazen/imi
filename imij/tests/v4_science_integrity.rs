#![cfg(all(feature = "encode", feature = "extended"))]

use imij::planes::{
    LevelDescriptor, PlaneDescriptor, PlaneRole, PlaneSetBuilder, PlaneSetLimits, PlaneSetReader,
    Rect, SliceReader,
};
use imij::region::RegionReader;
use imij::science::{
    CborEvent, CborLimits, CborReader, ChannelCalibration, IntegerNodata, ScienceRecords,
    StructuredFormat, StructuredMetadata, structured_identity,
};
use imij::{
    Channels, Cicp, Error, FrameConfig, FrameDecoder, ImageInfo, ImageRef, ImiEncoderConfig,
    Metadata, SampleType, SequenceEncoder, SequenceItem, SequenceKind,
};

fn item_metadata(info: &ImageInfo, key: &str) -> Metadata {
    let mut records = ScienceRecords::default();
    records.structured.push(
        StructuredMetadata::new("sensor", StructuredFormat::Cbor, vec![0xf5])
            .with_instance_key(key),
    );
    records.structured.push(
        StructuredMetadata::new("sensor", StructuredFormat::Cbor, vec![0xf4])
            .with_instance_key("reference"),
    );
    let mut calibration = ChannelCalibration::default();
    calibration.integer_nodata = Some(IntegerNodata::Unsigned(vec![u64::MAX - 1]));
    records.calibration.push(calibration);
    let mut metadata = Metadata::default();
    records.write_metadata(info, &mut metadata).unwrap();
    let mut attributes = imij::item_metadata::ItemMetadata::default();
    attributes.filename = Some(key.into());
    attributes.write_metadata(&mut metadata).unwrap();
    metadata
}

#[test]
fn signed_nodata_preserves_adjacent_values_beyond_binary64_precision() {
    let info = ImageInfo::new(2, 1, Channels::Scalar, SampleType::I64);
    let mut records = ScienceRecords::default();
    let mut calibration = ChannelCalibration::default();
    calibration.integer_nodata = Some(IntegerNodata::Signed(vec![i64::MIN + 1]));
    records.calibration.push(calibration);
    let mut metadata = Metadata::default();
    records.write_metadata(&info, &mut metadata).unwrap();
    let decoded = ScienceRecords::from_metadata(&info, &metadata).unwrap();
    assert_eq!(decoded, records);
    assert!(decoded.calibration[0].is_missing_i64(i64::MIN + 1));
    assert!(!decoded.calibration[0].is_missing_i64(i64::MIN));
    let unsigned = ImageInfo::new(2, 1, Channels::Scalar, SampleType::U64);
    assert!(ScienceRecords::from_metadata(&unsigned, &metadata).is_err());
}

#[test]
fn indexed_scalar_sequence_preserves_item_metadata_and_per_ordinal_hashes() {
    let info = ImageInfo::new(4, 3, Channels::Scalar, SampleType::U64);
    assert_eq!(info.cicp, Cicp::UNSPECIFIED);
    assert_eq!(info.channels.color_count(), 0);
    assert!(!info.channels.has_alpha());
    let a = vec![0; info.row_bytes() * info.height as usize];
    let b: Vec<u8> = (0..a.len() as u8).collect();
    let metadata_a = item_metadata(&info, "first");
    let metadata_b = item_metadata(&info, "second");
    let mut sequence = SequenceEncoder::new(
        SequenceKind::Pages,
        FrameConfig::new(ImiEncoderConfig::lossless().with_checksum(true)),
    );
    sequence
        .push(SequenceItem::new(&a, info.row_bytes(), &info).with_metadata(&metadata_a))
        .unwrap();
    sequence
        .push(SequenceItem::new(&b, info.row_bytes(), &info).with_metadata(&metadata_b))
        .unwrap();
    let file = imij::add_pixel_checksum(sequence.finish().unwrap()).unwrap();
    let mut region = RegionReader::open(SliceReader::new(&file)).unwrap();
    let outer = region.header().clone();
    let csum = region.file_checksum_chunk().unwrap();
    assert_eq!(&file[csum.start as usize..][..4], b"cSUM");
    let first = region.pixel_checksum_for(0).unwrap().unwrap();
    let second = region.pixel_checksum_for(1).unwrap().unwrap();
    assert_ne!(first, second);
    assert!(region.pixel_checksum_for(2).is_err());
    let frame = FrameDecoder::new(&file).unwrap();
    let span = frame.frame_seek_info(1).unwrap().encoded.unwrap();
    let nested = &file[span.start as usize..span.end as usize];
    let data = nested
        .windows(4)
        .position(|candidate| candidate == b"DATA")
        .unwrap()
        + 4;
    let end = data
        + nested[data..]
            .iter()
            .position(|byte| byte & 128 == 0)
            .unwrap()
        + 1;
    let (probe_info, probe_meta) = imij::probe_indexed_nested(&nested[..end], &outer).unwrap();
    assert_eq!(probe_info, info);
    assert_eq!(
        imij::item_metadata::ItemMetadata::from_metadata(&probe_meta)
            .unwrap()
            .filename
            .as_deref(),
        Some("second"),
    );
    let observed = ScienceRecords::from_metadata(&probe_info, &probe_meta).unwrap();
    assert_eq!(
        observed,
        ScienceRecords::from_metadata(&info, &metadata_b).unwrap()
    );
    let chunk = probe_meta
        .chunks
        .iter()
        .find(|chunk| &chunk.tag == b"sMDT")
        .unwrap();
    let identity = structured_identity(&chunk.data[..16]).unwrap().unwrap();
    assert_eq!(identity.schema, "sensor");
    assert_eq!(identity.instance_key, Some("second"));
    assert!(observed.calibration[0].is_missing_u64(u64::MAX - 1));
    assert!(!observed.calibration[0].is_missing_u64(u64::MAX));
    let mut out = vec![0xa7; a.len()];
    let (_, decoded_metadata) = imij::decode_indexed_nested_with_metadata(
        nested,
        &outer,
        None,
        &mut out,
        info.row_bytes(),
        1,
        None,
    )
    .unwrap();
    assert_eq!(out, b);
    assert_eq!(
        ScienceRecords::from_metadata(&info, &decoded_metadata).unwrap(),
        observed
    );
    let (crop, _) = imij::decode_indexed_nested_with_metadata(
        nested,
        &outer,
        None,
        &mut out[..8],
        8,
        1,
        Some(Rect::new(1, 1, 1, 1)),
    )
    .unwrap();
    assert_eq!((crop.width, crop.height), (1, 1));
    assert_eq!(&out[..8], &b[40..48]);
}

#[test]
fn scalar_plane_roles_survive_a_directory_beyond_the_previous_limit() {
    let info = ImageInfo::new(1, 1, Channels::Scalar, SampleType::U8);
    let metadata = Metadata::default();
    let coding = ImiEncoderConfig::lossless();
    let image = ImageRef::packed(&[5], &info);
    let primary = coding.encode_samples(image, &metadata).unwrap();
    assert!(
        ImiEncoderConfig::lossy(90.)
            .encode_samples(ImageRef::packed(&[5], &info), &metadata)
            .is_err()
    );
    let mut planes = PlaneSetBuilder::new(&primary).unwrap();
    let mut last = None;
    for i in 0..1_024 {
        let id = planes
            .add_plane(
                PlaneDescriptor::new(format!("column-{i}"), info.sample)
                    .with_role(PlaneRole::Scalar),
            )
            .unwrap();
        planes
            .add_level(
                id,
                LevelDescriptor::new(1, 1, 1, 1),
                &[i as u8],
                1,
                coding.clone(),
            )
            .unwrap();
        last = Some(id);
    }
    let file = planes.finish().unwrap();
    let mut reader =
        PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    assert_eq!(reader.planes().len(), 1_024);
    assert_eq!(reader.planes()[1_023].role(), &PlaneRole::Scalar);
    assert_eq!(reader.planes()[1_023].cicp(), Cicp::UNSPECIFIED);
    let mut out = [0];
    reader
        .decode_region(last.unwrap(), 0, Rect::new(0, 0, 1, 1), &mut out, 1)
        .unwrap();
    assert_eq!(out, [255]);
}

#[test]
fn deterministic_cbor_events_preserve_integer_precision_and_bound_work() {
    let data = b"\xa2\x61a\x1b\xff\xff\xff\xff\xff\xff\xff\xff\x61b\x81\xf6";
    let mut reader = CborReader::new(data, CborLimits::default()).unwrap();
    for event in [
        CborEvent::Map(2),
        CborEvent::Key("a"),
        CborEvent::Unsigned(u64::MAX),
        CborEvent::Key("b"),
        CborEvent::Array(1),
        CborEvent::Null,
        CborEvent::End,
        CborEvent::End,
    ] {
        assert_eq!(reader.next_event().unwrap(), Some(event));
    }
    assert!(reader.next_event().unwrap().is_none());
    for malformed in [
        b"\xa2\x61a\x00\x61a\x01".as_slice(),
        b"\x81\x18\x00",
        b"\x80\x00",
    ] {
        let mut reader = CborReader::new(malformed, CborLimits::default()).unwrap();
        loop {
            match reader.next_event() {
                Ok(Some(_)) => {}
                Err(Error::Corrupt(_)) => break,
                other => panic!("accepted malformed CBOR: {other:?}"),
            }
        }
    }
    let mut reader = CborReader::new(
        data,
        CborLimits {
            max_items: 2,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(reader.next_event(), Err(Error::TooLarge)));
}

#[cfg(feature = "blake3")]
#[test]
fn wide_image_checks_keep_full_results_and_fixed_slot_width() {
    use imij::{Digest, HashAlgorithm};
    let algorithm = HashAlgorithm::Blake3_1024;
    let full = algorithm.digest(b"");
    // BLAKE3 project's official zero-length XOF vector.
    assert_eq!(
        format!("{full:x}"),
        concat!(
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262e",
            "00f03e7b69af26b7faaf09fcd333050338ddfe085b8cc869ca98b206c08243a26f",
            "5487789e8f660afe6c99ef9e0c52b92e7393024a80459cf91f476f9ffdbda7001",
            "c22e159b402631f277ca96f2defdf1078282314e763699a31c5363165421",
        )
    );
    assert_eq!(Digest::MAX_WIDTH, 32);
    assert!(full.try_to_slot().is_none());
    assert!(Digest::from_slot(algorithm, &[0; 32]).is_none());
    assert!(full.matches_prefix(HashAlgorithm::Blake3_256.digest(b"").as_bytes()));
    let info = ImageInfo::new(8, 8, Channels::Scalar, SampleType::U8);
    let px = [5; 64];
    let file = ImiEncoderConfig::lossless()
        .with_checksum(true)
        .with_hash_algorithm(algorithm)
        .encode_samples(ImageRef::packed(&px, &info), &Metadata::default())
        .unwrap();
    let file = imij::add_pixel_checksum_with(file, algorithm).unwrap();
    assert_eq!(imij::decode(&file).unwrap().1, px);
    let mut region = RegionReader::open(SliceReader::new(&file)).unwrap();
    assert_eq!(
        region
            .pixel_checksum_for(0)
            .unwrap()
            .unwrap()
            .as_bytes()
            .len(),
        128
    );
    let mut out = [0; 64];
    region
        .decode_region(Rect::new(0, 0, 8, 8), &mut out, 8)
        .unwrap();
    assert_eq!(out, px);
}
