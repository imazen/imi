use super::*;
use imij::ImiEncoderConfig;
use imij::planes::{AttachmentDescriptor, PlaneSetBuilder, SliceReader};
use imij::{
    Channels, FrameConfig, FrameDuration, FrameEncoder, ImageReference, Metadata, SampleType,
    SequenceEncoder, SequenceItem, SequenceKind,
};
use imij_set::{EntryMetadata, Summary, Value, Writer};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

fn config() -> ImiEncoderConfig {
    let mut cfg = imij::ImiEncoderConfig::default();
    cfg = cfg.with_effort(0.0);
    cfg = cfg.with_threads(1);
    cfg
}
fn directory() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = PathBuf::from(std::env::var_os("HOME").unwrap()).join("tmp");
    fs::create_dir_all(&root).unwrap();
    root.join(format!(
        "imij-extract-test-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
fn sequence(sample: SampleType) -> (Vec<u8>, ImageInfo, Vec<Vec<u8>>, Vec<Metadata>) {
    let info = ImageInfo::new(33, 17, Channels::Rgba, sample);
    let first: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|n| (n * 79 + n / 13) as u8)
        .collect();
    let mut third = first.clone();
    third[11] ^= 3;
    let frames = vec![
        first.clone(),
        first.iter().map(|n| n ^ 0x69).collect(),
        third,
    ];
    let metadata: Vec<_> = (0..3)
        .map(|i| {
            let mut m = Metadata {
                xmp: Some(format!("frame-{i}").into_bytes()),
                orientation: 6,
                content_light_level: Some([0, 0, 0, i, 0, 0, 0, 2]),
                ..Default::default()
            };
            let mut kv = EntryMetadata::default();
            kv.attributes
                .insert("variant".into(), Value::Unsigned(i.into()));
            kv.write_metadata(&mut m).unwrap();
            m
        })
        .collect();
    let mut encoder = SequenceEncoder::new(SequenceKind::Pages, FrameConfig::new(config()));
    for (i, frame) in frames.iter().enumerate() {
        let mut item =
            SequenceItem::new(frame, info.row_bytes(), &info).with_metadata(&metadata[i]);
        if i == 2 {
            item = item.with_reference(ImageReference {
                index: 0,
                pixels: &frames[0],
                stride: info.row_bytes(),
            });
        }
        encoder.push(item).unwrap();
    }
    (
        imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap(),
        info,
        frames,
        metadata,
    )
}
fn flatten(bytes: &[u8]) -> Vec<Vec<u8>> {
    let result = RefCell::new(Vec::<Vec<u8>>::new());
    materialize(
        bytes,
        &PIXEL_LIMITS,
        &config(),
        |i| Ok(result.borrow()[i].clone()),
        |_, _, bytes| {
            result.borrow_mut().push(bytes.to_vec());
            Ok(())
        },
    )
    .unwrap();
    result.into_inner()
}

#[test]
fn standalone_outputs_preserve_nonadjacent_references_bits_and_metadata() {
    for sample in [SampleType::U8, SampleType::F32] {
        let (bytes, info, pixels, metadata) = sequence(sample);
        let mut source = FrameDecoder::new(&bytes).unwrap();
        source.seek_frame(2).unwrap();
        assert_eq!(source.reference_index(), Some(0));
        let output = flatten(&bytes);
        drop(source);
        drop(bytes);
        assert_eq!(output.len(), 3);
        for (i, bytes) in output.iter().enumerate() {
            assert_eq!(
                imij::decode(bytes).unwrap(),
                (info.clone(), pixels[i].clone())
            );
            let mut standalone = FrameDecoder::new(bytes).unwrap();
            assert_eq!(standalone.kind(), FrameKind::Still);
            assert_eq!(standalone.frame_count(), 1);
            standalone.next_frame().unwrap().unwrap();
            assert_eq!(standalone.reference_index(), None);
            assert_eq!(standalone.frame_metadata(), Some(&metadata[i]));
        }
    }
}

#[test]
fn legacy_animation_exports_full_canvases_including_empty_updates() {
    let info = ImageInfo::new(33, 17, Channels::Rgba, SampleType::U8);
    let first: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 13) as u8)
        .collect();
    let mut changed = first.clone();
    changed[200] ^= 1;
    let frames = [first, changed.clone(), changed];
    let mut encoder = FrameEncoder::animation(
        info.clone(),
        Metadata::default(),
        FrameConfig::new(config()),
    );
    for frame in &frames {
        encoder
            .push_frame(frame, info.row_bytes(), FrameDuration::from_millis(20))
            .unwrap();
    }
    let encoded = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
    let mut check = FrameDecoder::new(&encoded).unwrap();
    check.next_frame().unwrap();
    assert!(check.next_frame().unwrap().unwrap().uses_previous);
    assert!(check.next_frame().unwrap().unwrap().coding.is_none());
    let files = flatten(&encoded);
    for (file, expected) in files.iter().zip(frames) {
        assert_eq!(imij::decode(file).unwrap(), (info.clone(), expected));
    }
}

