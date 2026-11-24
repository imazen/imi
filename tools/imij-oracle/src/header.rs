//! The versions 2 and 3 container header (SPEC section 1.0).

use crate::bits::Reader;
use crate::error::{Error, Result};

/// A sample type code (SPEC section 1.0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    U8,
    U16,
    F16,
    U32,
    I32,
    F32,
    I8,
    I16,
    U64,
    I64,
    F64,
}

impl SampleType {
    pub(crate) fn from_code(code: u8) -> Result<SampleType> {
        Ok(match code {
            0 => SampleType::U8,
            1 => SampleType::U16,
            2 => SampleType::F16,
            3 => SampleType::U32,
            4 => SampleType::I32,
            5 => SampleType::F32,
            6 => SampleType::I8,
            7 => SampleType::I16,
            8 => SampleType::U64,
            9 => SampleType::I64,
            10 => SampleType::F64,
            other => {
                return Err(Error::unsupported(format!("sample type {other}")));
            }
        })
    }

    /// Storage bits of one full sample (or of a word for 64-bit types).
    pub(crate) fn storage_bits(self) -> u32 {
        match self {
            SampleType::U8 | SampleType::I8 => 8,
            SampleType::U16 | SampleType::F16 | SampleType::I16 => 16,
            SampleType::U32 | SampleType::I32 | SampleType::F32 => 32,
            SampleType::U64 | SampleType::I64 | SampleType::F64 => 64,
        }
    }

    /// Bytes per sample in the output buffer.
    pub(crate) fn bytes(self) -> usize {
        (self.storage_bits() / 8) as usize
    }

    pub(crate) fn is_wide(self) -> bool {
        matches!(self, SampleType::U64 | SampleType::I64 | SampleType::F64)
    }
}

/// One channel map mode (SPEC section 2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MapKind {
    Unsigned,
    Signed,
    Float,
}

/// The order-map family of a coded channel. For 64-bit samples the hi words
/// map like a 32-bit sample of the same kind and the lo words stay raw
/// (SPEC section 2.1).
pub(crate) fn coded_channel_kind(
    sample: SampleType,
    coded_channel: usize,
    channels: u8,
) -> MapKind {
    if sample.is_wide() && coded_channel >= channels as usize {
        return MapKind::Unsigned;
    }
    match sample {
        SampleType::I8 | SampleType::I16 | SampleType::I32 | SampleType::I64 => MapKind::Signed,
        SampleType::F16 | SampleType::F32 | SampleType::F64 => MapKind::Float,
        SampleType::U8 | SampleType::U16 | SampleType::U32 | SampleType::U64 => MapKind::Unsigned,
    }
}

/// The parsed header.
#[derive(Debug, Clone)]
pub struct Header {
    /// The header version byte: 3, or 2 for legacy files,
    /// whose unit, side and frame record ids translate (SPEC Appendix V3).
    pub(crate) version: u8,
    /// The feature mask as written (ids 0-16).
    pub(crate) features: u32,
    /// The feature set (SPEC section 0.1): the mask plus what the fields
    /// imply, `lossy` (16) for lossy coding and `samples_32` (15) for 32-
    /// and 64-bit samples.
    pub(crate) set: u32,
    pub(crate) sample: SampleType,
    pub(crate) sample_code: u8,
    pub(crate) channels: u8,
    pub(crate) channel_layout: u8,
    pub width: u32,
    pub height: u32,
}

impl Header {
    pub(crate) fn storage_bits(&self) -> u32 {
        self.sample.storage_bits()
    }

    pub(crate) fn sample_bytes(&self) -> usize {
        self.sample.bytes()
    }

    /// Coded channel count: 64-bit samples split each channel into two words.
    pub(crate) fn coded_channels(&self) -> usize {
        if self.sample.is_wide() {
            self.channels as usize * 2
        } else {
            self.channels as usize
        }
    }
}

