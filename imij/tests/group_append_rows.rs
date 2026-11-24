#![cfg(feature = "std")]

use imij::decode_resources::DecodePolicy;
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::stream_encode::{AppendRowsEncoder, IndexedGroupEncoder, StreamEncodeError};
use imij::{Channels, Error, ImageInfo, Limits, Metadata, SampleType, decode_reader};
use std::io::Cursor;

const WORKING: usize = 32 << 20;

fn fixed(
    info: ImageInfo,
    metadata: &Metadata,
    cfg: imij::ImiEncoderConfig,
    pixels: &[u8],
) -> Vec<u8> {
    let row = info.row_bytes();
    let bpp = info.bytes_per_pixel();
    let mut writer = IndexedGroupEncoder::new(
        Vec::new(),
        Cursor::new(Vec::new()),
        info,
        metadata,
        &cfg,
        1000,
    )
    .unwrap();
    while let Some(g) = writer.next_group() {
        let at = g.y as usize * row + g.x as usize * bpp;
        writer.write_group(&pixels[at..], row).unwrap();
    }
    writer.finish().unwrap().0
}

#[test]
fn appending_complete_and_partial_stripes_matches_fresh_encode() {
    for sample in [SampleType::U8, SampleType::U16, SampleType::F32] {
        let old = ImageInfo::new(7, 4, Channels::GrayAlpha, sample);
        let mut new = old.clone();
        new.height = 7;
        let row = old.row_bytes();
        let pixels: Vec<u8> = (0..new.checked_buffer_len().unwrap())
            .map(|i| (i * 73 + i / 3 * 17) as u8)
            .collect();
        let meta = Metadata {
            orientation: 3,
            xmp: Some(b"<row-append/>".to_vec()),
            ..Metadata::default()
        };
        let cfg = imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(3, 2, WORKING)));
        let original = fixed(old.clone(), &meta, cfg.clone(), &pixels[..row * 4]);
        let expected = fixed(new.clone(), &meta, cfg.clone(), &pixels);
        let mut appended = AppendRowsEncoder::new(
            Cursor::new(original.as_slice()),
            Vec::new(),
            Cursor::new(Vec::new()),
            3,
            &cfg,
            1000,
            DecodePolicy::default(),
        )
        .unwrap();
        let mut copied = 0;
        while let Some(g) = appended.next_group() {
            assert!(g.y >= old.height);
            let at = g.y as usize * row + g.x as usize * old.bytes_per_pixel();
            appended.write_group(&pixels[at..], row).unwrap();
            copied += 1;
        }
        assert_eq!(copied, 6);
        let (file, _, source) = appended.finish().unwrap();
        assert_eq!(source.into_inner(), original.as_slice());
        assert_eq!(file, expected);
        let mut decoded = Vec::new();
        let (info, got_meta) = decode_reader(file.as_slice(), &Limits::DEFAULT, |_, _, row| {
            decoded.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(info, new);
        assert_eq!(got_meta, meta);
        assert_eq!(decoded, pixels);
        let mut region = RegionReader::open(SliceReader::new(&file)).unwrap();
        let mut around_join = vec![0; row * 4];
        region
            .decode_region(Rect::new(0, 3, 7, 4), &mut around_join, row)
            .unwrap();
        assert_eq!(around_join, pixels[row * 3..row * 7]);
    }
}

#[test]
fn incompatible_or_corrupt_sources_leave_destination_empty() {
    let info = ImageInfo::new(7, 4, Channels::Gray, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::default()
        .with_row_groups(Some(imij::RowGroups::new(3, 2, WORKING)));
    let pixels: Vec<u8> = (0..28).map(|v| v as u8).collect();
    let original = fixed(info.clone(), &Metadata::default(), cfg.clone(), &pixels);

    let mut output = Vec::new();
    let wrong = AppendRowsEncoder::new(
        Cursor::new(original.as_slice()),
        &mut output,
        Cursor::new(Vec::new()),
        1,
        &imij::ImiEncoderConfig::default()
            .with_row_groups(Some(imij::RowGroups::new(4, 2, WORKING))),
        1000,
        DecodePolicy::default(),
    );
    assert!(matches!(
        wrong,
        Err(StreamEncodeError::Codec(Error::Unsupported(_)))
    ));
    assert!(output.is_empty());

    let mut bad_index = original.clone();
    let footer = bad_index.len() - 20;
    let index_len = u64::from_le_bytes(bad_index[footer + 8..footer + 16].try_into().unwrap());
    let index_at = bad_index.len() - index_len as usize;
    let mut first_entry = index_at + 4;
    while bad_index[first_entry] >= 128 {
        first_entry += 1;
    }
    first_entry += 1;
    bad_index[first_entry] = 1;
    let mut output = Vec::new();
    let malformed_index = AppendRowsEncoder::new(
        Cursor::new(bad_index.as_slice()),
        &mut output,
        Cursor::new(Vec::new()),
        1,
        &cfg,
        1000,
        DecodePolicy::default(),
    );
    assert!(matches!(
        malformed_index,
        Err(StreamEncodeError::Codec(Error::Corrupt(_)))
    ));
    assert!(output.is_empty());

    let mut output = Vec::new();
    let capped = AppendRowsEncoder::new(
        Cursor::new(original.as_slice()),
        &mut output,
        Cursor::new(Vec::new()),
        1,
        &cfg,
        1000,
        DecodePolicy::default().with_max_input_bytes(original.len() as u64 - 1),
    );
    assert!(matches!(
        capped,
        Err(StreamEncodeError::Codec(Error::TooLarge))
    ));
    assert!(output.is_empty());

    let mut short_info = info;
    short_info.height = 3;
    let short = fixed(short_info, &Metadata::default(), cfg.clone(), &pixels[..21]);
    let mut output = Vec::new();
    let unaligned = AppendRowsEncoder::new(
        Cursor::new(short.as_slice()),
        &mut output,
        Cursor::new(Vec::new()),
        1,
        &cfg,
        1000,
        DecodePolicy::default(),
    );
    assert!(matches!(
        unaligned,
        Err(StreamEncodeError::Codec(Error::InvalidInput(_)))
    ));
    assert!(output.is_empty());
}