#[test]
fn lossy_sequence_is_losslessly_materialized_from_reconstruction() {
    let info = ImageInfo::new(33, 17, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 13) as u8)
        .collect();
    let mut config = imij::ImiEncoderConfig::lossy(90.0);
    config = config.with_quality(30.0);
    config = config.with_effort(0.0);
    config = config.with_threads(1);
    let mut encoder =
        FrameEncoder::pages(info.clone(), Metadata::default(), FrameConfig::new(config));
    encoder.push_page(&pixels, info.row_bytes(), &info).unwrap();
    encoder.push_page(&pixels, info.row_bytes(), &info).unwrap();
    let bytes = encoder.finish().unwrap();
    let mut original = FrameDecoder::new(&bytes).unwrap();
    let files = flatten(&bytes);
    for file in files {
        original.next_frame().unwrap().unwrap();
        let mut reconstructed = vec![0; pixels.len()];
        original
            .decode_into(&mut reconstructed, info.row_bytes(), 1)
            .unwrap();
        assert_ne!(reconstructed, pixels);
        assert_eq!(imij::decode(&file).unwrap().1, reconstructed);
        assert_eq!(imij::header(&file).unwrap().coding, imij::Coding::Lossless);
    }
}

#[test]
fn mixed_pages_keep_each_pixel_description() {
    let descriptions = [
        ImageInfo::new(9, 7, Channels::Gray, SampleType::U16),
        ImageInfo::new(13, 3, Channels::Oklab, SampleType::F32),
    ];
    let pixels: Vec<Vec<u8>> = descriptions
        .iter()
        .map(|info| {
            (0..info.checked_buffer_len().unwrap())
                .map(|i| (i * 67) as u8)
                .collect()
        })
        .collect();
    let mut encoder = FrameEncoder::mixed_pages(FrameConfig::new(config()));
    for (info, pixels) in descriptions.iter().zip(&pixels) {
        encoder
            .push_page_with(
                pixels,
                info.row_bytes(),
                info,
                &Metadata::default(),
                &config(),
            )
            .unwrap();
    }
    let files = flatten(&encoder.finish().unwrap());
    assert_eq!(files.len(), descriptions.len());
    for ((file, info), expected) in files.iter().zip(descriptions).zip(pixels) {
        assert_eq!(imij::decode(file).unwrap(), (info, expected));
    }
}

#[test]
fn corrupt_archive_copy_retains_only_partial_blob_and_manifest() {
    let mut packed = archive(b"opaque");
    let entry = Reader::open(SliceReader::new(&packed))
        .unwrap()
        .get(42)
        .unwrap()
        .unwrap();
    packed[entry.offset() as usize] ^= 1;
    let output = directory();
    assert!(
        extract(
            &mut Reader::open(SliceReader::new(&packed)).unwrap(),
            &output,
            false,
            &config(),
        )
        .is_err()
    );
    assert!(output.join("42.partial").exists());
    assert!(!output.join("42.bin").exists());
    assert!(output.join("images.tsv.partial").exists());
    assert!(!output.join("images.tsv").exists());
}

fn archive(bytes: &[u8]) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new());
    let mut metadata = EntryMetadata::default();
    metadata.filename = Some("../do-not-use-as-a-path.imij".into());
    metadata
        .attributes
        .insert("quality".into(), Value::Unsigned(30));
    writer
        .insert_with_metadata(
            42,
            bytes.len() as u64,
            0,
            Summary::default(),
            &metadata,
            &mut &bytes[..],
        )
        .unwrap();
    writer
        .insert(43, 3, 0, Summary::default(), &mut &b"abc"[..])
        .unwrap();
    writer.finish().unwrap().0
}

