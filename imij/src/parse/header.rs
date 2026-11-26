//! The file header (SPEC 0.1, 1 and 1.0): signature, header
//! versions 2 and 3, the presence mask, the feature mask (0.1) and the header
//! record list. The only reader of the header: slice decoders, the reader,
//! region and plane readers, frame decoders and the writers that edit a
//! finished file all parse it here.

use alloc::vec::Vec;

use crate::{ByteReader, Channels, Cicp, Error, ImageInfo, SampleType, put_varint};

/// `IMI` CR LF 0x87 LF: CR LF catches line-ending conversion, the high-bit
/// byte catches 7-bit transports, and the final LF catches LF to CR LF.
pub(crate) const SIGNATURE: [u8; 8] = [b'I', b'M', b'I', b'J', 0x0D, 0x0A, 0x87, 0x0A];

/// The header version this library writes (SPEC 1.0). The fields below and
/// their defaults are fixed for versions 2 and 3.
pub(crate) const HEADER_VERSION: u8 = 3;
/// The header version written before 2026-10-26, still read: its unit, side,
/// directory and frame record ids translate through the registry (SPEC
/// Appendix V3).
pub(crate) const HEADER_VERSION_2: u8 = crate::registry::LEGACY_HEADER_VERSION;

pub(crate) fn check_header_version(version: u8) -> Result<(), Error> {
    match version {
        4 => Ok(()),
        HEADER_VERSION | HEADER_VERSION_2 if cfg!(feature = "v3") => Ok(()),
        HEADER_VERSION | HEADER_VERSION_2 => {
            Err(Error::Unsupported("enable the v3 feature for V2/V3 images"))
        }
        _ => Err(Error::Unsupported("unknown header version")),
    }
}

/// The header version byte of a file whose first bytes are `prefix`
/// (signature, header length, version).
pub(crate) fn header_version(prefix: &[u8]) -> Result<u8, Error> {
    let mut r = ByteReader::new(prefix);
    if r.bytes(8).map_err(|_| Error::NotImij)? != SIGNATURE {
        return Err(Error::NotImij);
    }
    r.varint()?;
    let version = r.u8()?;
    check_header_version(version)?;
    Ok(if cfg!(feature = "v3") { version } else { 4 })
}
// `present` bits (SPEC 1.0), ordered by how often the reference encoder
// writes the field: a set bit means the field is written; a clear bit means
// it takes its default (the defaults of version 1, frozen). The feature mask
// (bit 2) comes right after `present`; the other fields follow in bit order.
pub(crate) const F_CODING: u64 = 1;
pub(crate) const F_SAMPLE: u64 = 1 << 1;
pub(crate) const F_FEATURES: u64 = 1 << 2;
pub(crate) const F_CHANNELS: u64 = 1 << 3;
pub(crate) const F_ORIENT: u64 = 1 << 4;
pub(crate) const F_SPEED: u64 = 1 << 5;
pub(crate) const F_CICP: u64 = 1 << 6;
pub(crate) const F_BITS: u64 = 1 << 7;
pub(crate) const F_ALPHA: u64 = 1 << 8;
pub(crate) const F_ALL: u64 = 0x1ff;

/// How the `DATA` chunk codes the pixels. The header's `coding` field is
/// open (SPEC 0.3), so the enum is `#[non_exhaustive]`; [`Self::is_lossy`]
/// classifies a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum Coding {
    Lossless = 0,
    Lossy = 1,
}

impl Coding {
    /// Whether decoded pixels may differ from the source (every coding but
    /// [`Coding::Lossless`]).
    pub const fn is_lossy(self) -> bool {
        !matches!(self, Coding::Lossless)
    }
}

/// Decode-complexity tiers (header byte). The encoder records the tier it
/// targeted; decoders may use it to choose a path or refuse files above a
/// budget. Each tier is a fixed bundle of coding choices (see
/// `crate::ImiEncoderConfig::with_decode_speed`). A later version may add a
/// tier, so the enum is `#[non_exhaustive]`; tiers are ordered from
/// smallest files to fastest decoding (compare them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u8)]
#[non_exhaustive]
pub enum DecodeSpeed {
    /// Smallest files: XYB with superblocks below effort `9.0` and full
    /// adaptive superblock + 4x4 partitions with 9-context map DP from effort
    /// `9.0` onward (8-bit images with longer side `>= 128 px`).
    Compact = 0,
    /// Fixed 8x8 XYB blocks with faster entropy decoding for a small size
    /// trade-off (default).
    #[default]
    Balanced = 1,
    /// Fixed 8x8 XYB blocks with static Huffman side information: fastest XYB
    /// entropy decoding.
    Fast = 2,
    /// Fixed 8x8 blocks skipping the XYB color transform (YCoCg): fastest
    /// decoding, largest files at equal quality.
    Fastest = 3,
}

impl DecodeSpeed {
    pub(crate) fn from_u8(v: u8) -> Option<DecodeSpeed> {
        match v {
            0 => Some(DecodeSpeed::Compact),
            1 => Some(DecodeSpeed::Balanced),
            2 => Some(DecodeSpeed::Fast),
            3 => Some(DecodeSpeed::Fastest),
            _ => None,
        }
    }
}

/// A feature (SPEC 0.1): one critical capability a file declares in its
/// header's feature mask, bit `id` for feature `id`, or that its header's
/// own fields imply ([`Feature::Lossy`] by lossy coding,
/// [`Feature::Samples32`] by 32- and 64-bit samples). A decoder that does not
/// implement one rejects the file with [`Error::Unsupported`] right after
/// the header. Ids are frozen once shipped; new features get new ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Feature {
    /// Seed tables from bank 1 in lossy payloads.
    SeedBank1 = crate::registry::FEATURE_SEED_BANK_1 as isize,
    /// Exactly coded 8x8 blocks in lossy payloads.
    ExactBlocks = crate::registry::FEATURE_EXACT_BLOCKS as isize,
    /// i8 and i16 samples.
    SignedSamples = crate::registry::FEATURE_SIGNED_SAMPLES as isize,
    /// u64, i64 and f64 samples.
    WideSamples = crate::registry::FEATURE_WIDE_SAMPLES as isize,
    /// Near-lossless bounds in lossless payloads.
    NearLossless = crate::registry::FEATURE_NEAR_LOSSLESS as isize,
    /// The `FloatShift` channel map (mode 8).
    FloatShift = crate::registry::FEATURE_FLOAT_SHIFT as isize,
    /// Heterogeneous pages (`aNIM` kind 3).
    MixedPages = crate::registry::FEATURE_MIXED_PAGES as isize,
    /// Lossy base-matrix presets.
    DequantPresets = crate::registry::FEATURE_DEQUANT_PRESETS as isize,
    /// Explicit lossy quantization tables (matrix id 63).
    ExplicitTables = crate::registry::FEATURE_EXPLICIT_TABLES as isize,
    /// The lossy reconstruction bias.
    ReconBias = crate::registry::FEATURE_RECON_BIAS as isize,
    /// A signalled lossy color transform.
    ColorTransform = crate::registry::FEATURE_COLOR_TRANSFORM as isize,
    /// The `Offset` channel map (mode 7).
    OffsetMap = crate::registry::FEATURE_OFFSET_MAP as isize,
    /// Lossy superblocks (Compact tier): 16x16, 16x8 and 8x16 DCTs merged
    /// from 8x8 blocks, with a partition map per unit (SPEC 2.9).
    Superblocks = crate::registry::FEATURE_SUPERBLOCKS as isize,
    /// Bilevel stripes: template-context coding of a 1-bit slot.
    Bilevel = crate::registry::FEATURE_BILEVEL as isize,
    /// Lossy channel layouts (lossy head record 27): per-plane sampling,
    /// smaller transforms and the adaptive selector.
    ChannelLayouts = crate::registry::FEATURE_CHANNEL_LAYOUTS as isize,
    /// 32-bit sample coding (u32, i32, f32: the Extended profile's maps,
    /// context shift and 32-bit arithmetic, including F32 and I32 DC).
    /// Implied by a header's 32- or
    /// 64-bit sample type; declared for a lossy file's i32 DC image (head
    /// record 11) and by documents whose later frames or pages need it.
    Samples32 = crate::registry::FEATURE_SAMPLES_32 as isize,
    /// Lossy coding (the Lossy profile). Implied by a header's lossy
    /// `coding`; declared by documents whose later frames or pages are lossy.
    Lossy = crate::registry::FEATURE_LOSSY as isize,
    /// Lossy small transforms: four 4x4 DCTs per color plane and 8x8
    /// parent, selected by the joint superblock/small-transform map
    /// (SPEC 2.11).
    SmallTransforms = crate::registry::FEATURE_SMALL_TRANSFORMS as isize,
    /// Retired native components: v2 only with the `native-jpeg` Cargo feature.
    /// Every v3 header declaring this reserved feature is rejected.
    NativeComponents = crate::registry::FEATURE_NATIVE_COMPONENTS as isize,
    /// Common frame/page items with their own metadata and optional earlier-image reference.
    SequenceItems = crate::registry::FEATURE_SEQUENCE_ITEMS as isize,
    /// Explicit opaque alpha or undefined stored padding.
    AlphaInterpretation = crate::registry::FEATURE_ALPHA_INTERPRETATION as isize,
    /// Native CMYK and Oklab channel identities, without RGB conversion.
    PixelModels = crate::registry::FEATURE_PIXEL_MODELS as isize,
    /// Exact sample-word packets and raw units without entropy models.
    QuickPixels = crate::registry::FEATURE_QUICK_PIXELS as isize,
    /// Exact byte-plane bit packing and optional pixel-packet coding.
    QuickBitSmarts = crate::registry::FEATURE_QUICK_BIT_SMARTS as isize,
    /// Template palette stripes: palette indices coded from equality
    /// contexts of nearby pixels, with typical rows (SPEC 2.14).
    TemplatePalettes = crate::registry::FEATURE_TEMPLATE_PALETTES as isize,
    /// Value-table channel maps: a coded channel's values index a table of
    /// the sample values it uses (map mode 9, SPEC 2.1).
    ValueTables = crate::registry::FEATURE_VALUE_TABLES as isize,
    /// Every main unit record in `DATA` prefixes its payload with its 0-based
    /// main-record ordinal (`varint`), declared by head record 37 (SPEC 2.7).
    UnitOrdinals = crate::registry::FEATURE_UNIT_ORDINALS as isize,
    /// Independent ANS models and restart-state form in each AC unit.
    LocalAnsContexts = crate::registry::FEATURE_LOCAL_ANS as isize,
    /// Full-width seed-bank identifiers and extensible scope masks.
    SeedBankIds = crate::registry::FEATURE_SEED_BANK_IDS as isize,
    /// Scoped image, group and tile coding deltas.
    CodingDefaults = crate::registry::FEATURE_CODING_DEFAULTS as isize,
    /// Typed independent DC and AC probability model references.
    ModelReferences = crate::registry::FEATURE_MODEL_REFERENCES as isize,
    /// Consecutive nonuniform DC stripe coverage.
    DcCoverage = crate::registry::FEATURE_DC_COVERAGE as isize,
    /// Per-lane adaptive pixel color transform in lossless stripes (SPEC 2.4.1).
    AdaptiveColor = crate::registry::FEATURE_ADAPTIVE_COLOR as isize,
}

