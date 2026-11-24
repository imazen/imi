//! Per-channel guarantees about reconstructed IEEE sample bits.
use crate::{Error, Header, ImageInfo, ImageRef, SampleType};

/// Classes excluded from one stored IEEE floating-point channel.
/// Positive and negative zero are allowed by all three exclusions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FloatGuarantees(u8);

impl FloatGuarantees {
    pub const NONE: Self = Self(0);
    pub const NO_NAN: Self = Self(1);
    pub const NO_INFINITY: Self = Self(2);
    pub const NO_SUBNORMAL: Self = Self(4);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

pub(crate) const EMPTY: [FloatGuarantees; 4] = [FloatGuarantees::NONE; 4];

pub(crate) fn parse(
    body: &[u8],
    sample: SampleType,
    channels: usize,
) -> Result<[FloatGuarantees; 4], Error> {
    let (&version, masks) = body
        .split_first()
        .ok_or(Error::Corrupt("empty V4 float guarantee"))?;
    if version != 1 {
        return Ok(EMPTY);
    }
    if !sample.is_float()
        || masks.len() != channels
        || masks.iter().all(|&mask| mask == 0)
        || masks.iter().any(|&mask| mask & !7 != 0)
    {
        return Err(Error::Corrupt("invalid V4 float guarantee"));
    }
    let mut result = EMPTY;
    for (out, &mask) in result.iter_mut().zip(masks) {
        *out = FloatGuarantees(mask);
    }
    Ok(result)
}

#[cfg(feature = "encode")]
pub(crate) fn check_declaration(
    info: &ImageInfo,
    guarantees: [FloatGuarantees; 4],
) -> Result<(), Error> {
    if guarantees[info.channels.count()..]
        .iter()
        .any(|&mask| mask != FloatGuarantees::NONE)
        || (guarantees != EMPTY && !info.sample.is_float())
    {
        return Err(Error::InvalidInput(
            "float guarantees require stored float channels",
        ));
    }
    Ok(())
}

#[cfg(feature = "encode")]
pub(crate) fn write_header(
    file: alloc::vec::Vec<u8>,
    info: &ImageInfo,
    guarantees: [FloatGuarantees; 4],
) -> Result<alloc::vec::Vec<u8>, Error> {
    if guarantees == EMPTY {
        return Ok(file);
    }
    let mut body = [1; 5];
    for (byte, guarantee) in body[1..].iter_mut().zip(guarantees) {
        *byte = guarantee.0;
    }
    crate::container::with_extension(
        file,
        crate::registry::HEADER_FLOAT_GUARANTEES,
        Some(&body[..1 + info.channels.count()]),
    )
}

fn allowed<const N: usize>(
    row: &[u8],
    guarantees: &[FloatGuarantees],
    exponent: u64,
    fraction: u64,
    bits: impl Fn([u8; N]) -> u64,
) -> bool {
    row.chunks_exact(N * guarantees.len()).all(|pixel| {
        pixel
            .as_chunks::<N>()
            .0
            .iter()
            .zip(guarantees)
            .all(|(word, flags)| {
                if *flags == FloatGuarantees::NONE {
                    return true;
                }
                let bits = bits(*word);
                let exp = bits & exponent;
                let frac = bits & fraction;
                let class = if exp == exponent {
                    if frac == 0 {
                        FloatGuarantees::NO_INFINITY
                    } else {
                        FloatGuarantees::NO_NAN
                    }
                } else if exp == 0 && frac != 0 {
                    FloatGuarantees::NO_SUBNORMAL
                } else {
                    return true;
                };
                !flags.contains(class)
            })
    })
}

/// Check one visible row whose layout has already been admitted.
pub(crate) fn row_matches(row: &[u8], info: &ImageInfo, guarantees: [FloatGuarantees; 4]) -> bool {
    if guarantees == EMPTY {
        return true;
    }
    let flags = &guarantees[..info.channels.count()];
    match info.sample {
        SampleType::F16 => allowed(row, flags, 0x7c00, 0x03ff, |b| u16::from_ne_bytes(b).into()),
        SampleType::F32 => allowed(row, flags, 0x7f80_0000, 0x007f_ffff, |b| {
            u32::from_ne_bytes(b).into()
        }),
        SampleType::F64 => allowed(
            row,
            flags,
            0x7ff0_0000_0000_0000,
            0x000f_ffff_ffff_ffff,
            u64::from_ne_bytes,
        ),
        _ => false,
    }
}

pub(crate) fn verify(image: ImageRef<'_>, guarantees: [FloatGuarantees; 4]) -> Result<(), Error> {
    if guarantees == EMPTY {
        return Ok(());
    }
    let required = image
        .info
        .checked_stride_len(image.stride)
        .ok_or(Error::InvalidInput("float guarantee raster layout"))?;
    if image.pixels.len() < required {
        return Err(Error::InvalidInput("float guarantee raster extent"));
    }
    let row = image.info.row_bytes();
    if row == 0 {
        return Err(Error::InvalidInput("float guarantee empty row"));
    }
    for pixels in image.pixels[..required].chunks(image.stride) {
        if !row_matches(&pixels[..row], image.info, guarantees) {
            return Err(Error::Corrupt("V4 float guarantee violated"));
        }
    }
    Ok(())
}

#[cfg(feature = "encode")]
pub(crate) fn verify_source(
    image: ImageRef<'_>,
    guarantees: [FloatGuarantees; 4],
) -> Result<(), Error> {
    verify(image, guarantees).map_err(|error| match error {
        Error::Corrupt(_) => Error::InvalidInput("source violates float guarantee"),
        other => other,
    })
}

impl Header {
    /// Verify the declared guarantees on a decoded raster, region, or group.
    /// `image` must use the stored sample type, channel layout and native-endian
    /// representation. Padding is ignored. Success covers only these samples.
    pub fn verify_float_guarantees(&self, image: ImageRef<'_>) -> Result<(), Error> {
        if self.info.sample != image.info.sample || self.info.channels != image.info.channels {
            return Err(Error::InvalidInput("float guarantee sample layout"));
        }
        verify(image, self.float_guarantees)
    }
}

#[cfg(all(test, feature = "encode", feature = "std", feature = "extended"))]
mod tests {
    use super::*;
    use crate::planes::{Rect, SliceReader};
    use crate::{Channels, ImiEncoderConfig, Metadata, RowGroups};
    use alloc::vec;