pub(crate) fn parse_header(data: &[u8]) -> Result<Header> {
    if data.len() < 2 {
        return Err(Error::corrupt("short header"));
    }
    let version = data[0];
    if version != 2 && version != 3 {
        return Err(Error::unsupported(format!("header version {version}")));
    }
    let mut r = Reader::new(data);
    r.pos = 1;
    let present = r.varint("present")?;
    if present >> 9 != 0 {
        return Err(Error::unsupported("unknown present bit"));
    }
    let (mut features, mut high) = (0u64, false);
    if present & 4 != 0 {
        (features, high) = r.feature_mask()?;
    }
    let known = ((1u64 << (crate::registry::FEATURE_LOSSY + 1)) - 1)
        | (1u64 << crate::registry::FEATURE_SEQUENCE_ITEMS)
        | (1u64 << crate::registry::FEATURE_ALPHA_INTERPRETATION)
        | (1u64 << crate::registry::FEATURE_PIXEL_MODELS)
        | (1u64 << crate::registry::FEATURE_QUICK_PIXELS)
        | (1u64 << crate::registry::FEATURE_QUICK_BIT_SMARTS)
        | (1u64 << crate::registry::FEATURE_TEMPLATE_PALETTES)
        | (1u64 << crate::registry::FEATURE_VALUE_TABLES)
        | (1u64 << crate::registry::FEATURE_UNIT_ORDINALS)
        | (1u64 << crate::registry::FEATURE_SEED_BANK_IDS)
        | (1u64 << crate::registry::FEATURE_CODING_DEFAULTS)
        | (1u64 << crate::registry::FEATURE_MODEL_REFERENCES)
        | (1u64 << crate::registry::FEATURE_DC_COVERAGE);
    if high || features & !known != 0 {
        return Err(Error::unsupported("unknown feature bit"));
    }
    let features = features as u32;

    let field = |bit: u64, r: &mut Reader<'_>| -> Result<Option<u8>> {
        if present >> bit & 1 == 0 {
            return Ok(None);
        }
        Ok(Some(r.u8("header field")?))
    };

    let coding = field(0, &mut r)?.unwrap_or(0);
    let sample_code = field(1, &mut r)?.unwrap_or(0);
    let channel_layout = field(3, &mut r)?.unwrap_or(3);
    let channels = match channel_layout {
        1..=4 => channel_layout,
        5 | 7 => 4,
        6 => 3,
        _ => return Err(Error::corrupt("bad channel layout")),
    };
    if channel_layout >= 5 {
        if features & (1 << crate::registry::FEATURE_PIXEL_MODELS) == 0 {
            return Err(Error::corrupt("pixel model needs its feature"));
        }
        if coding != 0 {
            return Err(Error::corrupt("pixel model requires lossless coding"));
        }
    }
    let orientation = field(4, &mut r)?.unwrap_or(1);
    let _ = orientation;
    let _decode_speed = field(5, &mut r)?.unwrap_or(1);
    if present & 64 != 0 {
        let cicp = r.take(4, "cicp")?;
        // CICP full_range is a closed field (SPEC 0.3); the primaries,
        // transfer and matrix values are open and read as given.
        if cicp[3] > 1 {
            return Err(Error::corrupt("bad CICP full range"));
        }
    }
    let sample = SampleType::from_code(sample_code)?;
    let storage_bits = sample.storage_bits();
    if present & 128 != 0 {
        for bit in r.take(channels as usize, "sig_bits")? {
            if *bit < 1 || u32::from(*bit) > storage_bits {
                return Err(Error::corrupt("sig_bits out of range"));
            }
        }
    }
    let alpha_mode = field(8, &mut r)?.unwrap_or(0);
    if !(1..=4).contains(&channels) {
        return Err(Error::corrupt("bad channel count"));
    }
    if !(1..=8).contains(&orientation) {
        return Err(Error::corrupt("bad orientation"));
    }
    if alpha_mode > 3 {
        return Err(Error::corrupt("bad alpha mode"));
    }
    if alpha_mode >= 2 {
        if features & (1 << crate::registry::FEATURE_ALPHA_INTERPRETATION) == 0 {
            return Err(Error::corrupt("alpha mode needs its feature"));
        }
        if !matches!(channel_layout, 2 | 4 | 7) {
            return Err(Error::corrupt(
                "alpha interpretation needs a stored alpha slot",
            ));
        }
    }
    let width = r.varint("width")?;
    let height = r.varint("height")?;
    if width < 1 || width > u64::from(u32::MAX) {
        return Err(Error::corrupt("bad width"));
    }
    if height < 1 || height > u64::from(u32::MAX) {
        return Err(Error::corrupt("bad height"));
    }
    if matches!(sample, SampleType::I8 | SampleType::I16)
        && features >> crate::registry::FEATURE_SIGNED_SAMPLES & 1 == 0
    {
        return Err(Error::corrupt("signed sample without its feature"));
    }
    if sample.is_wide() && features >> crate::registry::FEATURE_WIDE_SAMPLES & 1 == 0 {
        return Err(Error::corrupt("wide sample without its feature"));
    }
    let mut set = features;
    if coding != 0 {
        set |= 1 << crate::registry::FEATURE_LOSSY;
    }
    if sample.storage_bits() >= 32 {
        set |= 1 << crate::registry::FEATURE_SAMPLES_32;
    }
    // Header records (SPEC section 1.0): id 2 and 4 are ignorable; any other
    // odd id is unknown and critical.
    let mut previous: Option<u64> = None;
    while r.pos < data.len() {
        let id = r.varint("record id")?;
        let len = r.varint("record len")?;
        if let Some(prev) = previous
            && id <= prev
        {
            return Err(Error::corrupt("header record order"));
        }
        previous = Some(id);
        if id % 2 == 1 {
            return Err(Error::unsupported(format!("header record id {id}")));
        }
        if r.pos + len as usize > data.len() {
            return Err(Error::corrupt("record overruns header"));
        }
        r.pos += len as usize;
    }
    Ok(Header {
        version,
        features,
        set,
        sample,
        sample_code,
        channels,
        channel_layout,
        width: width as u32,
        height: height as u32,
    })
}

