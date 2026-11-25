//! Decode conformance for independently addressed V4 images and views.

use crate::{Channels, Coding, Error, Header, ImageInfo, SampleType};

/// A checked subset of V4 coding for an image or selected view.
///
/// Profiles use the existing wire syntax. Select one with
/// [`crate::decode_resources::DecodePolicy::with_v4_profile`],
/// [`crate::region::RegionReader::open_v4_profile`], or
/// [`crate::planes::PlaneSetLimits::with_v4_profile`]. A profile can only narrow
/// what the compiled reader accepts. Independently accessed views and tiles
/// are checked when read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CodingProfile {
    /// Exact numeric U8, U16 and F16, plus narrow U8/U16 DCT. Gray and RGB
    /// layouts can carry alpha. Every participating coded slot is at most 16
    /// bits, including lossy DC, and groups cannot use temporal references.
    NarrowStill,
    /// RGB8 DCT with ordinary 8x8 transforms, exact biased-U16 RGB DC
    /// and AC scaling through image defaults or group settings.
    Fixed8x8Photo,
    /// Exact numeric images and planes at any supported stored width.
    ExactNumeric,
    /// All V4 syntax supported by the compiled reader.
    Complete,
}

impl CodingProfile {
    /// Stable spelling for catalogs and command-line tools.
    pub const fn name(self) -> &'static str {
        match self {
            Self::NarrowStill => "narrow-still",
            Self::Fixed8x8Photo => "fixed-8x8-photo",
            Self::ExactNumeric => "exact-numeric",
            Self::Complete => "complete-v4",
        }
    }

    pub(crate) fn check_header(self, header: &Header) -> Result<(), Error> {
        if header.version != 4 {
            return Err(Error::Unsupported("V4 coding profile requires version 4"));
        }
        self.check_image(&header.info, header.coding)
    }

    pub(crate) fn check_image(self, info: &ImageInfo, coding: Coding) -> Result<(), Error> {
        let accepted = match self {
            Self::NarrowStill => {
                matches!(
                    info.channels,
                    Channels::Gray | Channels::GrayAlpha | Channels::Rgb | Channels::Rgba
                ) && (matches!(info.sample, SampleType::U8 | SampleType::U16)
                    || (info.sample == SampleType::F16 && coding == Coding::Lossless))
            }
            Self::Fixed8x8Photo => {
                info.channels == Channels::Rgb
                    && info.sample == SampleType::U8
                    && coding == Coding::Lossy
            }
            Self::ExactNumeric => coding == Coding::Lossless,
            Self::Complete => true,
        };
        if accepted {
            Ok(())
        } else {
            Err(Error::Unsupported(
                "image outside requested V4 coding profile",
            ))
        }
    }

    pub(crate) fn check_sequence(self) -> Result<(), Error> {
        if self == Self::Complete {
            Ok(())
        } else {
            Err(Error::Unsupported(
                "sequence outside requested V4 coding profile",
            ))
        }
    }

    pub(crate) fn check_numeric(self, parsed: &crate::v1::Parsed<'_>) -> Result<(), Error> {
        if self != Self::Complete && parsed.layout.near.iter().any(|&bound| bound != 0) {
            return Err(Error::Unsupported(
                "bounded model outside requested V4 coding profile",
            ));
        }
        if matches!(self, Self::NarrowStill | Self::Fixed8x8Photo)
            && parsed.layout.slot_bits.iter().any(|&bits| bits > 16)
        {
            return Err(Error::Unsupported(
                "wide model outside requested V4 coding profile",
            ));
        }
        Ok(())
    }

    pub(crate) fn check_lossy_control(self, control: u8, storage: u64) -> Result<(), Error> {
        let accepted = match self {
            Self::Fixed8x8Photo => control == 0 && storage == 0,
            Self::NarrowStill => control < 3 && storage == 0,
            _ => true,
        };
        if accepted {
            Ok(())
        } else {
            Err(Error::Unsupported(
                "lossy controls outside requested V4 coding profile",
            ))
        }
    }
}