    fn flags(first: FloatGuarantees, second: FloatGuarantees) -> [FloatGuarantees; 4] {
        [first, second, FloatGuarantees::NONE, FloatGuarantees::NONE]
    }

    fn put(word: u64, sample: SampleType, out: &mut [u8]) {
        match sample {
            SampleType::F16 => out.copy_from_slice(&(word as u16).to_ne_bytes()),
            SampleType::F32 => out.copy_from_slice(&(word as u32).to_ne_bytes()),
            SampleType::F64 => out.copy_from_slice(&word.to_ne_bytes()),
            _ => unreachable!(),
        }
    }

    #[test]
    fn checked_writer_retains_bits_and_declares_each_channel_in_the_header() {
        use FloatGuarantees as F;
        for (sample, neg_zero, subnormal, inf, nan) in [
            (SampleType::F16, 0x8000, 0x8001, 0x7c00, 0x7d01),
            (
                SampleType::F32,
                0x8000_0000,
                0x8000_0001,
                0x7f80_0000,
                0x7f80_0001,
            ),
            (
                SampleType::F64,
                1 << 63,
                (1 << 63) | 1,
                0x7ff0_0000_0000_0000,
                0x7ff0_0000_0000_0001,
            ),
        ] {
            let info = ImageInfo::new(3, 2, Channels::GrayAlpha, sample);
            let size = sample.bytes();
            let stride = info.row_bytes() + 3;
            let mut pixels = vec![0xff; stride + info.row_bytes()];
            // Each channel contains a class excluded in the other channel.
            let words = [inf | neg_zero, nan, subnormal, inf, neg_zero, 0];
            for row in pixels.chunks_mut(stride) {
                for (&word, out) in words.iter().zip(row.chunks_exact_mut(size)) {
                    put(word, sample, out);
                }
            }
            let image = ImageRef::new(&pixels, stride, &info);
            let config = ImiEncoderConfig::lossless().with_checksum(true);
            let guaranteed = flags(F::NO_NAN, F::NO_SUBNORMAL);
            let file = config
                .encode_samples_with_float_guarantees(image, &Metadata::default(), guaranteed)
                .unwrap();
            let head = crate::container::data_at(&file).unwrap();
            let header = crate::header(&file[..head]).unwrap();
            assert_eq!(header.float_guarantees, guaranteed);
            assert!(header.float_guarantees[0].contains(F::NO_NAN));
            assert!(!header.float_guarantees[0].contains(F::NO_INFINITY));
            let (decoded_info, decoded) = crate::decode(&file).unwrap();
            let expected = pixels
                .chunks(stride)
                .flat_map(|row| row[..info.row_bytes()].iter().copied())
                .collect::<alloc::vec::Vec<_>>();
            assert_eq!(decoded, expected);
            header
                .verify_float_guarantees(ImageRef::packed(&decoded, &decoded_info))
                .unwrap();
            let mut scratch = std::io::Cursor::new(alloc::vec::Vec::new());
            crate::validation::validate(
                &mut SliceReader::new(&file),
                &mut scratch,
                &Default::default(),
            )
            .unwrap();

            for invalid in [
                flags(F::NO_INFINITY, F::NONE),
                flags(F::NO_SUBNORMAL, F::NONE),
                flags(F::NONE, F::NO_NAN),
            ] {
                assert_eq!(
                    config.encode_samples_with_float_guarantees(
                        image,
                        &Metadata::default(),
                        invalid
                    ),
                    Err(Error::InvalidInput("source violates float guarantee"))
                );
            }
        }
    }