/// Number of implemented feature ids; higher bits (including reservations)
/// are unsupported until their syntax is implemented.
const FEATURE_COUNT: u32 = 33;
/// Every assigned feature's bit.
const FEATURES_ALL: u64 = (1 << FEATURE_COUNT) - 1;

impl Feature {
    /// Every feature, in id order.
    pub(crate) const ALL: [Feature; FEATURE_COUNT as usize] = [
        Feature::SeedBank1,
        Feature::ExactBlocks,
        Feature::SignedSamples,
        Feature::WideSamples,
        Feature::NearLossless,
        Feature::FloatShift,
        Feature::MixedPages,
        Feature::DequantPresets,
        Feature::ExplicitTables,
        Feature::ReconBias,
        Feature::ColorTransform,
        Feature::OffsetMap,
        Feature::Superblocks,
        Feature::Bilevel,
        Feature::ChannelLayouts,
        Feature::Samples32,
        Feature::Lossy,
        Feature::SmallTransforms,
        Feature::NativeComponents,
        Feature::SequenceItems,
        Feature::AlphaInterpretation,
        Feature::PixelModels,
        Feature::QuickPixels,
        Feature::QuickBitSmarts,
        Feature::TemplatePalettes,
        Feature::ValueTables,
        Feature::UnitOrdinals,
        Feature::LocalAnsContexts,
        Feature::SeedBankIds,
        Feature::CodingDefaults,
        Feature::ModelReferences,
        Feature::DcCoverage,
        Feature::AdaptiveColor,
    ];

    /// The feature id: its bit in the header's feature mask.
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// The registry name (SPEC 0.1), such as `"near_lossless"`.
    pub const fn name(self) -> &'static str {
        match self {
            Feature::SeedBank1 => "seed_bank_1",
            Feature::ExactBlocks => "exact_blocks",
            Feature::SignedSamples => "signed_samples",
            Feature::WideSamples => "wide_samples",
            Feature::NearLossless => "near_lossless",
            Feature::FloatShift => "float_shift",
            Feature::MixedPages => "mixed_pages",
            Feature::DequantPresets => "dequant_presets",
            Feature::ExplicitTables => "explicit_tables",
            Feature::ReconBias => "recon_bias",
            Feature::ColorTransform => "color_transform",
            Feature::OffsetMap => "offset_map",
            Feature::Superblocks => "superblocks",
            Feature::Bilevel => "bilevel",
            Feature::ChannelLayouts => "channel_layouts",
            Feature::Samples32 => "samples_32",
            Feature::Lossy => "lossy",
            Feature::SmallTransforms => "small_transforms",
            Feature::NativeComponents => "native_components",
            Feature::SequenceItems => "sequence_items",
            Feature::AlphaInterpretation => "alpha_interpretation",
            Feature::PixelModels => "pixel_models",
            Feature::QuickPixels => "quick_pixels",
            Feature::QuickBitSmarts => "quick_bit_smarts",
            Feature::TemplatePalettes => "template_palettes",
            Feature::ValueTables => "value_tables",
            Feature::UnitOrdinals => "unit_ordinals",
            Feature::LocalAnsContexts => "local_ans_contexts",
            Feature::SeedBankIds => "seed_bank_ids",
            Feature::CodingDefaults => "coding_defaults",
            Feature::DcCoverage => "dc_coverage",
            Feature::ModelReferences => "model_references",
            Feature::AdaptiveColor => "adaptive_color",
        }
    }

    /// The `Unsupported` message of a decoder that lacks this feature.
    pub(crate) fn unsupported(self) -> Error {
        Error::Unsupported(match self {
            Feature::SeedBank1 => "feature seed_bank_1 (seed tables)",
            Feature::ExactBlocks => "feature exact_blocks",
            Feature::SignedSamples => "feature signed_samples (i8, i16)",
            Feature::WideSamples => "feature wide_samples (64-bit samples, extended tier)",
            Feature::NearLossless => "feature near_lossless",
            Feature::FloatShift => "feature float_shift",
            Feature::MixedPages => "feature mixed_pages",
            Feature::DequantPresets => "feature dequant_presets (Lossy 0.4 dequantization)",
            Feature::ExplicitTables => "feature explicit_tables (Lossy 0.4 dequantization)",
            Feature::ReconBias => "feature recon_bias (Lossy 0.4 dequantization)",
            Feature::ColorTransform => "feature color_transform (Lossy 0.4 color transform)",
            Feature::OffsetMap => "feature offset_map",
            Feature::Superblocks => "feature superblocks (Compact)",
            Feature::Bilevel => "feature bilevel (Bilevel)",
            Feature::ChannelLayouts => "feature channel_layouts (lossy channel layouts)",
            Feature::Samples32 => "feature samples_32 (32-bit coding)",
            Feature::Lossy => "feature lossy (lossy coding)",
            Feature::SmallTransforms => "feature small_transforms (Compact)",
            Feature::NativeComponents => "feature native_components",
            Feature::SequenceItems => "feature sequence_items",
            Feature::AlphaInterpretation => "feature alpha_interpretation",
            Feature::PixelModels => "feature pixel_models",
            Feature::QuickPixels => "feature quick_pixels",
            Feature::QuickBitSmarts => "feature quick_bit_smarts",
            Feature::TemplatePalettes => "feature template_palettes (Bilevel 0.2)",
            Feature::ValueTables => "feature value_tables (Scientific 0.2)",
            Feature::UnitOrdinals => "feature unit_ordinals",
            Feature::LocalAnsContexts => "feature local_ans_contexts (Lossy 0.5)",
            Feature::SeedBankIds => "feature seed_bank_ids (Lossy 0.6)",
            Feature::CodingDefaults => "feature coding_defaults (Lossy 0.7)",
            Feature::DcCoverage => "feature dc_coverage (Lossy 0.7)",
            Feature::ModelReferences => "feature model_references (Lossy 0.7)",
            Feature::AdaptiveColor => "feature adaptive_color",
        })
    }

    /// This feature's bit in a feature mask.
    pub(crate) const fn bit(self) -> u64 {
        1 << (self as u32)
    }
}

