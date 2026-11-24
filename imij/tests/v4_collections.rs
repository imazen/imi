//! Existing row/feed contracts select the primary image of a collection.
use imij::codec::{DecodeConfig, FeedStatus};
use imij::decode_resources::{DecodePolicy, DecodeSession};
use imij::{
    Channels, FrameConfig, FrameDuration, FrameEncoder, ImageInfo, ImiEncoderConfig, LossyEntropy,
    Metadata, SampleType,
};
use zencodec::decode::DecoderConfig;

fn check_rows(native: &[u8]) {
    let (info, expected) = imij::decode(native).unwrap();
    let file = imij::v4::upgrade_image(native, &Default::default()).unwrap();
    let mut session = DecodeSession::new(DecodePolicy::default());
    let mut row = vec![0xa7; info.row_bytes() + 19];
    let mut next = 0;
    session
        .decode_rows(&file, &mut row, |got, y, pixels| {
            assert_eq!(got, &info);
            assert_eq!(y, next);
            assert_eq!(
                pixels,
                &expected[y as usize * info.row_bytes()..][..info.row_bytes()]
            );
            next += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(next, info.height);
    assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xa7));
    assert_eq!(session.usage().input_bytes, file.len() as u64);
    let mut decoded = Vec::new();
    let mut session = DecodeSession::new(DecodePolicy::default());
    session
        .decode_reader(file.as_slice(), |got, y, row| {
            assert_eq!(got, &info);
            assert_eq!(decoded.len(), y as usize * info.row_bytes());
            decoded.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
    assert_eq!(decoded, expected);
    assert_eq!(session.usage().input_bytes, file.len() as u64);
    let mut feed = DecodeConfig::default().job().feed_decoder(17).unwrap();
    let mut at = 0;
    let mut eof = false;
    let mut decoded = Vec::new();
    loop {
        match feed.advance().unwrap() {
            FeedStatus::NeedInput => {
                if at == file.len() {
                    assert!(!eof, "decoder requested input after EOF");
                    feed.finish_input().unwrap();
                    eof = true;
                } else {
                    let accepted = feed.push(&file[at..]).unwrap();
                    assert!(accepted > 0);
                    at += accepted;
                }
            }
            FeedStatus::Row { y } => {
                assert_eq!(decoded.len(), y as usize * info.row_bytes());
                decoded.extend_from_slice(feed.row().unwrap().unwrap().row(0));
            }
            FeedStatus::Complete => break,
            status => panic!("unexpected feed status: {status:?}"),
        }
    }
    assert_eq!(decoded, expected);
    assert_eq!(feed.usage().input_bytes, file.len() as u64);
}

#[test]
fn collection_primary_rows_and_feed_preserve_coders_late_checksums_and_padding() {
    let info = ImageInfo::new(97, 67, Channels::Rgba, SampleType::U8);
    let source: Vec<u8> = (0..info.checked_buffer_len().unwrap())
        .map(|i| (i * 73 + i / 7 * 41) as u8)
        .collect();
    for coding in [
        ImiEncoderConfig::lossless(),
        ImiEncoderConfig::quick(),
        ImiEncoderConfig::lossy(73.0),
        ImiEncoderConfig::lossy(73.0).tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
    ] {
        let config = FrameConfig::new(coding.with_checksum(true).with_column_groups(Some(32)));
        let mut pages = FrameEncoder::pages(info.clone(), Metadata::default(), config.clone());
        pages.push_page(&source, info.row_bytes(), &info).unwrap();
        pages.push_page(&source, info.row_bytes(), &info).unwrap();
        check_rows(&pages.finish().unwrap());
        let mut mixed = FrameEncoder::mixed_pages(config.clone());
        mixed.push_page(&source, info.row_bytes(), &info).unwrap();
        let small = ImageInfo::new(49, 37, Channels::Gray, SampleType::U8);
        mixed
            .push_page_with(
                &vec![42; small.checked_buffer_len().unwrap()],
                small.row_bytes(),
                &small,
                &Metadata::default(),
                &ImiEncoderConfig::lossless(),
            )
            .unwrap();
        check_rows(&mixed.finish().unwrap());
        let mut animation = FrameEncoder::animation(info.clone(), Metadata::default(), config);
        animation
            .push_frame(&source, info.row_bytes(), FrameDuration::from_millis(33))
            .unwrap();
        let mut changed = source.clone();
        changed[info.row_bytes() * 31 + 47 * 4] ^= 37;
        animation
            .push_frame(&changed, info.row_bytes(), FrameDuration::from_millis(41))
            .unwrap();
        check_rows(&animation.finish().unwrap());
    }
}