    #[test]
    fn checked_crops_are_local_and_strict_validation_rejects_a_false_claim() {
        let info = ImageInfo::new(16, 8, Channels::Scalar, SampleType::F32);
        let mut pixels = vec![0; info.row_bytes() * 8];
        pixels[10 * 4..11 * 4].copy_from_slice(&0x7fc0_0123u32.to_ne_bytes());
        let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(8, 8, 8 << 20));
        let file = cfg
            .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())
            .unwrap();
        assert_eq!(crate::header(&file).unwrap().float_guarantees, EMPTY);
        let file = write_header(
            file,
            &info,
            flags(FloatGuarantees::NO_NAN, FloatGuarantees::NONE),
        )
        .unwrap();
        let mut reader = crate::region::RegionReader::open(SliceReader::new(&file)).unwrap();
        let mut out = [0xa7; 8 * 34];
        reader
            .decode_region_with_float_guarantees(Rect::new(0, 0, 8, 8), &mut out, 34, 2)
            .unwrap();
        for row in out.as_chunks::<34>().0 {
            assert_eq!(&row[..32], &[0; 32]);
            assert_eq!(&row[32..], &[0xa7; 2]);
        }
        let bad = Rect::new(8, 0, 8, 8);
        assert_eq!(
            reader.decode_region_with_float_guarantees(bad, &mut out, 34, 2),
            Err(Error::Corrupt("V4 float guarantee violated"))
        );
        let mut scratch = std::io::Cursor::new(alloc::vec::Vec::new());
        assert_eq!(
            crate::validation::validate(
                &mut SliceReader::new(&file),
                &mut scratch,
                &Default::default()
            ),
            Err(Error::Corrupt("V4 float guarantee violated"))
        );
    }

    #[test]
    fn forward_writer_retries_a_rejected_group_without_publishing_it() {
        use FloatGuarantees as F;
        let info = ImageInfo::new(8, 8, Channels::Scalar, SampleType::F16);
        let cfg = ImiEncoderConfig::lossless().with_row_groups(RowGroups::new(8, 4, 8 << 20));
        let declared = flags(F::NO_SUBNORMAL.union(F::NO_NAN), F::NONE);
        let mut writer = crate::v4::GroupEncodeSession::new_with_float_guarantees(
            info.clone(),
            &Metadata::default(),
            &cfg,
            declared,
        )
        .unwrap();
        assert_eq!(
            crate::header(writer.output()).unwrap().float_guarantees,
            declared
        );
        let mut encoded = writer.output().to_vec();
        writer.consume_output(encoded.len()).unwrap();
        let next = writer.next_group().unwrap();
        let mut samples = [0; 64];
        samples[..2].copy_from_slice(&1u16.to_ne_bytes());
        assert_eq!(
            writer.push_group(&samples, 16),
            Err(Error::InvalidInput("source violates float guarantee"))
        );
        assert!(writer.output().is_empty());
        assert_eq!(writer.next_group(), Some(next));
        // An infinity is allowed when only NaNs and subnormals are excluded.
        samples[..2].copy_from_slice(&0x7c00u16.to_ne_bytes());
        for _ in 0..2 {
            writer.push_group(&samples, 16).unwrap();
            encoded.extend_from_slice(writer.output());
            writer.consume_output(writer.output().len()).unwrap();
        }
        writer.finish().unwrap();
        let (_, pixels) = crate::decode(&encoded).unwrap();
        assert_eq!(pixels, samples.repeat(2));
    }

    #[test]
    fn malformed_v1_guarantees_are_rejected_and_future_versions_are_ignored() {
        for body in [&[][..], &[1], &[1, 0], &[1, 8], &[1, 1, 1]] {
            assert!(matches!(
                parse(body, SampleType::F16, 1),
                Err(Error::Corrupt(_))
            ));
        }
        assert!(parse(&[1, 1], SampleType::U16, 1).is_err());
        assert_eq!(parse(&[2, 128], SampleType::F16, 1).unwrap(), EMPTY);
    }
}