/// The features a header's own fields imply (SPEC 0.1): `lossy` for lossy
/// coding, `samples_32` for 32- and 64-bit samples (64-bit samples code two
/// 32-bit words). They join the header's mask in its feature set and cost
/// no mask bit in that header.
pub(crate) fn implied_features(coding: Coding, sample: SampleType) -> Features {
    let mut bits = 0;
    if !matches!(coding, Coding::Lossless) {
        bits |= Feature::Lossy.bit();
    }
    if sample.bytes() >= 4 {
        bits |= Feature::Samples32.bit();
    }
    Features { bits }
}

/// A set of features (SPEC 0.1): what a header declares
/// ([`Header::features`]), what this build decodes ([`supported_features`]),
/// what a profile version comprises.
///
/// The wire mask has no length limit (SPEC 1.0). This crate version knows
/// feature ids below 64 only, so a header with a higher bit set fails with
/// [`Error::Unsupported`] like any unknown feature; [`Self::bits`] and
/// [`Self::from_bits`] carry ids 0 to 63.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Features {
    bits: u64,
}

impl Features {
    /// No features.
    pub const EMPTY: Features = Features { bits: 0 };
    /// Every assigned feature: payloads parsed without a header (tests and
    /// lab tools).
    #[cfg(any(all(test, feature = "encode"), feature = "lab"))]
    pub(crate) const ALL: Features = Features { bits: FEATURES_ALL };

    /// A set from feature-mask bits 0 to 63; `None` when a bit has no
    /// feature in this crate version (a later version may assign it).
    pub const fn from_bits(bits: u64) -> Option<Features> {
        if bits & !FEATURES_ALL != 0 {
            None
        } else {
            Some(Features { bits })
        }
    }

    /// The feature-mask bits 0 to 63 (every id this crate version knows).
    pub const fn bits(self) -> u64 {
        self.bits
    }

    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    pub const fn contains(self, f: Feature) -> bool {
        self.bits & f.bit() != 0
    }

    /// Whether every feature of `other` is in this set: for example whether
    /// [`supported_features`] covers a header's [`Header::features`].
    pub const fn covers(self, other: Features) -> bool {
        other.bits & !self.bits == 0
    }

    #[cfg_attr(not(any(feature = "encode", feature = "std")), allow(dead_code))]
    pub(crate) const fn with(self, f: Feature) -> Features {
        Features {
            bits: self.bits | f.bit(),
        }
    }

    pub(crate) const fn without(self, f: Feature) -> Features {
        Features {
            bits: self.bits & !f.bit(),
        }
    }

    pub(crate) const fn union(self, other: Features) -> Features {
        Features {
            bits: self.bits | other.bits,
        }
    }

    /// The features of this set that are not in `other`.
    #[cfg_attr(not(any(feature = "encode", feature = "std")), allow(dead_code))]
    pub(crate) const fn difference(self, other: Features) -> Features {
        Features {
            bits: self.bits & !other.bits,
        }
    }

    /// The features in the set, in id order.
    pub fn iter(self) -> impl Iterator<Item = Feature> {
        Feature::ALL.into_iter().filter(move |&f| self.contains(f))
    }

    /// Whether the declaration permits gated syntax of feature `f` (SPEC
    /// 0.1: gated syntax in a file that does not declare its feature is
    /// `Corrupt`).
    pub(crate) fn allows(self, f: Feature) -> bool {
        self.contains(f)
    }

    /// The features this build decodes, including optional reconstruction
    /// and sample tiers enabled in Cargo.
    pub(crate) const fn implemented() -> Features {
        let mut bits = FEATURES_ALL;
        if !cfg!(feature = "native-jpeg") {
            bits &= !Feature::NativeComponents.bit();
        }
        if !cfg!(feature = "near-lossless") {
            bits &= !Feature::NearLossless.bit();
        }
        if !cfg!(feature = "extended") {
            bits &= !(Feature::SignedSamples.bit()
                | Feature::Samples32.bit()
                | Feature::WideSamples.bit());
        }
        Features { bits }
    }
}

/// The features this build decodes. `native_components` needs the optional
/// `native-jpeg` Cargo feature and is valid only in historical v2 files; v3
/// rejects it in every build. The `extended` feature enables `signed_samples`,
/// `samples_32`, and `wide_samples`. `near-lossless` enables `near_lossless`.
/// A file whose [`Header::features`] this set does not cover
/// fails with [`Error::Unsupported`] before any payload is read; one whose
/// features and sample types it supports decodes, with every frame and page (SPEC 0.1: the
/// outer header's features cover theirs).
pub fn supported_features() -> Features {
    Features::implemented()
}

/// A decoder feature set with its own pre-release version (SPEC 0.2).
/// Version 2 files carry features (SPEC 0.1), not profile versions: each
/// version names a set of features ([`ProfileVersion::features`]), and
/// [`Header::profiles`] reports the versions a file's features need.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Profile {
    /// Container, metadata and lossless coding of u8, u16 and f16 samples
    /// (0.2 adds i8 and i16): everything a lossless-only decoder implements.
    Lossless = 0,
    /// 32-bit samples (u32, i32, f32) and the offset map; 0.2 adds 64-bit
    /// samples (u64, i64, f64) coded as two 32-bit words.
    Extended = 1,
    /// The lossy payload as specified (0.2 adds seed tables, 0.4 the
    /// dequantization presets, explicit tables, reconstruction bias and
    /// color transform). Its nested DC and alpha images use lossless coding,
    /// so `Lossy` needs `Lossless`; `Fast`/`Fastest` encodes stay within
    /// these two.
    Lossy = 2,
    /// Slower-to-decode, smaller-file lossy features. 0.1: exact blocks;
    /// 0.2 adds superblocks, 0.3 small transforms.
    Compact = 3,
    /// Science features that add decoder complexity. 0.1: near-lossless
    /// lossless payloads and the `FloatShift` float map.
    Scientific = 4,
    /// Template-context coding of low-color images (documents, masks,
    /// maps, screens). 0.1: bilevel stripes; 0.2 adds template palette
    /// stripes.
    Bilevel = 5,
}

/// A profile and its version (`major.minor`; pre-release versions are 0.x).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileVersion {
    pub profile: Profile,
    pub major: u8,
    pub minor: u8,
}

const fn version(profile: Profile, minor: u8) -> ProfileVersion {
    ProfileVersion {
        profile,
        major: 0,
        minor,
    }
}

const fn set(features: &[Feature]) -> Features {
    let mut bits = 0;
    let mut i = 0;
    while i < features.len() {
        bits |= features[i].bit();
        i += 1;
    }
    Features { bits }
}

/// Every profile version, in profile order, and its features (SPEC 0.2).
/// Versions are cumulative. The 0.3 versions named the unit table, which
/// every version 2 decoder reads (the hint, SPEC 2.7): they keep the 0.2
/// sets so claims made against them stay true.
const PROFILE_VERSIONS: [(ProfileVersion, Features); 19] = {
    use Feature::*;
    const LOSSY_0_4: Features = set(&[
        Lossy,
        SeedBank1,
        DequantPresets,
        ExplicitTables,
        ReconBias,
        ColorTransform,
    ]);
    [
        (version(Profile::Lossless, 1), Features::EMPTY),
        (version(Profile::Lossless, 2), set(&[SignedSamples])),
        (version(Profile::Lossless, 3), set(&[SignedSamples])),
        (version(Profile::Extended, 1), set(&[Samples32, OffsetMap])),
        (
            version(Profile::Extended, 2),
            set(&[Samples32, OffsetMap, WideSamples]),
        ),
        (version(Profile::Lossy, 1), set(&[Lossy])),
        (version(Profile::Lossy, 2), set(&[Lossy, SeedBank1])),
        (version(Profile::Lossy, 3), set(&[Lossy, SeedBank1])),
        (version(Profile::Lossy, 4), LOSSY_0_4),
        (version(Profile::Lossy, 5), LOSSY_0_4.with(LocalAnsContexts)),
        (
            version(Profile::Lossy, 6),
            LOSSY_0_4.with(LocalAnsContexts).with(SeedBankIds),
        ),
        (
            version(Profile::Lossy, 7),
            LOSSY_0_4
                .with(LocalAnsContexts)
                .with(SeedBankIds)
                .with(CodingDefaults)
                .with(ModelReferences)
                .with(DcCoverage),
        ),
        (version(Profile::Compact, 1), set(&[ExactBlocks])),
        (
            version(Profile::Compact, 2),
            set(&[ExactBlocks, Superblocks]),
        ),
        (
            version(Profile::Compact, 3),
            set(&[ExactBlocks, Superblocks, SmallTransforms]),
        ),
        (
            version(Profile::Scientific, 1),
            set(&[NearLossless, FloatShift]),
        ),
        (
            version(Profile::Scientific, 2),
            set(&[NearLossless, FloatShift, ValueTables]),
        ),
        (version(Profile::Bilevel, 1), set(&[Bilevel])),
        (
            version(Profile::Bilevel, 2),
            set(&[Bilevel, TemplatePalettes]),
        ),
    ]
};

