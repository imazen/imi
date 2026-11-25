//! Independently maintained wire registry, transcribed from SPEC.md.
//! It is not imported from the reference codec. Conformance tests compare
//! both implementations' assignments and the specification.
#![allow(dead_code)]

// feature|seed_bank_1
pub(crate) const FEATURE_SEED_BANK_1: u8 = 0;
// feature|exact_blocks
pub(crate) const FEATURE_EXACT_BLOCKS: u8 = 1;
// feature|signed_samples
pub(crate) const FEATURE_SIGNED_SAMPLES: u8 = 2;
// feature|wide_samples
pub(crate) const FEATURE_WIDE_SAMPLES: u8 = 3;
// feature|near_lossless
pub(crate) const FEATURE_NEAR_LOSSLESS: u8 = 4;
// feature|float_shift
pub(crate) const FEATURE_FLOAT_SHIFT: u8 = 5;
// feature|mixed_pages
pub(crate) const FEATURE_MIXED_PAGES: u8 = 6;
// feature|dequant_presets
pub(crate) const FEATURE_DEQUANT_PRESETS: u8 = 7;
// feature|explicit_tables
pub(crate) const FEATURE_EXPLICIT_TABLES: u8 = 8;
// feature|recon_bias
pub(crate) const FEATURE_RECON_BIAS: u8 = 9;
// feature|color_transform
pub(crate) const FEATURE_COLOR_TRANSFORM: u8 = 10;
// feature|offset_map
pub(crate) const FEATURE_OFFSET_MAP: u8 = 11;
// feature|superblocks
pub(crate) const FEATURE_SUPERBLOCKS: u8 = 12;
// feature|bilevel
pub(crate) const FEATURE_BILEVEL: u8 = 13;
// feature|channel_layouts
pub(crate) const FEATURE_CHANNEL_LAYOUTS: u8 = 14;
// feature|samples_32
pub(crate) const FEATURE_SAMPLES_32: u8 = 15;
// feature|lossy
pub(crate) const FEATURE_LOSSY: u8 = 16;
// feature|small_transforms
pub(crate) const FEATURE_SMALL_TRANSFORMS: u8 = 17;
// feature|native_components
pub(crate) const FEATURE_NATIVE_COMPONENTS: u8 = 18;
// feature|sequence_items
pub(crate) const FEATURE_SEQUENCE_ITEMS: u8 = 19;
// feature|alpha_interpretation
pub(crate) const FEATURE_ALPHA_INTERPRETATION: u8 = 20;
// feature|pixel_models
pub(crate) const FEATURE_PIXEL_MODELS: u8 = 21;
// feature|quick_pixels
pub(crate) const FEATURE_QUICK_PIXELS: u8 = 22;
// feature|quick_bit_smarts
pub(crate) const FEATURE_QUICK_BIT_SMARTS: u8 = 23;
// feature|template_palettes
pub(crate) const FEATURE_TEMPLATE_PALETTES: u8 = 24;
// feature|value_tables
pub(crate) const FEATURE_VALUE_TABLES: u8 = 25;
// feature|unit_ordinals
pub(crate) const FEATURE_UNIT_ORDINALS: u8 = 26;
// feature|local_ans_contexts
pub(crate) const FEATURE_LOCAL_ANS: u8 = 27;
// feature|seed_bank_ids
pub(crate) const FEATURE_SEED_BANK_IDS: u8 = 28;
// feature|coding_defaults
pub(crate) const FEATURE_CODING_DEFAULTS: u8 = 29;
// feature|model_references
pub(crate) const FEATURE_MODEL_REFERENCES: u8 = 30;
// feature|dc_coverage
pub(crate) const FEATURE_DC_COVERAGE: u8 = 31;
// feature|adaptive_color
pub(crate) const FEATURE_ADAPTIVE_COLOR: u8 = 32;
// record|header|file checksum announcement|ignorable|none
pub(crate) const HEADER_CHECKSUM: u64 = 2;
// record|header|DATA locator|ignorable|none
pub(crate) const HEADER_LOCATOR: u64 = 4;
// record|header|V4 float sample guarantees|ignorable|none
pub(crate) const HEADER_FLOAT_GUARANTEES: u64 = 6;
// record|lossless and lossy heads|unit table hint|ignorable|none
pub(crate) const HEAD_UNIT_TABLE: u64 = 10;
// record|lossless and lossy heads|unit table reservation|ignorable|none
pub(crate) const HEAD_UNIT_RESERVE: u64 = 12;
// record|lossy head|exact blocks declaration|critical|exact_blocks
pub(crate) const HEAD_EXACT: u64 = 5;
// record|lossy head|AQ block codes|critical|lossy
pub(crate) const HEAD_AQ: u64 = 7;
// record|lossy head|keep blocks|critical|lossy
pub(crate) const HEAD_KEEP: u64 = 9;
// record|lossy head|i32 DC image|critical|samples_32
pub(crate) const HEAD_DC32: u64 = 11;
// record|lossy head|explicit quantization tables|critical|explicit_tables
pub(crate) const HEAD_EXPLICIT: u64 = 13;
// record|lossy head|reconstruction bias|critical|recon_bias
pub(crate) const HEAD_BIAS: u64 = 15;
// record|lossy head|color transform|critical|color_transform
pub(crate) const HEAD_TRANSFORM: u64 = 17;
// record|lossless head|near-lossless bounds|critical|near_lossless
pub(crate) const HEAD_NEAR: u64 = 19;
// record|lossy head|superblocks declaration|critical|superblocks
pub(crate) const HEAD_SUPERBLOCKS: u64 = 21;
// record|lossy head|joint superblock and small-transform map|critical|small_transforms
pub(crate) const HEAD_SMALL_TRANSFORMS: u64 = 23;
// record|lossless head|bilevel probability table|critical|bilevel
pub(crate) const HEAD_BILEVEL: u64 = 25;
// record|lossless head|template palette tables|critical|template_palettes
pub(crate) const HEAD_TEMPLATE: u64 = 35;
// record|lossless and lossy heads|unit record ordinals|critical|unit_ordinals
pub(crate) const HEAD_UNIT_ORDINALS: u64 = 37;
// record|lossy head|independent AC unit ANS models|critical|local_ans_contexts
pub(crate) const HEAD_LOCAL_ANS: u64 = 39;
// record|lossy head|scoped coding defaults|critical|coding_defaults
pub(crate) const HEAD_CODING_DEFAULTS: u64 = 41;
// record|lossy head|typed independent DC and AC model references|critical|model_references
pub(crate) const HEAD_MODEL_REFERENCES: u64 = 43;
// record|lossy head|consecutive DC coverage|critical|dc_coverage
pub(crate) const HEAD_DC_COVERAGE: u64 = 45;
// record|lossy head|channel layouts|critical|channel_layouts
pub(crate) const HEAD_LAYOUTS: u64 = 27;
// record|native head|retired native component tables and band geometry|critical|native_components
pub(crate) const HEAD_NATIVE_COMPONENTS: u64 = 29;
// record|unit records|retired native component MCU row band|critical|native_components
pub(crate) const UNIT_NATIVE_COMPONENTS: u64 = 61;
// record|frame records|earlier image ordinal and decoded raster hash|critical|sequence_items
pub(crate) const FRAME_REFERENCE: u64 = 111;
// record|unit side records|retired native component band side record|ignorable|none
pub(crate) const UNIT_NATIVE_COMPONENTS_SIDE: u64 = 80;
// record|unit records|DC image stripe|critical|lossy
pub(crate) const UNIT_DC: u64 = 51;
// record|unit records|alpha image stripe|critical|lossy
pub(crate) const UNIT_ALPHA: u64 = 53;
// record|unit records|AC stripe|critical|lossy
pub(crate) const UNIT_AC: u64 = 55;
// record|unit records|exact image stripes|critical|exact_blocks
pub(crate) const UNIT_EXACT: u64 = 57;
// record|unit records|lossless stripe|critical|none
pub(crate) const UNIT_STRIPE: u64 = 59;
// record|V4 DATA only|immutable model definition|critical|V4
pub(crate) const MODEL_DEFINITION: u64 = 63;
// record|V4 DATA only|spatial tile begin|critical|V4
pub(crate) const TILE_BEGIN: u64 = 65;
// record|V4 DATA only|spatial tile end|critical|V4
pub(crate) const TILE_END: u64 = 67;
// record|unit side records|DC side record|ignorable|none
pub(crate) const UNIT_DC_SIDE: u64 = 70;
// record|unit side records|alpha side record|ignorable|none
pub(crate) const UNIT_ALPHA_SIDE: u64 = 72;
// record|unit side records|AC side record|ignorable|none
pub(crate) const UNIT_AC_SIDE: u64 = 74;
// record|unit side records|exact side record|ignorable|none
pub(crate) const UNIT_EXACT_SIDE: u64 = 76;
// record|unit side records|lossless stripe side record|ignorable|none
pub(crate) const UNIT_STRIPE_SIDE: u64 = 78;
// record|plane directory|primary image|critical|none
pub(crate) const DIRECTORY_PRIMARY: u64 = 91;
// record|plane directory|tile data|critical|none
pub(crate) const DIRECTORY_TILES: u64 = 93;
// record|plane directory|plane descriptions|critical|none
pub(crate) const DIRECTORY_PLANES: u64 = 95;
// record|plane directory|attachments|critical|none
pub(crate) const DIRECTORY_ATTACHMENTS: u64 = 97;
// V4-only assignments; this registry does not extend the V2/V3 decoder.
// record|plane directory|paged record authority|critical|none
pub(crate) const DIRECTORY_RECORD_PAGES: u64 = 99;
// record|plane directory|native image views|critical|none
pub(crate) const DIRECTORY_IMAGES: u64 = 101;