impl core::str::FromStr for CodingProfile {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "narrow-still" => Ok(Self::NarrowStill),
            "fixed-8x8-photo" => Ok(Self::Fixed8x8Photo),
            "exact-numeric" => Ok(Self::ExactNumeric),
            "complete-v4" => Ok(Self::Complete),
            _ => Err(Error::Unsupported("unknown V4 coding profile")),
        }
    }
}

#[cfg(all(test, feature = "std", feature = "encode"))]
mod tests {
    use super::*;
    use crate::decode_resources::{DecodePolicy, DecodeSession};
    use crate::planes::{
        LevelDescriptor, PlaneDescriptor, PlaneSetBuilder, PlaneSetLimits, PlaneSetReader, Rect,
        SliceReader,
    };
    use crate::{ImageRef, ImiEncoderConfig, Limits, LossyEntropy, Metadata};
    use alloc::vec::Vec;

    fn encode(info: &ImageInfo, pixels: &[u8], config: &ImiEncoderConfig) -> Vec<u8> {
        config
            .encode_samples(ImageRef::packed(pixels, info), &Metadata::default())
            .unwrap()
    }

    fn crop(file: &[u8], profile: CodingProfile) -> Result<[u8; 3], Error> {
        let mut reader = crate::region::RegionReader::open_v4_profile(
            SliceReader::new(file),
            &Limits::DEFAULT,
            profile,
        )?;
        let mut out = [0; 3];
        reader.decode_region_threads(Rect::new(0, 0, 1, 1), &mut out, 3, 2)?;
        Ok(out)
    }

    fn decode(file: &[u8], profile: CodingProfile) -> Result<(ImageInfo, Vec<u8>), Error> {
        DecodeSession::new(DecodePolicy::default().with_v4_profile(profile)).decode(file)
    }

    #[test]
    fn fixed_photo_writer_and_adaptive_refusal_across_readers() {
        let info = ImageInfo::new(65, 53, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 53 + i / 9) as u8)
            .collect();
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let preset = ImiEncoderConfig::lossy_fixed_8x8_photo(85.0)
                .with_ac_scale(2.0)
                .tune_lossy(|tuning| tuning.entropy = entropy);
            let file = encode(&info, &pixels, &preset);
            let fixed = CodingProfile::Fixed8x8Photo;
            let (_, expected) = crate::decode(&file).unwrap();
            assert_eq!(decode(&file, fixed).unwrap().1, expected);
            assert_eq!(crop(&file, fixed).unwrap(), expected[..3]);
            let policy = DecodePolicy::default().with_v4_profile(fixed);
            assert_eq!(
                super::super::feed_tests::feed_with_policy(&file, 251, policy).unwrap(),
                (expected, file.len() as u64)
            );
            assert!(matches!(
                decode(&file, CodingProfile::ExactNumeric),
                Err(Error::Unsupported(_))
            ));