impl ProfileVersion {
    /// The features a decoder of this version implements (SPEC 0.2);
    /// `None` for a version this crate does not know. `Compact` needs
    /// `Lossy`. `mixed_pages` and `channel_layouts` belong to no profile.
    pub fn features(self) -> Option<Features> {
        PROFILE_VERSIONS
            .iter()
            .find(|(v, _)| *v == self)
            .map(|&(_, f)| f)
    }
}

/// The profile versions this build can decode, in profile order: those
/// whose complete sample and feature contracts this build supports.
///
/// Builds without `extended` omit [`Profile::Extended`] and the signed-sample
/// Lossless versions. Builds without `near-lossless` omit [`Profile::Scientific`]
/// because its complete feature set includes bounded residuals. Header inspection
/// recognizes those profiles in all builds.
pub fn supported_profiles() -> &'static [ProfileVersion] {
    // The derivation, as constants (`supported_profiles_are_derived`).
    #[cfg(feature = "extended")]
    const LIST: &[ProfileVersion] = &[
        version(Profile::Lossless, 1),
        version(Profile::Lossless, 2),
        version(Profile::Lossless, 3),
        version(Profile::Extended, 1),
        version(Profile::Extended, 2),
        version(Profile::Lossy, 1),
        version(Profile::Lossy, 2),
        version(Profile::Lossy, 3),
        version(Profile::Lossy, 4),
        version(Profile::Lossy, 5),
        version(Profile::Lossy, 6),
        version(Profile::Lossy, 7),
        version(Profile::Compact, 1),
        version(Profile::Compact, 2),
        version(Profile::Compact, 3),
        #[cfg(feature = "near-lossless")]
        version(Profile::Scientific, 1),
        #[cfg(feature = "near-lossless")]
        version(Profile::Scientific, 2),
        version(Profile::Bilevel, 1),
        version(Profile::Bilevel, 2),
    ];
    #[cfg(not(feature = "extended"))]
    const LIST: &[ProfileVersion] = &[
        version(Profile::Lossless, 1),
        version(Profile::Lossy, 1),
        version(Profile::Lossy, 2),
        version(Profile::Lossy, 3),
        version(Profile::Lossy, 4),
        version(Profile::Lossy, 5),
        version(Profile::Lossy, 6),
        version(Profile::Lossy, 7),
        version(Profile::Compact, 1),
        version(Profile::Compact, 2),
        version(Profile::Compact, 3),
        #[cfg(feature = "near-lossless")]
        version(Profile::Scientific, 1),
        #[cfg(feature = "near-lossless")]
        version(Profile::Scientific, 2),
        version(Profile::Bilevel, 1),
        version(Profile::Bilevel, 2),
    ];
    LIST
}

/// The profile versions a header's feature set needs (its mask and what its
/// fields imply, [`implied_features`]), in profile order: `Lossless` always,
/// and per other profile the lowest version whose features cover the
/// file's features of that profile.
pub(crate) fn profiles_of(f: Features) -> Vec<ProfileVersion> {
    let needed = |p: Profile| p == Profile::Lossless;
    let mut out = Vec::with_capacity(4);
    for p in [
        Profile::Lossless,
        Profile::Extended,
        Profile::Lossy,
        Profile::Compact,
        Profile::Scientific,
        Profile::Bilevel,
    ] {
        let versions = PROFILE_VERSIONS.iter().filter(|(v, _)| v.profile == p);
        let all = versions
            .clone()
            .fold(Features::EMPTY, |a, &(_, s)| a.union(s));
        let own = Features {
            bits: f.bits & all.bits,
        };
        if !needed(p) && own.is_empty() {
            continue;
        }
        // The last version covers every feature of its profile.
        if let Some(&(v, _)) = versions.clone().find(|(_, s)| s.covers(own)) {
            out.push(v);
        }
    }
    out
}

/// Header record id of the DATA locator (even: ignorable, SPEC 1.0).
pub(crate) const EXT_LOCATOR: u64 = crate::registry::HEADER_LOCATOR;

/// The fixed header of a file. Its fields are read, not built: the type is
/// `#[non_exhaustive]`, so a field a later version adds is not a breaking
/// change.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Header {
    /// The header version byte: 4 for newly encoded files (SPEC_V4), or 2/3
    /// for compatibility files and edits retaining their version. Version 2 unit,
    /// plane directory and frame record ids are translated (SPEC Appendix V3).
    pub version: u8,
    pub info: ImageInfo,
    pub coding: Coding,
    /// The lossy payload's internal color space (`None` for lossless).
    pub lossy_color: Option<crate::LossyColor>,
    /// The decode-complexity tier the encoder targeted.
    pub decode_speed: DecodeSpeed,
    /// EXIF orientation 1..=8 (1 = as stored). Decoders return pixels as
    /// stored; applying the orientation is the caller's choice.
    pub orientation: u8,
    /// Every feature the file needs (SPEC 0.1): its header's feature mask
    /// and the features the fields above imply ([`Feature::Lossy`] for
    /// lossy coding, [`Feature::Samples32`] for 32- and 64-bit samples). In a
    /// file with frames or pages the set covers every frame's and page's. A
    /// file may declare features it does not use; a decoder needs every one:
    /// `supported_features().covers(header.features)` is the whole
    /// admission check.
    pub features: Features,
    /// The profile versions the file's features correspond to, in profile
    /// order (see [`supported_profiles`]).
    pub profiles: Vec<ProfileVersion>,
    /// Reconstructed float sample declarations in stored channel order.
    /// Undeclared and unused channels carry [`crate::FloatGuarantees::NONE`].
    /// A header read exposes the declaration. Use
    /// [`Self::verify_float_guarantees`] to verify reconstructed samples.
    pub float_guarantees: [crate::FloatGuarantees; 4],
}

/// Reads the signature and header.
pub(crate) fn read_header(r: &mut ByteReader<'_>) -> Result<Header, Error> {
    Ok(read_header_sized(r)?.header)
}

/// A parsed header with what the payload parsers need beyond [`Header`].
/// Offsets are positions in the data of the reader the header was read from.
pub(crate) struct ParsedHeader {
    pub header: Header,
    /// The file's feature set: its mask and what its own fields imply
    /// (SPEC 0.1). What the payload may use, and what its frames and pages
    /// may need.
    pub declared: Features,
    /// The header's feature mask as written (without implied features).
    // Retained in the shared parsed representation for wire re-emission.
    #[cfg_attr(not(feature = "encode"), allow(dead_code))]
    pub mask: Features,
    /// Header record length in bytes (after the signature and length varint).
    pub len: usize,
    /// File offset of the first header record (the header record's end when
    /// it has none).
    pub ext_at: usize,
    /// The DATA locator (header record 4) when present and well formed. It
    /// is a claim: readers check it against the chunks (SPEC 1.0).
    #[cfg_attr(not(any(feature = "v3", feature = "std")), allow(dead_code))]
    pub locator: Option<Locator>,
    /// The file checksum record (header record 2, SPEC 1.4), if any.
    pub checksum: Option<ChecksumRecord>,
}

impl ParsedHeader {
    /// The header declares `mixed_pages`: `aNIM` kind 3 may follow.
    pub(crate) fn mixed_pages(&self) -> bool {
        self.declared.contains(Feature::MixedPages)
    }
}

/// A header's file checksum record (record 2).
#[derive(Clone, Copy)]
pub(crate) struct ChecksumRecord {
    pub announced: crate::checksum::Announced,
    /// Offset of the record's payload (its piece size byte).
    pub at: usize,
}

/// Reads a header's feature mask (SPEC 1.0): a little-endian LEB128 bit
/// string of any length (bit `i` = feature `i`) in the minimal encoding, so
/// for masks below 2^64 the bytes of a varint. Returns the low 64 bits and
/// whether a bit at or above 64 is set. The mask ends inside the header
/// record, whose length bounds it; bits above 63 are not stored (a decoder
/// only needs to know that one it does not implement is set).
pub(crate) fn read_feature_mask(r: &mut ByteReader<'_>) -> Result<(u64, bool), Error> {
    let (mut low, mut high) = (0u64, false);
    let mut shift = 0u32;
    loop {
        let b = r.u8()?;
        let group = u64::from(b & 0x7f);
        if shift < 64 {
            low |= group << shift;
            // The group's bits past bit 63 (only the tenth byte has any).
            high |= shift > 57 && group >> (64 - shift) != 0;
        } else {
            high |= group != 0;
        }
        if b < 0x80 {
            if b == 0 && shift > 0 {
                return Err(Error::Corrupt("non-minimal varint"));
            }
            return Ok((low, high));
        }
        shift = shift.saturating_add(7);
    }
}

