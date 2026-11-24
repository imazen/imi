use super::*;
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use alloc::{vec, vec::Vec};

#[test]
fn unindexed_open_data_retains_later_metadata_and_exact_pixels() {
    let info = ImageInfo::new(41, 31, Channels::Rgba, SampleType::U8);
    let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 53) as u8)
        .collect();
    for config in [
        ImiEncoderConfig::lossless(),
        ImiEncoderConfig::quick(),
        ImiEncoderConfig::lossy(73.0).with_column_groups(Some(16)),
    ] {
        let native = config
            .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
            .unwrap();
        let expected = crate::decode(&native).unwrap().1;
        let original = upgrade_image(&native, &crate::UpgradeLimits::default()).unwrap();
        let mut file =
            super::file_tests::rewrite_placement(&original, DirectoryPlacement::None, true);
        crate::container::put_chunk(&mut file, b"xTRA", b"after open DATA");
        assert_eq!(crate::decode(&file).unwrap().1, expected);
        let metadata =
            crate::v4::file::parse_file(&file, &DecodeSession::new(DecodePolicy::default()))
                .unwrap()
                .metadata()
                .unwrap();
        assert_eq!(metadata.chunks.last().unwrap().data, b"after open DATA");
        super::feed_tests::check_stream(&file, &expected, &info);
        let trace: serde_json::Value =
            serde_json::from_str(&crate::lab::codestream_trace(&file)).unwrap();
        assert_eq!(trace["decode"], "ok");
        assert_eq!(trace["checks"]["problems"], serde_json::json!([]));
        assert_eq!(trace["checks"]["unattributed"], serde_json::json!([]));
    }
}

fn with_post_tile_ancillary(file: &[u8]) -> Vec<u8> {
    let mut r = crate::ByteReader::new(file);
    crate::parse::header::read_header_v4(&mut r, 64 << 10).unwrap();
    let header_end = r.pos;
    let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
    assert_eq!(&tag, b"DATA");
    let mut data = crate::parse::chunks::payload(&mut r, len.unwrap())
        .unwrap()
        .to_vec();
    let (tag, len) = crate::parse::chunks::read_head(&mut r).unwrap();
    assert_eq!(&tag, b"uIDX");
    let body = crate::parse::chunks::payload(&mut r, len.unwrap()).unwrap();
    let index = &body[..body.len() - 12];
    let mut count = crate::ByteReader::new(index);
    let entries = count.varint().unwrap();
    // An independently formed, locally unchecksummed ignorable object.
    let extra = [100, 3, b'e', b'n', b'd'];
    data.extend_from_slice(&extra);
    let mut updated = Vec::new();
    crate::put_varint(&mut updated, entries + 1);
    updated.extend_from_slice(&index[count.pos..index.len() - 8]);
    let mut encoded = [0; 40];
    let used = Entry {
        kind: EntryKind::Ignorable,
        role: 0,
        id: 0,
        record_len: extra.len() as u64,
    }
    .encode(&mut encoded);
    updated.extend_from_slice(&encoded[..used]);
    let objects_len = u64::from_le_bytes(index[index.len() - 8..].try_into().unwrap());
    updated.extend_from_slice(&(objects_len + extra.len() as u64).to_le_bytes());
    let chunk_len = crate::parse::records::chunk_len_of(updated.len() as u64 + 12);
    updated.extend_from_slice(&chunk_len.to_le_bytes());
    updated.extend_from_slice(b"uIDX");
    let mut output = file[..header_end].to_vec();
    crate::container::put_data(&mut output, &data);
    crate::container::put_chunk(&mut output, b"uIDX", &updated);
    output
}

#[test]
fn bounded_data_keeps_post_tile_objects_while_open_data_ends_at_tile() {
    let info = ImageInfo::new(41, 31, Channels::Gray, SampleType::U8);
    let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 53) as u8)
        .collect();
    let limits = TileLimits {
        max_object_bytes: 1 << 20,
        max_tile_bytes: 16 << 20,
    };
    let file = encode_numeric(
        info.clone(),
        &pixels,
        info.row_bytes(),
        &Metadata::default(),
        &ImiEncoderConfig::lossless().with_row_groups(crate::RowGroups::new(9, 7, 8 << 20)),
        SpatialConfig {
            tile_columns: 3,
            tile_rows: 2,
            order: TileOrder::Morton,
            integrity: ObjectIntegrity::None,
            limits,
        },
    )
    .unwrap();
    let extended = with_post_tile_ancillary(&file);
    for placement in [
        DirectoryPlacement::Tail,
        DirectoryPlacement::Front,
        DirectoryPlacement::None,
    ] {
        let bounded = super::file_tests::rewrite_placement(&extended, placement, false);
        assert_eq!(crate::decode(&bounded).unwrap().1, pixels);
        super::feed_tests::check_stream(&bounded, &pixels, &info);
        let open = super::file_tests::rewrite_placement(&extended, placement, true);
        assert!(matches!(
            crate::decode(&open),
            Err(crate::Error::Corrupt(_))
        ));
        let mut output = vec![0; pixels.len()];
        assert!(
            crate::decode_reader(open.as_slice(), &crate::Limits::DEFAULT, |_, y, row| {
                let at = y as usize * info.row_bytes();
                output[at..at + row.len()].copy_from_slice(row);
                Ok(())
            })
            .is_err()
        );
        if placement != DirectoryPlacement::None {
            let session = DecodeSession::new(DecodePolicy::default());
            NumericTileReader::open(crate::planes::SliceReader::new(&bounded), limits, &session)
                .unwrap();
            assert!(matches!(
                NumericTileReader::open(crate::planes::SliceReader::new(&open), limits, &session),
                Err(crate::Error::Corrupt(_))
            ));
        }
    }
}