/// Header version 2 numbered the unit, side, directory and frame records
/// differently (SPEC Appendix V3): the ids it wrote and their assignments
/// above. An id a version 2 list did not define stays undefined (same
/// parity, matching nothing above).
const V2_UNIT: &[(u64, u64)] = &[
    (1, UNIT_DC),
    (3, UNIT_ALPHA),
    (5, UNIT_AC),
    (7, UNIT_EXACT),
    (9, UNIT_STRIPE),
    (31, UNIT_NATIVE_COMPONENTS),
    (0, UNIT_DC_SIDE),
    (2, UNIT_ALPHA_SIDE),
    (4, UNIT_AC_SIDE),
    (6, UNIT_EXACT_SIDE),
    (8, UNIT_STRIPE_SIDE),
    (30, UNIT_NATIVE_COMPONENTS_SIDE),
];
const V2_FRAME: &[(u64, u64)] = &[(33, FRAME_REFERENCE)];

fn translate(table: &[(u64, u64)], version: u8, id: u64) -> u64 {
    if version != 2 {
        return id;
    }
    table
        .iter()
        .find(|(old, _)| *old == id)
        .map_or(id | (1 << 62), |(_, new)| *new)
}

/// A unit main or side record id read under header `version`.
pub(crate) fn unit_id(version: u8, id: u64) -> u64 {
    translate(V2_UNIT, version, id)
}

/// A frame record id read under header `version`.
pub(crate) fn frame_id(version: u8, id: u64) -> u64 {
    translate(V2_FRAME, version, id)
}