/// Common framing stops before coding/sample/channel interpretation.
/// The component-mode reader uses this same prefix without making a packed
/// `ImageInfo`; existing readers still admit only their assigned features.
pub(crate) struct HeaderPrefix<'a> {
    pub version: u8,
    pub present: u64,
    pub low: u64,
    pub high: bool,
    pub len: u64,
    pub head_at: usize,
    pub body: ByteReader<'a>,
}

pub(crate) fn read_header_prefix<'a>(
    r: &mut ByteReader<'a>,
    max_header_bytes: u64,
) -> Result<HeaderPrefix<'a>, Error> {
    read_header_prefix_for::<false>(r, max_header_bytes)
}

fn read_header_prefix_for<'a, const V4: bool>(
    r: &mut ByteReader<'a>,
    max_header_bytes: u64,
) -> Result<HeaderPrefix<'a>, Error> {
    #[cfg(all(feature = "lab", feature = "std"))]
    use crate::trace::describe as d;
    use crate::trace::{tf, tfd};
    if tf!("signature", r, r.bytes(8).map_err(|_| Error::NotImij)?) != SIGNATURE {
        return Err(Error::NotImij);
    }
    crate::trace::trace_enter_addr("header", "1.0", "header record", r.mark());
    // The record length stays u64 until it is checked against the bytes
    // present: a 32-bit reader reports the same error as a 64-bit one on a
    // header record that runs past the input.
    let n = tf!("header length", r, r.varint()?);
    if n > max_header_bytes {
        return Err(Error::TooLarge);
    }
    let head_at = r.pos;
    let mut h = ByteReader::new(r.bytes_len(n)?);
    let version = tfd!("header version", h, h.u8()?, |v: &u8| d::header_version(*v));
    if V4 {
        if version != 4 {
            return Err(Error::Unsupported("expected header version 4"));
        }
    } else {
        check_header_version(version)?;
    }
    let present = tfd!("present fields", h, h.varint()?, |v: &u64| d::present(*v));
    // A field this version does not define has no known length: the file
    // needs a newer decoder.
    if present & !F_ALL != 0 {
        return Err(Error::Unsupported("unknown header field"));
    }
    // The feature mask, right after `present` (SPEC 1.0).
    let (low, high) = if present & F_FEATURES != 0 {
        tfd!("features", h, read_feature_mask(&mut h)?, |v: &(
            u64,
            bool
        )| {
            d::features(v.0, v.1)
        })
    } else {
        (0, false)
    };
    Ok(HeaderPrefix {
        version: if V4 || !cfg!(feature = "v3") {
            4
        } else {
            version
        },
        present,
        low,
        high,
        len: n,
        head_at,
        body: h,
    })
}

/// The only packed-header walk: readers and checksum writers share it.
/// Header-only dispatch. Payload consumers still select their own explicit
/// grammar; permitting a V4 declaration never admits V4 bytes to a V2/V3 parser.
pub(crate) fn read_versioned(
    r: &mut ByteReader<'_>,
    max_header_bytes: u64,
) -> Result<ParsedHeader, Error> {
    if !cfg!(feature = "v3") || header_version(&r.data[r.pos..]).ok() == Some(4) {
        read_header_v4(r, max_header_bytes)
    } else {
        read_header_sized(r)
    }
}

pub(crate) fn read_header_sized(r: &mut ByteReader<'_>) -> Result<ParsedHeader, Error> {
    read_header_sized_for::<false>(r, u64::MAX)
}

pub(crate) fn read_header_v4(
    r: &mut ByteReader<'_>,
    max_header_bytes: u64,
) -> Result<ParsedHeader, Error> {
    read_header_sized_for::<true>(r, max_header_bytes)
}