#[test]
fn default_extraction_is_standalone_and_keep_sequences_is_byte_exact() {
    let (bytes, info, pixels, _) = sequence(SampleType::U8);
    let packed = archive(&bytes);
    let standalone = directory();
    extract(
        &mut Reader::open(SliceReader::new(&packed)).unwrap(),
        &standalone,
        false,
        &config(),
    )
    .unwrap();
    let kept = directory();
    extract(
        &mut Reader::open(SliceReader::new(&packed)).unwrap(),
        &kept,
        true,
        &config(),
    )
    .unwrap();
    assert_eq!(fs::read(kept.join("42.imij")).unwrap(), bytes);
    drop(bytes);
    drop(packed);
    // Read each output without any archive or reference file supplied to decode.
    for (i, expected) in pixels.into_iter().enumerate() {
        assert_eq!(
            imij::decode(&fs::read(frame_path(&standalone, 42, i)).unwrap()).unwrap(),
            (info.clone(), expected)
        );
    }
    assert_eq!(fs::read(standalone.join("43.bin")).unwrap(), b"abc");
    let meta = EntryMetadata::from_bytes(&fs::read(standalone.join("42.kvmd")).unwrap()).unwrap();
    assert_eq!(
        meta.filename.as_deref(),
        Some("../do-not-use-as-a-path.imij")
    );
    assert_eq!(meta.attributes["quality"], Value::Unsigned(30));
    assert!(standalone.join("images.tsv").is_file());
    assert!(
        extract(
            &mut Reader::open(SliceReader::new(&archive(b"new"))).unwrap(),
            &standalone,
            false,
            &config(),
        )
        .is_err()
    );
    assert_eq!(fs::read(standalone.join("43.bin")).unwrap(), b"abc");
}

#[test]
fn plane_sequences_refuse_flattening_but_whole_entry_is_preserved() {
    let (bytes, _, _, _) = sequence(SampleType::U8);
    let mut builder = PlaneSetBuilder::new(&bytes).unwrap();
    builder
        .add_attachment(
            AttachmentDescriptor::new("calibration", "application/octet-stream"),
            b"keep me",
        )
        .unwrap();
    let planes = builder.finish().unwrap();
    let error = materialize(
        &planes,
        &PIXEL_LIMITS,
        &config(),
        |_| unreachable!(),
        |_, _, _| unreachable!(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("plane attachments"));
    let output = directory();
    extract(
        &mut Reader::open(SliceReader::new(&archive(&planes))).unwrap(),
        &output,
        true,
        &config(),
    )
    .unwrap();
    assert_eq!(fs::read(output.join("42.imij")).unwrap(), planes);
}

#[test]
fn limits_and_wrong_references_fail_before_emitting_a_dependent_image() {
    let (bytes, info, _, _) = sequence(SampleType::U8);
    let limit = {
        let mut cfg = PIXEL_LIMITS;
        cfg.max_bytes = 1;
        cfg
    };
    assert!(
        materialize(
            &bytes,
            &limit,
            &config(),
            |_| unreachable!(),
            |_, _, _| unreachable!()
        )
        .is_err()
    );
    let wrong = config()
        .encode_samples(
            imij::ImageRef::new(
                &vec![0; info.checked_buffer_len().unwrap()],
                info.row_bytes(),
                &info,
            ),
            &Metadata::default(),
        )
        .unwrap();
    let mut emitted = 0;
    let error = materialize(
        &bytes,
        &PIXEL_LIMITS,
        &config(),
        |_| Ok(wrong.clone()),
        |_, _, _| {
            emitted += 1;
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(emitted, 2);
    assert!(
        error
            .to_string()
            .contains("reference raster checksum mismatch")
    );
}

#[test]
fn extraction_effort_validation_and_exact_outputs() {
    let parse = |flags: &[&str]| {
        options(
            ["archive", "directory"]
                .into_iter()
                .chain(flags.iter().copied())
                .map(Into::into),
        )
    };
    assert_eq!(parse(&[]).unwrap().config.effort(), 0.0);
    for flags in [
        vec!["--effort"],
        vec!["--effort", "NaN"],
        vec!["--effort", "inf"],
        vec!["--effort", "-1"],
        vec!["--effort", "10.1"],
        vec!["--effort", "2", "--effort", "3"],
        vec!["--keep-sequences", "--effort", "2"],
        vec!["--effort", "2", "--keep-sequences"],
        vec!["--unknown"],
    ] {
        assert!(parse(&flags).is_err(), "{flags:?}");
    }
    for sample in [SampleType::U8, SampleType::F32] {
        let (bytes, info, pixels, metadata) = sequence(sample);
        let packed = archive(&bytes);
        for effort in ["0", "2.5", "7.5", "10"] {
            let cfg = parse(&["--effort", effort]).unwrap().config;
            let dest = directory();
            extract(
                &mut Reader::open(SliceReader::new(&packed)).unwrap(),
                &dest,
                false,
                &cfg,
            )
            .unwrap();
            for (i, expected) in pixels.iter().enumerate() {
                let output = fs::read(frame_path(&dest, 42, i)).unwrap();
                assert_eq!(
                    imij::decode(&output).unwrap(),
                    (info.clone(), expected.clone())
                );
                // Also prove the selected effort reached the encoder, including metadata.
                let direct = cfg
                    .encode_samples(
                        imij::ImageRef::new(expected, info.row_bytes(), &info),
                        &metadata[i],
                    )
                    .unwrap();
                assert_eq!(output, direct);
            }
        }
    }
}