/// Fail early on features of the header's set this decoder does not
/// implement (SPEC sections 0.1 and 1.0): the lossy ones. The oracle is a
/// lossless decoder; `lossy` is implied by lossy coding.
pub(crate) fn check_features(header: &Header) -> Result<()> {
    const UNSUPPORTED: [(u8, &str); 13] = [
        (crate::registry::FEATURE_SEED_BANK_1, "feature seed_bank_1"),
        (
            crate::registry::FEATURE_CODING_DEFAULTS,
            "feature coding_defaults",
        ),
        (
            crate::registry::FEATURE_MODEL_REFERENCES,
            "feature model_references",
        ),
        (crate::registry::FEATURE_DC_COVERAGE, "feature dc_coverage"),
        (
            crate::registry::FEATURE_SEED_BANK_IDS,
            "feature seed_bank_ids",
        ),
        (
            crate::registry::FEATURE_EXACT_BLOCKS,
            "feature exact_blocks",
        ),
        (
            crate::registry::FEATURE_DEQUANT_PRESETS,
            "feature dequant_presets",
        ),
        (
            crate::registry::FEATURE_EXPLICIT_TABLES,
            "feature explicit_tables",
        ),
        (crate::registry::FEATURE_RECON_BIAS, "feature recon_bias"),
        (
            crate::registry::FEATURE_COLOR_TRANSFORM,
            "feature color_transform",
        ),
        (crate::registry::FEATURE_SUPERBLOCKS, "feature superblocks"),
        (
            crate::registry::FEATURE_CHANNEL_LAYOUTS,
            "feature channel_layouts",
        ),
        (crate::registry::FEATURE_LOSSY, "feature lossy"),
    ];
    for (bit, name) in UNSUPPORTED {
        if header.set >> bit & 1 != 0 {
            return Err(Error::unsupported(name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_one_is_unsupported() {
        let err = parse_header(&[1, 0, 1, 1]).unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));
    }

    /// The feature mask has no length limit; any bit at or above 64 is an
    /// unknown feature, and the minimal-encoding rule still holds.
    #[test]
    fn feature_mask_of_any_length() {
        let with_mask = |mask: &[u8]| [&[2u8, 4][..], mask, &[37, 19]].concat();
        for mask in [
            [vec![0xFF; 9], vec![0x02]].concat(),
            [vec![0x80; 20], vec![0x01]].concat(),
        ] {
            let err = parse_header(&with_mask(&mask)).unwrap_err();
            assert!(matches!(err, Error::Unsupported(_)), "{err:?}");
        }
        let err = parse_header(&with_mask(&[0x80, 0x80, 0x00])).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "{err:?}");
        let header = parse_header(&with_mask(&[0x04])).unwrap();
        assert_eq!(header.features, 4);
    }

    #[test]
    fn masked_defaults() {
        // 02 00 width height: 8-bit sRGB RGB lossless.
        let header = parse_header(&[2, 0x00, 37, 19]).unwrap();
        assert_eq!(header.width, 37);
        assert_eq!(header.height, 19);
        assert_eq!(header.channels, 3);
        assert_eq!(header.sample, SampleType::U8);
        // Lossless: the set has no `lossy` (16).
        assert_eq!(header.set, 0);
    }
}