fn read_header_sized_for<const V4: bool>(
    r: &mut ByteReader<'_>,
    max_header_bytes: u64,
) -> Result<ParsedHeader, Error> {
    #[cfg(all(feature = "lab", feature = "std"))]
    use crate::trace::describe as d;
    use crate::trace::{tf, tfd};
    let HeaderPrefix {
        version,
        present,
        low,
        high,
        len: n,
        head_at,
        body: mut h,
    } = if V4 {
        read_header_prefix_for::<true>(r, max_header_bytes)?
    } else {
        read_header_prefix(r, max_header_bytes)?
    };
    if version == 4 && low & (0x3f << 26) != 0 {
        return Err(Error::Corrupt("reserved V4 syntax features"));
    }
    let has = |f: u64| present & f != 0;
    let declared = Features::from_bits(low)
        .filter(|_| !high)
        .ok_or(Error::Unsupported("unknown feature"))?;
    if version >= HEADER_VERSION && declared.contains(Feature::NativeComponents) {
        return Err(Error::Unsupported("native components retired in v3"));
    }
    let coding_byte = if has(F_CODING) {
        tfd!("coding", h, h.u8()?, |v: &u8| d::coding(*v))
    } else {
        0
    };
    let (coding, lossy_color) = match coding_byte {
        0 => (Coding::Lossless, None),
        1 => (Coding::Lossy, Some(crate::LossyColor::Xyb)),
        2 => (Coding::Lossy, Some(crate::LossyColor::YCoCg)),
        _ => return Err(Error::Unsupported("unknown coding")),
    };
    let sample = if has(F_SAMPLE) {
        // A sample type this decoder does not know needs a newer decoder.
        SampleType::from_u8(tfd!("sample type", h, h.u8()?, |v: &u8| d::sample(*v)))
            .ok_or(Error::Unsupported("unknown sample type"))?
    } else {
        SampleType::U8
    };
    let channels = if has(F_CHANNELS) {
        Channels::from_u8(tfd!("channels", h, h.u8()?, |v: &u8| d::channels(*v)))
            .ok_or(Error::Corrupt("bad channels"))?
    } else {
        Channels::Rgb
    };
    if channels == Channels::Scalar && version < 3 {
        return Err(Error::Corrupt("bad channels"));
    }
    if channels.is_native_model() {
        if !declared.contains(Feature::PixelModels) {
            return Err(Error::Corrupt("pixel model needs its feature"));
        }
        if coding != Coding::Lossless {
            return Err(Error::Corrupt("pixel model requires lossless coding"));
        }
    }
    let orientation = if has(F_ORIENT) {
        tfd!("orientation", h, h.u8()?, |v: &u8| d::orientation(*v))
    } else {
        1
    };
    if !(1..=8).contains(&orientation) {
        return Err(Error::Corrupt("bad orientation"));
    }
    let decode_speed = if has(F_SPEED) {
        // Informational: an unknown tier reads as the fastest known one.
        DecodeSpeed::from_u8(tfd!("decode speed", h, h.u8()?, |v: &u8| d::speed(*v)))
            .unwrap_or(DecodeSpeed::Fastest)
    } else {
        DecodeSpeed::Balanced
    };
    let cicp = if has(F_CICP) {
        let c = tfd!(
            "CICP (primaries/transfer/matrix/range)",
            h,
            h.bytes(4)?,
            |c: &&[u8]| d::cicp(c)
        );
        if c[3] > 1 {
            return Err(Error::Corrupt("reserved header bits"));
        }
        Cicp {
            color_primaries: c[0],
            transfer_characteristics: c[1],
            matrix_coefficients: c[2],
            full_range: c[3] != 0,
        }
    } else {
        channels.default_cicp()
    };
    if channels == Channels::Scalar && cicp != Cicp::UNSPECIFIED {
        return Err(Error::Corrupt("CICP on scalar channels"));
    }
    let mut significant_bits = [0; 4];
    if has(F_BITS) {
        for bits in &mut significant_bits[..channels.count()] {
            let n = tf!("significant bits", h, h.u8()?);
            if n == 0 || n as u32 > sample.depth() {
                return Err(Error::Corrupt("invalid significant-bit declaration"));
            }
            *bits = if n as u32 == sample.depth() { 0 } else { n };
        }
    }
    let alpha_mode = if has(F_ALPHA) {
        tfd!("alpha mode", h, h.u8()?, |v: &u8| d::alpha_mode(*v))
    } else {
        0
    };
    if alpha_mode > 3 {
        return Err(Error::Corrupt("reserved header bits"));
    }
    if alpha_mode >= 2 {
        if !declared.contains(Feature::AlphaInterpretation) {
            return Err(Error::Corrupt("alpha mode needs its feature"));
        }
        if !channels.has_alpha() {
            return Err(Error::Corrupt(
                "alpha interpretation needs a stored alpha slot",
            ));
        }
    }
    let width = tf!("width", h, h.varint()?);
    let height = tf!("height", h, h.varint()?);
    if width == 0 || height == 0 || width > u32::MAX as u64 || height > u32::MAX as u64 {
        return Err(Error::Corrupt("bad dimensions"));
    }
    // Header records (SPEC 1, 1.0): (id, length, bytes), ids strictly
    // increasing. Even ids may be skipped; an odd id this decoder does not
    // know changes how the file must be read.
    let mut last: Option<u64> = None;
    let ext_at = head_at + h.pos;
    let mut locator = None;
    let mut checksum = None;
    let mut float_guarantees = crate::float_guarantees::EMPTY;
    while h.remaining() != 0 {
        let id = tf!("record id", h, h.varint()?);
        let len = tf!("record length", h, h.varint()?);
        if last.is_some_and(|l| id <= l) {
            return Err(Error::Corrupt("header records out of order"));
        }
        last = Some(id);
        if id == crate::checksum::EXT_CHECKSUM {
            let at = head_at + h.pos;
            let rec = tf!("file checksum record", h, h.bytes_len(len)?);
            checksum = Some(ChecksumRecord {
                announced: crate::checksum::Announced::parse(rec, version),
                at,
            });
            continue;
        }
        if version == 4 && id == crate::registry::HEADER_FLOAT_GUARANTEES {
            let record = tf!("float sample guarantees", h, h.bytes_len(len)?);
            float_guarantees = crate::float_guarantees::parse(record, sample, channels.count())?;
            continue;
        }
        if id == EXT_LOCATOR {
            locator = Locator::parse(tf!("DATA locator", h, h.bytes_len(len)?));
            continue;
        }
        if id & 1 == 1 {
            return Err(Error::Unsupported("unknown critical header record"));
        }
        tf!("record (ignored)", h, h.bytes_len(len)?);
    }
    // i8/i16 samples need `signed_samples`, 64-bit samples `wide_samples`
    // (SPEC 1.0): gated syntax without its feature.
    if let Some(need) = sample.required_feature()
        && !declared.contains(need)
    {
        return Err(Error::Corrupt("sample type needs its feature"));
    }
    // The file's feature set: the mask and what the fields imply (SPEC 0.1).
    let mask = declared;
    let declared = mask.union(implied_features(coding, sample));
    let info = ImageInfo {
        width: width as u32,
        height: height as u32,
        channels,
        sample,
        cicp,
        alpha_mode: match alpha_mode {
            0 => crate::AlphaMode::Straight,
            1 => crate::AlphaMode::Premultiplied,
            2 => crate::AlphaMode::Opaque,
            3 => crate::AlphaMode::Undefined,
            _ => unreachable!(),
        },
        significant_bits,
    };
    info.validate_precision()
        .map_err(|_| Error::Corrupt("invalid significant-bit declaration"))?;
    // A header is refused only when the image cannot be measured: a size
    // over `u64` is too large everywhere, while a size that fits `u64` but
    // not `usize` is `TooLarge` for the routes that allocate, not for a
    // reader of the header alone (which needs no pixel-sized buffer).
    if info.checked_buffer_len_u64().is_none() {
        return Err(Error::TooLarge);
    }
    #[cfg(all(feature = "lab", feature = "std"))]
    if crate::trace::on() {
        let names = d::channel_names(info.channels).join(", ");
        crate::trace::note(
            "output image",
            alloc::format!(
                "{}x{} {:?} ({names}), {:?} samples: what the decoder delivers. The payload codes this \
                 differently: see the DATA head notes.",
                info.width,
                info.height,
                info.channels,
                info.sample
            ),
        );
        let c = &info.cicp;
        crate::trace::note(
            "color signaling",
            d::cicp(&[
                c.color_primaries,
                c.transfer_characteristics,
                c.matrix_coefficients,
                c.full_range as u8,
            ]),
        );
        crate::trace::note(
            "coding",
            match lossy_color {
                None => alloc::string::String::from("Lossless"),
                Some(c) => alloc::format!("Lossy, internal color {c:?}"),
            },
        );
    }
    crate::trace::exit_addr(h.mark());
    Ok(ParsedHeader {
        header: Header {
            version,
            profiles: profiles_of(declared),
            float_guarantees,
            features: declared,
            info,
            coding,
            lossy_color,
            decode_speed,
            orientation,
        },
        declared,
        mask,
        len: n as usize,
        ext_at,
        locator,
        checksum,
    })
}

/// The DATA locator (header record 4, SPEC 1.0): the whole length of every
/// chunk between the header and DATA, and the bytes from the DATA chunk's
/// tag to the end of its head (where the first unit record starts).
/// Ignorable: a reader uses it only after
/// checking it against the chunks it names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Locator {
    pub prefix: u64,
    pub chunks: Vec<u64>,
}

impl Locator {
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(4 + 3 * self.chunks.len());
        put_varint(&mut b, self.prefix);
        put_varint(&mut b, self.chunks.len() as u64);
        for &c in &self.chunks {
            put_varint(&mut b, c);
        }
        b
    }

    /// `None` when the record does not parse (readers then ignore it).
    pub(crate) fn parse(rec: &[u8]) -> Option<Locator> {
        let mut r = ByteReader::new(rec);
        let prefix = r.varint().ok()?;
        let n = r.var_usize().ok()?;
        // Every length takes at least one byte.
        if n > r.remaining() {
            return None;
        }
        let mut chunks = Vec::new();
        chunks.try_reserve_exact(n).ok()?;
        for _ in 0..n {
            chunks.push(r.varint().ok()?);
        }
        (r.remaining() == 0).then_some(Locator { prefix, chunks })
    }
}