            let adaptive = encode(
                &info,
                &pixels,
                &preset.tune_lossy(|tuning| {
                    tuning.channel_layouts[0] = crate::LossyChannelLayout::new(
                        crate::LossyTransform::Dct4,
                        crate::ChannelSampling::Full,
                    );
                }),
            );
            assert_eq!(
                decode(&adaptive, CodingProfile::NarrowStill).unwrap().1,
                crate::decode(&adaptive).unwrap().1
            );
            let refusal = Error::Unsupported("adaptive transforms outside V4 fixed-photo profile");
            assert_eq!(decode(&adaptive, fixed).unwrap_err(), refusal);
            assert_eq!(crop(&adaptive, fixed).unwrap_err(), refusal);
            assert_eq!(
                super::super::feed_tests::feed_with_policy(&adaptive, 251, policy).unwrap_err(),
                refusal
            );
        }
    }

    #[test]
    fn exact_numeric_checks_nested_plane_models() {
        let info = ImageInfo::new(31, 27, Channels::Gray, SampleType::U8);
        let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
            .map(|i| ((i * 13 + i / 7) % 143) as u8)
            .collect();
        let exact = ImiEncoderConfig::lossless();
        let bounded = exact
            .clone()
            .tune_lossless(|tuning| tuning.max_error = [3; 4]);
        let file = encode(&info, &pixels, &exact);
        assert_eq!(decode(&file, CodingProfile::NarrowStill).unwrap().1, pixels);
        assert_eq!(
            decode(&file, CodingProfile::ExactNumeric).unwrap().1,
            pixels
        );
        assert!(matches!(
            decode(&file, CodingProfile::Fixed8x8Photo),
            Err(Error::Unsupported(_))
        ));

        let mut builder = PlaneSetBuilder::new(&file).unwrap();
        let plane = builder
            .add_plane(PlaneDescriptor::new("bounded", SampleType::U8))
            .unwrap();
        builder
            .add_level(
                plane,
                LevelDescriptor::new(info.width, info.height, 16, 16),
                &pixels,
                info.row_bytes(),
                bounded,
            )
            .unwrap();
        let combined = builder.finish().unwrap();
        assert_eq!(
            decode(&combined, CodingProfile::ExactNumeric).unwrap().1,
            pixels
        );
        let rect = Rect::new(0, 0, 8, 7);
        let mut out = [0xa7; 56];
        let limits = PlaneSetLimits::default().with_v4_profile(CodingProfile::ExactNumeric);
        let mut reader = PlaneSetReader::open(SliceReader::new(&combined), limits).unwrap();
        assert_eq!(
            reader.decode_region(plane, 0, rect, &mut out, 8),
            Err(Error::Unsupported(
                "bounded model outside requested V4 coding profile"
            ))
        );
        assert_eq!(out, [0xa7; 56]);
        let limits = limits.with_v4_profile(CodingProfile::Complete);
        let mut reader = PlaneSetReader::open(SliceReader::new(&combined), limits).unwrap();
        reader.decode_region(plane, 0, rect, &mut out, 8).unwrap();
        for (y, row) in out.as_chunks::<8>().0.iter().enumerate() {
            assert!(
                row.iter()
                    .zip(&pixels[y * info.row_bytes()..])
                    .all(|(&a, &b)| a.abs_diff(b) <= 3)
            );
        }
    }

    #[test]
    fn adaptive_color_and_compact_superblocks_respect_profiles() {
        // 1. PR #99 adaptive_color (feature 32) is exact 8-bit lossless RGB:
        //    accepted by NarrowStill, ExactNumeric, and Complete; rejected by Fixed8x8Photo.
        let (w, h) = (160u32, 128u32);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let mut pixels = Vec::with_capacity(info.checked_buffer_len().unwrap());
        for y in 0..h {
            for x in 0..w {
                let g = (((x * 3 + y * 5 + ((x ^ y) & 15)) & 0xFF) as u8)
                    .wrapping_add(((x.wrapping_mul(17) ^ y.wrapping_mul(31)) & 7) as u8);
                let gs = g as i32 - 128;
                let r = (128 + ((48 * gs) >> 5) + ((x + y) & 3) as i32 - 1).clamp(0, 255) as u8;
                let rs = r as i32 - 128;
                let b = (128 + ((8 * gs + 24 * rs) >> 5) + ((x ^ y) & 3) as i32 - 1).clamp(0, 255)
                    as u8;
                pixels.extend_from_slice(&[r, g, b]);
            }
        }
        let adapt_cfg = ImiEncoderConfig::lossless()
            .with_effort(10.0)
            .tune_lossless(|t| {
                t.decor_mask = 1 << 4;
                t.palette = false;
            });
        let adapt_file = encode(&info, &pixels, &adapt_cfg);
        assert!(
            crate::header(&adapt_file)
                .unwrap()
                .features
                .contains(crate::Feature::AdaptiveColor)
        );
        assert_eq!(
            decode(&adapt_file, CodingProfile::NarrowStill).unwrap().1,
            pixels
        );
        assert_eq!(
            decode(&adapt_file, CodingProfile::ExactNumeric).unwrap().1,
            pixels
        );
        assert!(matches!(
            decode(&adapt_file, CodingProfile::Fixed8x8Photo),
            Err(Error::Unsupported(_))
        ));

        // 2. PR #100 DecodeSpeed::Compact at effort >= 9.0 emits superblocks and
        //    4x4 splits: accepted by NarrowStill and Complete, rejected by Fixed8x8Photo.
        let mut mixed = vec![0u8; info.checked_buffer_len().unwrap()];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 3;
                if y < 64 {
                    mixed[i] = (40 + x) as u8;
                    mixed[i + 1] = (90 + y) as u8;
                    mixed[i + 2] = 170;
                } else {
                    let stripe = if ((x + y) / 2) % 2 == 0 { 230 } else { 30 };
                    mixed[i] = stripe;
                    mixed[i + 1] = (stripe ^ 0x24).wrapping_add((x & 7) as u8);
                    mixed[i + 2] = 255 - stripe;
                }
            }
        }
        let compact_cfg = ImiEncoderConfig::lossy(70.0)
            .with_decode_speed(crate::DecodeSpeed::Compact)
            .with_effort(9.0);
        let compact_file = encode(&info, &mixed, &compact_cfg);
        let (_, expected) = crate::decode(&compact_file).unwrap();
        assert_eq!(
            decode(&compact_file, CodingProfile::NarrowStill).unwrap().1,
            expected
        );
        assert_eq!(
            decode(&compact_file, CodingProfile::Complete).unwrap().1,
            expected
        );
        let refusal = Error::Unsupported("adaptive transforms outside V4 fixed-photo profile");
        assert_eq!(
            decode(&compact_file, CodingProfile::Fixed8x8Photo).unwrap_err(),
            refusal
        );
        assert_eq!(
            crop(&compact_file, CodingProfile::Fixed8x8Photo).unwrap_err(),
            refusal
        );
        let fixed_policy = DecodePolicy::default().with_v4_profile(CodingProfile::Fixed8x8Photo);
        assert_eq!(
            super::super::feed_tests::feed_with_policy(&compact_file, 251, fixed_policy)
                .unwrap_err(),
            refusal
        );

        // 3. The fixed-photo preset stays within Fixed8x8Photo even at effort 10.0.
        let fixed_e10 = encode(
            &info,
            &mixed,
            &ImiEncoderConfig::lossy_fixed_8x8_photo(70.0).with_effort(10.0),
        );
        let (_, fixed_px) = crate::decode(&fixed_e10).unwrap();
        assert_eq!(
            decode(&fixed_e10, CodingProfile::Fixed8x8Photo).unwrap().1,
            fixed_px
        );
    }

    #[cfg(feature = "extended")]
    #[test]
    fn numeric_width_and_catalog_names() {
        for profile in [
            CodingProfile::NarrowStill,
            CodingProfile::Fixed8x8Photo,
            CodingProfile::ExactNumeric,
            CodingProfile::Complete,
        ] {
            assert_eq!(profile.name().parse(), Ok(profile));
        }
        let info = ImageInfo::new(9, 1, Channels::Scalar, SampleType::U64);
        let pixels: Vec<u8> = (0..9_u64)
            .flat_map(|i| (u64::MAX - i.pow(3)).to_ne_bytes())
            .collect();
        let file = encode(&info, &pixels, &ImiEncoderConfig::lossless());
        assert_eq!(
            decode(&file, CodingProfile::ExactNumeric).unwrap().1,
            pixels
        );
        assert!(matches!(
            decode(&file, CodingProfile::NarrowStill),
            Err(Error::Unsupported(_))
        ));
    }
}
