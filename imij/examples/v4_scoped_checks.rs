//! Portable public-API witness: native and WASI must emit the same receipts.
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::v4::{
    self, CheckDescriptor, CheckLimits, Checks, ObjectIntegrity, SpatialConfig, TileLimits,
    TileOrder,
};
use imij::{
    Channels, HashAlgorithm, Hasher, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, RowGroups,
    SampleType,
};

fn verify(file: &[u8], pixels: &[u8], info: &ImageInfo) {
    let mut decoder = imij::StreamDecoder::new(file).unwrap();
    let mut row = vec![0xa7; info.row_bytes() + 17];
    for y in 0..info.height as usize {
        assert!(decoder.next_row(&mut row).unwrap());
        assert_eq!(
            &row[..info.row_bytes()],
            &pixels[y * info.row_bytes()..][..info.row_bytes()]
        );
        assert!(row[info.row_bytes()..].iter().all(|&p| p == 0xa7));
    }
    assert!(!decoder.next_row(&mut row).unwrap());
    let mut session = DecodeSession::new(DecodePolicy::default());
    let mut y = 0;
    session
        .decode_reader(file, |actual, actual_y, row| {
            assert_eq!(actual, info);
            assert_eq!(actual_y as usize, y);
            assert_eq!(row, &pixels[y * info.row_bytes()..][..info.row_bytes()]);
            y += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(y, info.height as usize);
}
fn main() {
    let descriptors = [
        CheckDescriptor::xxh3(HashAlgorithm::Xxh3_64),
        CheckDescriptor::xxh3(HashAlgorithm::Xxh3_128),
    ];
    let mut receipt = Hasher::new(HashAlgorithm::Xxh3_128);
    let mut cases = 0;
    for flags in 1..8 {
        let mut declaration = Vec::new();
        Checks::append(
            &mut declaration,
            core::array::from_fn(|i| {
                if flags & (1 << i) != 0 {
                    &descriptors[..]
                } else {
                    &[]
                }
            }),
            CheckLimits::default(),
        )
        .unwrap();
        let checks = Checks::read(&declaration, CheckLimits::default())
            .unwrap()
            .0;
        for sample in [
            SampleType::U8,
            SampleType::U16,
            SampleType::U32,
            SampleType::U64,
            SampleType::I8,
            SampleType::I16,
            SampleType::I32,
            SampleType::I64,
            SampleType::F16,
            SampleType::F32,
            SampleType::F64,
        ] {
            let info = ImageInfo::new(19, 23, Channels::Rgba, sample);
            let packed: Vec<_> = (0..info.checked_buffer_len().unwrap())
                .map(|i| (i * 73 + i / 11) as u8)
                .collect();
            let stride = info.row_bytes() + 13;
            let mut strided = vec![0xa7; stride * info.height as usize];
            for y in 0..info.height as usize {
                strided[y * stride..][..info.row_bytes()]
                    .copy_from_slice(&packed[y * info.row_bytes()..][..info.row_bytes()]);
            }
            for quick in [false, true] {
                let config = if quick {
                    ImiEncoderConfig::quick()
                } else {
                    ImiEncoderConfig::lossless()
                }
                .with_row_groups(RowGroups::new(7, 5, 8 << 20));
                let spatial = SpatialConfig {
                    tile_columns: 2,
                    tile_rows: 2,
                    order: TileOrder::Morton,
                    integrity: ObjectIntegrity::ObjectsAndTiles,
                    limits: TileLimits {
                        max_object_bytes: 1 << 20,
                        max_tile_bytes: 8 << 20,
                    },
                };
                let file = v4::encode_numeric_checked(
                    info.clone(),
                    &strided,
                    stride,
                    &Metadata::default(),
                    &config,
                    spatial,
                    checks,
                )
                .unwrap();
                verify(&file, &packed, &info);
                receipt.update(&(file.len() as u64).to_le_bytes());
                receipt.update(&file);
                cases += 1;
            }
        }
        let info = ImageInfo::new(39, 35, Channels::Rgba, SampleType::U8);
        let source: Vec<_> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 73 + i / 11) as u8)
            .collect();
        for coder in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf] {
            let original = ImiEncoderConfig::lossy(71.0)
                .with_column_groups(Some(16))
                .tune_lossy(|t| {
                    t.entropy = coder;
                    t.stripe_block_rows = 2;
                })
                .encode_samples(ImageRef::packed(&source, &info), &Metadata::default())
                .unwrap();
            let expected = imij::decode(&original).unwrap().1;
            let session = DecodeSession::new(DecodePolicy::default());
            let file = v4::add_scoped_checks(&original, checks, &session, 8 << 20).unwrap();
            verify(&file, &expected, &info);
            receipt.update(&(file.len() as u64).to_le_bytes());
            receipt.update(&file);
            cases += 1;
        }
    }
    let digest = receipt.finish();
    let hex: String = digest
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("scoped checks: {cases} public-API cases; XXH3-128 {hex}");
}