/// What this build cannot decode, rejected from the header (SPEC 0.1)
/// before the chunks or a checksum are read: a feature of the file's set
/// (`declared`: its mask and what its fields imply) that this build does
/// not implement ([`Features::implemented`]), or an extended sample type.
/// F32 and nested I32 DC require `extended`.
pub(crate) fn check_decodable(h: &Header, declared: Features) -> Result<(), Error> {
    h.info.sample.check_decode_support()?;
    match declared
        .iter()
        .find(|&f| !Features::implemented().contains(f))
    {
        Some(f) => Err(f.unsupported()),
        None => Ok(()),
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, EncodeConfig, LossyConfig, Metadata};

    #[test]
    fn feature_ids_and_sets() {
        let ids: Vec<_> = (0..FEATURE_COUNT as usize).collect();
        assert_eq!(Feature::ALL.len(), ids.len());
        for (id, f) in ids.into_iter().zip(Feature::ALL) {
            assert_eq!(f as usize, id);
            assert_eq!(f.bit(), 1 << id);
        }
        assert_eq!(
            Features::from_bits((1 << 17) | (1 << 18)).unwrap(),
            Features::EMPTY
                .with(Feature::SmallTransforms)
                .with(Feature::NativeComponents)
        );
        assert_eq!(
            Features::from_bits(1 << 19),
            Some(Features::EMPTY.with(Feature::SequenceItems))
        );
        assert_eq!(
            Features::from_bits(1 << 23),
            Some(Features::EMPTY.with(Feature::QuickBitSmarts))
        );
        assert_eq!(
            Features::from_bits(1 << 24),
            Some(Features::EMPTY.with(Feature::TemplatePalettes))
        );
        assert_eq!(
            Features::from_bits(1 << 25),
            Some(Features::EMPTY.with(Feature::ValueTables))
        );
        assert_eq!(
            Features::from_bits(1 << 26),
            Some(Features::EMPTY.with(Feature::UnitOrdinals))
        );
        assert_eq!(
            Features::from_bits(1 << 27),
            Some(Features::EMPTY.with(Feature::LocalAnsContexts))
        );
        assert_eq!(
            Features::from_bits(1 << 28),
            Some(Features::EMPTY.with(Feature::SeedBankIds))
        );
        assert_eq!(
            Features::from_bits(1 << 29),
            Some(Features::EMPTY.with(Feature::CodingDefaults))
        );
        assert_eq!(
            Features::from_bits(1 << 30),
            Some(Features::EMPTY.with(Feature::ModelReferences))
        );
        assert_eq!(
            Features::from_bits(1 << 31),
            Some(Features::EMPTY.with(Feature::DcCoverage))
        );
        assert_eq!(
            Features::from_bits(1 << 32),
            Some(Features::EMPTY.with(Feature::AdaptiveColor))
        );
        assert_eq!(Features::from_bits(1 << 33), None);
        assert_eq!(Features::from_bits(u64::MAX), None);
        let f = Features::EMPTY
            .with(Feature::SeedBank1)
            .with(Feature::ReconBias);
        assert_eq!(f.bits(), 1 | 1 << 9);
        assert_eq!(
            f.iter().map(Feature::name).collect::<Vec<_>>(),
            ["seed_bank_1", "recon_bias"]
        );
        assert!(f.allows(Feature::SeedBank1) && !f.allows(Feature::ExactBlocks));
        assert!(f.covers(Features::EMPTY.with(Feature::ReconBias)));
        assert!(!f.covers(Features::EMPTY.with(Feature::ExactBlocks)));
        assert_eq!(f.without(Feature::ReconBias).bits(), 1);
        assert!(Features::ALL.covers(Features::implemented()));
        // The Lossy 0.4 dequantization features and superblocks (Compact) are
        // implemented.
        for g in [
            Feature::DequantPresets,
            Feature::ExplicitTables,
            Feature::ReconBias,
            Feature::ColorTransform,
            Feature::Superblocks,
        ] {
            assert!(Features::implemented().contains(g));
        }
        // Lossy coding and channel layouts are in every build; the
        // extended tier's features only with `extended`.
        for g in [Feature::Lossy, Feature::ChannelLayouts, Feature::OffsetMap] {
            assert!(Features::implemented().contains(g));
        }
        for g in [
            Feature::SignedSamples,
            Feature::Samples32,
            Feature::WideSamples,
        ] {
            assert_eq!(
                Features::implemented().contains(g),
                cfg!(feature = "extended")
            );
        }
        assert_eq!(
            [Feature::ChannelLayouts, Feature::Samples32, Feature::Lossy].map(Feature::name),
            ["channel_layouts", "samples_32", "lossy"]
        );
        assert_eq!(
            implied_features(Coding::Lossy, SampleType::F16),
            Features::EMPTY.with(Feature::Lossy)
        );
        for s in [
            SampleType::U32,
            SampleType::I32,
            SampleType::F32,
            SampleType::F64,
        ] {
            assert_eq!(
                implied_features(Coding::Lossless, s),
                Features::EMPTY.with(Feature::Samples32)
            );
        }
        for s in [SampleType::U8, SampleType::I16, SampleType::F16] {
            assert!(implied_features(Coding::Lossless, s).is_empty());
        }
    }

    #[test]
    fn profiles_follow_the_features_and_fields() {
        // A header's feature set: its mask and what its fields imply.
        let set = |coding, sample, mask: Features| mask.union(implied_features(coding, sample));
        let none = Features::EMPTY;
        assert_eq!(
            profiles_of(set(Coding::Lossless, SampleType::U8, none)),
            [version(Profile::Lossless, 1)]
        );
        assert_eq!(
            profiles_of(set(
                Coding::Lossy,
                SampleType::U32,
                none.with(Feature::SeedBank1)
            )),
            [
                version(Profile::Lossless, 1),
                version(Profile::Extended, 1),
                version(Profile::Lossy, 2)
            ]
        );
        assert_eq!(
            profiles_of(set(
                Coding::Lossy,
                SampleType::U8,
                none.with(Feature::ExactBlocks).with(Feature::FloatShift)
            )),
            [
                version(Profile::Lossless, 1),
                version(Profile::Lossy, 1),
                version(Profile::Compact, 1),
                version(Profile::Scientific, 1)
            ]
        );
        // The lowest version that covers the file's features of a profile.
        assert_eq!(
            profiles_of(set(
                Coding::Lossy,
                SampleType::U8,
                none.with(Feature::SignedSamples).with(Feature::ReconBias)
            )),
            [version(Profile::Lossless, 2), version(Profile::Lossy, 4)]
        );
        assert_eq!(
            profiles_of(set(
                Coding::Lossless,
                SampleType::U64,
                none.with(Feature::WideSamples)
            )),
            [version(Profile::Lossless, 1), version(Profile::Extended, 2)]
        );
        // An 8-bit lossy file with an i32 DC image declares `samples_32`:
        // Extended shows at the header.
        assert_eq!(
            profiles_of(set(
                Coding::Lossy,
                SampleType::U8,
                none.with(Feature::Samples32)
            )),
            [
                version(Profile::Lossless, 1),
                version(Profile::Extended, 1),
                version(Profile::Lossy, 1)
            ]
        );
        // A lossless page one of a document with lossy pages declares
        // `lossy`; `mixed_pages` and `channel_layouts` are in no profile.
        assert_eq!(
            profiles_of(set(
                Coding::Lossless,
                SampleType::U8,
                none.with(Feature::MixedPages).with(Feature::Lossy)
            )),
            [version(Profile::Lossless, 1), version(Profile::Lossy, 1)]
        );
        assert_eq!(
            profiles_of(set(
                Coding::Lossy,
                SampleType::U8,
                none.with(Feature::ChannelLayouts)
            )),
            [version(Profile::Lossless, 1), version(Profile::Lossy, 1)]
        );
    }

    /// The listed constants are the derivation: every version whose
    /// features this build implements (`Extended` needs `samples_32`, so
    /// only with `extended`).
    #[test]
    fn supported_profiles_are_derived() {
        let derived: Vec<_> = PROFILE_VERSIONS
            .iter()
            .filter(|(v, f)| {
                Features::implemented().covers(*f)
                    && (cfg!(feature = "extended") || v.profile != Profile::Extended)
            })
            .map(|&(v, _)| v)
            .collect();
        assert_eq!(supported_profiles(), derived);
        // Supplemental features below remain outside the frozen named versions.
        // Every other feature is in some profile; versions are cumulative.
        let mut all = Features::EMPTY;
        for w in PROFILE_VERSIONS.windows(2) {
            if w[0].0.profile == w[1].0.profile {
                assert!(w[1].1.covers(w[0].1), "{:?}", w[1].0);
            }
            all = all.union(w[0].1).union(w[1].1);
        }
        assert!(!all.contains(Feature::QuickPixels));
        assert!(!all.contains(Feature::QuickBitSmarts));
        assert_eq!(
            all.with(Feature::MixedPages)
                .with(Feature::ChannelLayouts)
                .with(Feature::NativeComponents)
                .with(Feature::SequenceItems)
                .with(Feature::AlphaInterpretation)
                .with(Feature::PixelModels)
                .with(Feature::QuickPixels)
                .with(Feature::QuickBitSmarts)
                .with(Feature::UnitOrdinals)
                .with(Feature::AdaptiveColor),
            Features::from_bits(FEATURES_ALL).unwrap()
        );
        assert_eq!(
            version(Profile::Lossy, 4).features(),
            PROFILE_VERSIONS.get(8).map(|p| p.1)
        );
        assert_eq!(version(Profile::Lossy, 9).features(), None);
    }

    #[test]
    fn locator_round_trips_and_refuses_damage() {
        for l in [
            Locator {
                prefix: 0,
                chunks: vec![],
            },
            Locator {
                prefix: 300,
                chunks: vec![1, 200, 70_000],
            },
        ] {
            let b = l.bytes();
            assert_eq!(Locator::parse(&b), Some(l.clone()));
            // Every cut and every extra byte is ignored as unparsable.
            for n in 0..b.len() {
                assert_eq!(Locator::parse(&b[..n]), None, "{n}");
            }
            assert_eq!(Locator::parse(&[b.as_slice(), &[0]].concat()), None);
        }
        // A count larger than the bytes left.
        assert_eq!(Locator::parse(&[0, 5, 1]), None);
    }

    /// Every truncation of a real header record is an error, every
    /// single-byte change parses or fails without panicking, and the whole
    /// header reads back what the encoder wrote.
    #[test]
    fn header_reader_on_real_headers() {
        let info = ImageInfo::new(7, 5, Channels::Rgba, SampleType::U16);
        let px = vec![7u8; info.row_bytes() * 5];
        let rb = info.row_bytes();
        let files = [
            crate::encode(
                &px,
                rb,
                &info,
                &Metadata::default(),
                &EncodeConfig::default(),
            )
            .unwrap(),
            crate::encode_lossy(
                &px,
                rb,
                &info,
                &Metadata::default(),
                &LossyConfig::new(70.0),
            )
            .unwrap(),
        ];
        for f in &files {
            let ph = read_header_sized(&mut ByteReader::new(f)).unwrap();
            assert_eq!(ph.header.info, info);
            let end = 8 + crate::varint_len(ph.len as u64) + ph.len;
            for n in 0..end {
                assert!(
                    read_header_sized(&mut ByteReader::new(&f[..n])).is_err(),
                    "cut at {n}"
                );
            }
            for i in 0..end {
                for m in [1u8, 0x80] {
                    let mut g = f[..end].to_vec();
                    g[i] ^= m;
                    let _ = read_header_sized(&mut ByteReader::new(&g));
                }
            }
        }
        assert_eq!(
            read_header(&mut ByteReader::new(&files[0])).unwrap().coding,
            Coding::Lossless
        );
        assert_eq!(
            read_header(&mut ByteReader::new(&files[1])).unwrap().coding,
            Coding::Lossy
        );
    }

    /// Header bytes for `present`, `features` (when bit 2 is set), the
    /// fields `fields` and the dimensions.
    fn header_bytes(present: u64, features: u64, fields: &[u8]) -> Vec<u8> {
        let mut mask = Vec::new();
        put_varint(&mut mask, features);
        header_bytes_raw(present, &mask, fields)
    }

    /// [`header_bytes`] with the feature mask's bytes as given.
    fn header_bytes_raw(present: u64, mask: &[u8], fields: &[u8]) -> Vec<u8> {
        let mut body = vec![HEADER_VERSION];
        put_varint(&mut body, present);
        if present & F_FEATURES != 0 {
            body.extend_from_slice(mask);
        }
        body.extend_from_slice(fields);
        body.extend_from_slice(&[3, 2]);
        let mut f = SIGNATURE.to_vec();
        put_varint(&mut f, body.len() as u64);
        f.extend_from_slice(&body);
        f
    }

    /// The feature mask (SPEC 1.0) is an LEB128 bit string of any length:
    /// below 2^64 its bytes are the varint's, a set bit at or above 64 (in
    /// a tenth byte or any later one) is an unknown feature, a zero last
    /// byte is non-minimal, and the header's length bounds it.
    #[test]
    fn feature_mask_has_no_length_limit() {
        let read = |b: &[u8]| {
            let mut r = ByteReader::new(b);
            read_feature_mask(&mut r).map(|v| (v, r.pos))
        };
        let mut values = vec![0u64, 1, u64::MAX, 1 << 63];
        for k in 1..64 {
            values.extend([(1u64 << k) - 1, 1 << k, (1 << k) + 1]);
        }
        for v in values {
            let mut b = Vec::new();
            put_varint(&mut b, v);
            assert_eq!(read(&b), Ok(((v, false), b.len())), "{v:#x}");
        }
        // Bits 64 and up: a tenth byte above 1, an eleventh byte, a long one.
        let ff = |n: usize| vec![0xffu8; n];
        for (bytes, low) in [
            ([ff(9), vec![0x02]].concat(), u64::MAX >> 1),
            ([ff(9), vec![0x7f]].concat(), u64::MAX),
            ([vec![0x80; 10], vec![0x01]].concat(), 0),
            ([vec![0x81], vec![0x80; 2000], vec![0x40]].concat(), 1),
        ] {
            assert_eq!(read(&bytes), Ok(((low, true), bytes.len())));
        }
        let corrupt = |b: &[u8], why| assert_eq!(read(b), Err(Error::Corrupt(why)));
        corrupt(&[0x80, 0x00], "non-minimal varint");
        corrupt(&[vec![0x80; 40], vec![0x00]].concat(), "non-minimal varint");
        corrupt(&[0x80; 40], "truncated");
        // In a header: unknown above 63, however long; the header's end
        // bounds the mask (a continuation past it is truncated).
        let parse = |f: &[u8]| read_header_sized(&mut ByteReader::new(f)).map(|p| p.declared);
        for mask in [
            [ff(9), vec![0x02]].concat(),
            [vec![0x80; 10], vec![0x01]].concat(),
            [vec![0x80; 100_000], vec![0x01]].concat(),
        ] {
            assert_eq!(
                parse(&header_bytes_raw(F_FEATURES, &mask, &[])).err(),
                Some(Error::Unsupported("unknown feature")),
                "{} bytes",
                mask.len()
            );
        }
        let mut f = SIGNATURE.to_vec();
        f.extend_from_slice(&[5, HEADER_VERSION, F_FEATURES as u8, 0x80, 0x80, 0x80]);
        assert_eq!(parse(&f).err(), Some(Error::Corrupt("truncated")));
        f = header_bytes_raw(F_FEATURES, &[0x82, 0x80, 0x00], &[]);
        assert_eq!(parse(&f).err(), Some(Error::Corrupt("non-minimal varint")));
        f = header_bytes_raw(F_FEATURES, &[0x82, 0x00], &[]);
        assert_eq!(parse(&f).err(), Some(Error::Corrupt("non-minimal varint")));
        f = header_bytes_raw(F_FEATURES, &[0x02], &[]);
        assert_eq!(parse(&f), Ok(Features::EMPTY.with(Feature::ExactBlocks)));
    }

    /// The feature mask is read right after `present`; an unassigned bit
    /// needs a newer decoder; i8/i16 and 64-bit samples need their feature;
    /// over-declaring is allowed; an unimplemented feature is refused by
    /// [`check_decodable`], naming it.
    #[test]
    fn feature_mask_rules() {
        let parse = |f: &[u8]| read_header_sized(&mut ByteReader::new(f));
        let ok = parse(&header_bytes(F_FEATURES, 0, &[])).unwrap();
        assert_eq!(ok.declared, Features::EMPTY);
        for bit in [FEATURE_COUNT, 40, 63] {
            assert_eq!(
                parse(&header_bytes(F_FEATURES, 1 << bit, &[])).err(),
                Some(Error::Unsupported("unknown feature"))
            );
        }
        let i16 = SampleType::I16 as u8;
        assert_eq!(
            parse(&header_bytes(F_SAMPLE, 0, &[i16])).err(),
            Some(Error::Corrupt("sample type needs its feature"))
        );
        let signed = Feature::SignedSamples.bit();
        let p = parse(&header_bytes(F_SAMPLE | F_FEATURES, signed, &[i16])).unwrap();
        assert_eq!(p.declared, Features::EMPTY.with(Feature::SignedSamples));
        // Declared but unused.
        let over = Feature::ExactBlocks.bit() | Feature::MixedPages.bit();
        let p = parse(&header_bytes(F_FEATURES, over, &[])).unwrap();
        assert!(p.mixed_pages());
        check_decodable(&p.header, p.declared).unwrap();
        // Every implemented feature is available with `extended`; without
        // it the extended tier's features are refused by name.
        #[cfg(not(feature = "extended"))]
        {
            let p = parse(&header_bytes(F_FEATURES, Feature::SignedSamples.bit(), &[])).unwrap();
            assert_eq!(
                check_decodable(&p.header, p.declared),
                Err(Feature::SignedSamples.unsupported())
            );
            let p = parse(&header_bytes(F_FEATURES, Feature::OffsetMap.bit(), &[])).unwrap();
            check_decodable(&p.header, p.declared).unwrap();
        }
        // The fields imply `lossy` and `samples_32`: in the set, not in the
        // mask; declaring them too is over-declaring, and the set is the same.
        let fields = [1u8, SampleType::F32 as u8];
        let p = parse(&header_bytes(F_CODING | F_SAMPLE, 0, &fields)).unwrap();
        let implied = Features::EMPTY
            .with(Feature::Lossy)
            .with(Feature::Samples32);
        assert_eq!((p.mask, p.declared), (Features::EMPTY, implied));
        assert_eq!(p.header.features, implied);
        let both = implied.bits();
        let q = parse(&header_bytes(
            F_CODING | F_SAMPLE | F_FEATURES,
            both,
            &fields,
        ))
        .unwrap();
        assert_eq!((q.mask, q.declared), (implied, implied));
        // A file with narrow output can still declare 32-bit coding, for
        // example when its DC plane or another image in a sequence needs it.
        let s32 = parse(&header_bytes(F_FEATURES, Feature::Samples32.bit(), &[])).unwrap();
        if cfg!(feature = "extended") {
            check_decodable(&s32.header, s32.declared).unwrap();
            check_decodable(&p.header, p.declared).unwrap();
        } else {
            assert_eq!(
                check_decodable(&s32.header, s32.declared),
                Err(Feature::Samples32.unsupported())
            );
            assert_eq!(
                check_decodable(&p.header, p.declared),
                Err(Error::Unsupported("32-bit samples (extended tier)"))
            );
        }
        // The retired v1 extensions (1: required profiles, 3: mixed-pages
        // capability) are unknown critical records now.
        for id in [1u8, 3] {
            let mut f = header_bytes(0, 0, &[]);
            f.extend_from_slice(&[id, 0]);
            f[8] += 2;
            assert_eq!(
                parse(&f).err(),
                Some(Error::Unsupported("unknown critical header record"))
            );
        }
    }
}
