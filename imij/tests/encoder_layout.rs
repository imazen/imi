//! Native encoder layout admission precedes full-raster analysis storage.
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

fn encode(pixels: &[u8], stride: usize, info: &ImageInfo) -> Result<Vec<u8>, Error> {
    imij::ImiEncoderConfig::default().encode_samples(
        imij::ImageRef::new(pixels, stride, info),
        &Metadata::default(),
    )
}

#[test]
fn overflowing_stride_extent_is_rejected_before_analysis() {
    let info = ImageInfo::new(1, 3, Channels::Rgb, SampleType::U8);
    for stride in [usize::MAX, usize::MAX / 2, usize::MAX / 2 + 1] {
        assert_eq!(
            encode(&[0; 9], stride, &info),
            Err(Error::InvalidInput("invalid image layout"))
        );
    }
}

#[test]
fn truncated_extent_and_short_stride_are_distinct() {
    let info = ImageInfo::new(3, 2, Channels::Rgb, SampleType::U8);
    assert_eq!(
        encode(&[0; 18], 8, &info),
        Err(Error::InvalidInput("invalid image layout"))
    );
    assert_eq!(
        encode(&[0; 19], 11, &info),
        Err(Error::InvalidInput("pixel buffer too small"))
    );
}

#[test]
fn single_row_does_not_require_trailing_stride_storage() {
    let info = ImageInfo::new(3, 1, Channels::Rgb, SampleType::U8);
    let pixels = [1, 2, 3, 4, 5, 6, 7, 8, 9];
    assert_eq!(
        encode(&pixels, usize::MAX, &info).unwrap(),
        encode(&pixels, 9, &info).unwrap()
    );
}

#[test]
fn padding_and_oversized_stripe_preserve_samples_and_bytes() {
    let info = ImageInfo::new(17, 3, Channels::Rgba, SampleType::U8);
    let packed: Vec<u8> = (0..17 * 3 * 4).map(|i| (i * 37) as u8).collect();
    let mut padded = Vec::new();
    for (y, row) in packed.as_chunks::<68>().0.iter().enumerate() {
        padded.extend_from_slice(row);
        if y != 2 {
            padded.extend_from_slice(&[0xee; 5]);
        }
    }
    for stripe_rows in [1, 3, u32::MAX] {
        let cfg = {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.tune_lossless(|t| {
                t.stripe_rows = Some(stripe_rows);
            });
            cfg
        };
        let a = cfg
            .encode_samples(
                imij::ImageRef::new(&packed, 68, &info),
                &Metadata::default(),
            )
            .unwrap();
        let b = cfg
            .encode_samples(
                imij::ImageRef::new(&padded, 73, &info),
                &Metadata::default(),
            )
            .unwrap();
        assert_eq!(a, b);
        assert_eq!(imij::decode(&a).unwrap().1, packed);
    }
}

#[cfg(target_pointer_width = "32")]
#[test]
fn unrepresentable_row_is_rejected_before_analysis() {
    let info = ImageInfo::new(1 << 31, 1, Channels::Rgb, SampleType::U8);
    assert_eq!(
        encode(&[], usize::MAX, &info),
        Err(Error::InvalidInput("invalid image layout"))
    );
}
