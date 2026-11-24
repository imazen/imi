//! Numeric wire assignments (SPEC 0.1 and the record registry in section 1).
//! Feature bits have their own namespace. Record ids share one namespace
//! across every list, in ranges that leave room per category: 1-49 header
//! and payload head records, 50-69 unit main and V4 DATA records, 70-89 unit side
//! records, 90-109 plane directory entries, 110-127 frame records (all one
//! varint byte). New assignments must be added here before use; duplicates
//! and parity mistakes fail to compile. Header version 2 numbered the unit,
//! side, directory and frame records differently: [`unit_id`],
//! [`directory_id`] and [`frame_id`] map a record id read under a header
//! version to its assignment here (SPEC Appendix V3).

#![allow(dead_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Record {
    pub name: &'static str,
    pub id: u64,
    pub lists: &'static str,
    pub meaning: &'static str,
    pub class: &'static str,
    pub feature: &'static str,
}

macro_rules! feature_ids {
    ($($name:ident = $id:literal, $wire:literal;)*) => {
        $(pub(crate) const $name: u8 = $id;)*
        pub(crate) const FEATURES: &[(&str, u8)] = &[$(($wire, $name)),*];
        pub(crate) const FEATURE_IDS: &[(&str, u8)] = &[$((stringify!($name), $name)),*];
    };
}

macro_rules! record_ids {
    ($($name:ident = $id:literal, $lists:literal, $meaning:literal, $class:literal, $feature:literal;)*) => {
        $(pub(crate) const $name: u64 = $id;)*
        pub(crate) const RECORDS: &[Record] = &[$(Record {
            name: stringify!($name), id: $name, lists: $lists,
            meaning: $meaning, class: $class, feature: $feature,
        }),*];
    };
}

feature_ids! {
    FEATURE_SEED_BANK_1 = 0, "seed_bank_1";
    FEATURE_EXACT_BLOCKS = 1, "exact_blocks";
    FEATURE_SIGNED_SAMPLES = 2, "signed_samples";
    FEATURE_WIDE_SAMPLES = 3, "wide_samples";
    FEATURE_NEAR_LOSSLESS = 4, "near_lossless";
    FEATURE_FLOAT_SHIFT = 5, "float_shift";
    FEATURE_MIXED_PAGES = 6, "mixed_pages";
    FEATURE_DEQUANT_PRESETS = 7, "dequant_presets";
    FEATURE_EXPLICIT_TABLES = 8, "explicit_tables";
    FEATURE_RECON_BIAS = 9, "recon_bias";
    FEATURE_COLOR_TRANSFORM = 10, "color_transform";
    FEATURE_OFFSET_MAP = 11, "offset_map";
    FEATURE_SUPERBLOCKS = 12, "superblocks";
    FEATURE_BILEVEL = 13, "bilevel";
    FEATURE_CHANNEL_LAYOUTS = 14, "channel_layouts";
    FEATURE_SAMPLES_32 = 15, "samples_32";
    FEATURE_LOSSY = 16, "lossy";
    FEATURE_SMALL_TRANSFORMS = 17, "small_transforms";
    FEATURE_NATIVE_COMPONENTS = 18, "native_components";
    FEATURE_SEQUENCE_ITEMS = 19, "sequence_items";
    FEATURE_ALPHA_INTERPRETATION = 20, "alpha_interpretation";
    FEATURE_PIXEL_MODELS = 21, "pixel_models";
    FEATURE_QUICK_PIXELS = 22, "quick_pixels";
    FEATURE_QUICK_BIT_SMARTS = 23, "quick_bit_smarts";
    FEATURE_TEMPLATE_PALETTES = 24, "template_palettes";
    FEATURE_VALUE_TABLES = 25, "value_tables";
    FEATURE_UNIT_ORDINALS = 26, "unit_ordinals";
    FEATURE_LOCAL_ANS = 27, "local_ans_contexts";
    FEATURE_SEED_BANK_IDS = 28, "seed_bank_ids";
    FEATURE_CODING_DEFAULTS = 29, "coding_defaults";
    FEATURE_MODEL_REFERENCES = 30, "model_references";
    FEATURE_DC_COVERAGE = 31, "dc_coverage";
}

record_ids! {
    HEADER_CHECKSUM = 2, "header", "file checksum announcement", "ignorable", "none";
    HEADER_LOCATOR = 4, "header", "DATA locator", "ignorable", "none";
    HEADER_FLOAT_GUARANTEES = 6, "header", "V4 float sample guarantees", "ignorable", "none";
    HEAD_UNIT_TABLE = 10, "lossless and lossy heads", "unit table hint", "ignorable", "none";
    HEAD_UNIT_RESERVE = 12, "lossless and lossy heads", "unit table reservation", "ignorable", "none";
    HEAD_EXACT = 5, "lossy head", "exact blocks declaration", "critical", "exact_blocks";
    HEAD_AQ = 7, "lossy head", "AQ block codes", "critical", "lossy";
    HEAD_KEEP = 9, "lossy head", "keep blocks", "critical", "lossy";
    HEAD_DC32 = 11, "lossy head", "i32 DC image", "critical", "samples_32";
    HEAD_EXPLICIT = 13, "lossy head", "explicit quantization tables", "critical", "explicit_tables";
    HEAD_BIAS = 15, "lossy head", "reconstruction bias", "critical", "recon_bias";
    HEAD_TRANSFORM = 17, "lossy head", "color transform", "critical", "color_transform";
    HEAD_NEAR = 19, "lossless head", "near-lossless bounds", "critical", "near_lossless";
    HEAD_SUPERBLOCKS = 21, "lossy head", "superblocks declaration", "critical", "superblocks";
    HEAD_SMALL_TRANSFORMS = 23, "lossy head", "joint superblock and small-transform map", "critical", "small_transforms";
    HEAD_BILEVEL = 25, "lossless head", "bilevel probability table", "critical", "bilevel";
    HEAD_TEMPLATE = 35, "lossless head", "template palette tables", "critical", "template_palettes";
    HEAD_UNIT_ORDINALS = 37, "lossless and lossy heads", "unit record ordinals", "critical", "unit_ordinals";
    HEAD_LOCAL_ANS = 39, "lossy head", "independent AC unit ANS models", "critical", "local_ans_contexts";
    HEAD_CODING_DEFAULTS = 41, "lossy head", "scoped coding defaults", "critical", "coding_defaults";
    HEAD_MODEL_REFERENCES = 43, "lossy head", "typed independent DC and AC model references", "critical", "model_references";
    HEAD_DC_COVERAGE = 45, "lossy head", "consecutive DC coverage", "critical", "dc_coverage";
    HEAD_LAYOUTS = 27, "lossy head", "channel layouts", "critical", "channel_layouts";
    HEAD_NATIVE_COMPONENTS = 29, "native head", "retired native component tables and band geometry", "critical", "native_components";
    UNIT_NATIVE_COMPONENTS = 61, "unit records", "retired native component MCU row band", "critical", "native_components";
    FRAME_REFERENCE = 111, "frame records", "earlier image ordinal and decoded raster hash", "critical", "sequence_items";
    UNIT_NATIVE_COMPONENTS_SIDE = 80, "unit side records", "retired native component band side record", "ignorable", "none";
    UNIT_DC = 51, "unit records", "DC image stripe", "critical", "lossy";
    UNIT_ALPHA = 53, "unit records", "alpha image stripe", "critical", "lossy";
    UNIT_AC = 55, "unit records", "AC stripe", "critical", "lossy";
    UNIT_EXACT = 57, "unit records", "exact image stripes", "critical", "exact_blocks";
    UNIT_STRIPE = 59, "unit records", "lossless stripe", "critical", "none";
    MODEL_DEFINITION = 63, "V4 DATA only", "immutable model definition", "critical", "V4";
    TILE_BEGIN = 65, "V4 DATA only", "spatial tile begin", "critical", "V4";
    TILE_END = 67, "V4 DATA only", "spatial tile end", "critical", "V4";
    UNIT_DC_SIDE = 70, "unit side records", "DC side record", "ignorable", "none";
    UNIT_ALPHA_SIDE = 72, "unit side records", "alpha side record", "ignorable", "none";
    UNIT_AC_SIDE = 74, "unit side records", "AC side record", "ignorable", "none";
    UNIT_EXACT_SIDE = 76, "unit side records", "exact side record", "ignorable", "none";
    UNIT_STRIPE_SIDE = 78, "unit side records", "lossless stripe side record", "ignorable", "none";
    DIRECTORY_PRIMARY = 91, "plane directory", "primary image", "critical", "none";
    DIRECTORY_TILES = 93, "plane directory", "tile data", "critical", "none";
    DIRECTORY_PLANES = 95, "plane directory", "plane descriptions", "critical", "none";
    DIRECTORY_ATTACHMENTS = 97, "plane directory", "attachments", "critical", "none";
    DIRECTORY_RECORD_PAGES = 99, "plane directory", "paged record authority", "critical", "none";
    DIRECTORY_IMAGES = 101, "plane directory", "native image views", "critical", "none";
}

/// Header version 2 (SPEC Appendix V3): the ids it used for the records
/// that moved, with their assignments above.
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
const V2_DIRECTORY: &[(u64, u64)] = &[
    (1, DIRECTORY_PRIMARY),
    (3, DIRECTORY_TILES),
    (5, DIRECTORY_PLANES),
    (7, DIRECTORY_ATTACHMENTS),
];
const V2_FRAME: &[(u64, u64)] = &[(33, FRAME_REFERENCE)];

/// The header version whose record ids the tables above translate.
pub(crate) const LEGACY_HEADER_VERSION: u8 = 2;

/// Marks an id a version 2 list did not define: it keeps its parity (so an
/// even one is still skipped and an odd one still `Unsupported`) and matches
/// no assignment above, so a version 3 id in a version 2 list is not
/// accepted by accident.
const UNDEFINED: u64 = 1 << 62;

const fn translate(table: &[(u64, u64)], version: u8, id: u64) -> u64 {
    if version != LEGACY_HEADER_VERSION {
        return id;
    }
    let mut i = 0;
    while i < table.len() {
        if table[i].0 == id {
            return table[i].1;
        }
        i += 1;
    }
    id | UNDEFINED
}

/// A unit main or side record id read under header `version`, as assigned here.
pub(crate) const fn unit_id(version: u8, id: u64) -> u64 {
    translate(V2_UNIT, version, id)
}

/// A plane directory entry id read under header `version`, as assigned here.
pub(crate) const fn directory_id(version: u8, id: u64) -> u64 {
    translate(V2_DIRECTORY, version, id)
}

/// The wire id to emit when extending an image with a validated header
/// version. The directory must use the retained primary image's numbering.
pub(crate) const fn directory_wire_id(version: u8, id: u64) -> Option<u64> {
    if version != LEGACY_HEADER_VERSION {
        return Some(id);
    }
    let mut i = 0;
    while i < V2_DIRECTORY.len() {
        if V2_DIRECTORY[i].1 == id {
            return Some(V2_DIRECTORY[i].0);
        }
        i += 1;
    }
    None
}

/// A frame record id read under header `version`, as assigned here.
pub(crate) const fn frame_id(version: u8, id: u64) -> u64 {
    translate(V2_FRAME, version, id)
}

/// The side record (even, ignorable) that may precede unit main record
/// `main`: the same position in the 70-89 range as `main` in 50-69.
pub(crate) const fn side_of(main: u64) -> u64 {
    main + 19
}

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const _: () = {
    let mut i = 0;
    while i < FEATURES.len() {
        let mut j = i + 1;
        while j < FEATURES.len() {
            assert!(FEATURES[i].1 != FEATURES[j].1, "duplicate feature bit");
            j += 1;
        }
        i += 1;
    }
    i = 0;
    while i < RECORDS.len() {
        let entry = RECORDS[i];
        assert!(
            same(entry.class, "critical") || same(entry.class, "ignorable"),
            "unknown record class"
        );
        assert!(
            (entry.id % 2 == 1) == same(entry.class, "critical"),
            "record parity mismatch"
        );
        assert!(entry.id < 128, "record id needs more than one varint byte");
        // Version 1's required-profiles and mixed-pages records remain
        // retired (SPEC 1.0), including in the unified version 3 namespace.
        assert!(entry.id != 1 && entry.id != 3, "retired record id");
        let mut j = i + 1;
        while j < RECORDS.len() {
            assert!(!same(entry.name, RECORDS[j].name), "duplicate record name");
            assert!(entry.id != RECORDS[j].id, "duplicate record id");
            j += 1;
        }
        i += 1;
    }
    // Every list sits in its range (module docs).
    i = 0;
    while i < RECORDS.len() {
        let e = RECORDS[i];
        let (lo, hi) = if same(e.lists, "unit records") || same(e.lists, "V4 DATA only") {
            (50, 69)
        } else if same(e.lists, "unit side records") {
            (70, 89)
        } else if same(e.lists, "plane directory") {
            (90, 109)
        } else if same(e.lists, "frame records") {
            (110, 127)
        } else {
            (1, 49)
        };
        assert!(
            e.id >= lo && e.id <= hi,
            "record id outside its category range"
        );
        i += 1;
    }
    assert!(side_of(UNIT_DC) == UNIT_DC_SIDE);
    assert!(side_of(UNIT_ALPHA) == UNIT_ALPHA_SIDE);
    assert!(side_of(UNIT_AC) == UNIT_AC_SIDE);
    assert!(side_of(UNIT_EXACT) == UNIT_EXACT_SIDE);
    assert!(side_of(UNIT_STRIPE) == UNIT_STRIPE_SIDE);
    assert!(side_of(UNIT_NATIVE_COMPONENTS) == UNIT_NATIVE_COMPONENTS_SIDE);
    // Every version 2 id maps to a record of the same list and parity, once.
    let tables: [&[(u64, u64)]; 3] = [V2_UNIT, V2_DIRECTORY, V2_FRAME];
    let mut t = 0;
    while t < tables.len() {
        let table = tables[t];
        i = 0;
        while i < table.len() {
            assert!(
                table[i].0 % 2 == table[i].1 % 2,
                "version 2 map changes parity"
            );
            let mut j = i + 1;
            while j < table.len() {
                assert!(
                    table[i].0 != table[j].0 && table[i].1 != table[j].1,
                    "version 2 map repeats an id"
                );
                j += 1;
            }
            i += 1;
        }
        t += 1;
    }
};

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    fn cells(line: &str) -> Vec<&str> {
        line.trim_matches('|').split('|').map(str::trim).collect()
    }

    #[test]
    fn specification_and_independent_oracle_match_the_registry() {
        let spec = include_str!("../../SPEC.md");
        let feature_section = spec
            .split("### 0.1 Features")
            .nth(1)
            .unwrap()
            .split("### 0.2 Profiles")
            .next()
            .unwrap();
        let features: Vec<_> = feature_section
            .lines()
            .filter_map(|line| {
                let row = cells(line);
                let id = row.first()?.parse::<u8>().ok()?;
                Some((row[1].trim_matches('`'), id))
            })
            .collect();
        assert_eq!(features, FEATURES, "SPEC feature assignments");

        let section = spec
            .split("**Record and tag registry.**")
            .nth(1)
            .unwrap()
            .split("### 1.0 Header")
            .next()
            .unwrap();
        let rows: Vec<_> = section
            .lines()
            .filter_map(|line| {
                let row = cells(line);
                let id = row.first()?.parse::<u64>().ok()?;
                assert_eq!(row.len(), 6, "registry row: {line}");
                Some(Record {
                    name: row[5],
                    id,
                    lists: row[1],
                    meaning: row[2],
                    class: row[3],
                    feature: row[4],
                })
            })
            .collect();
        assert_eq!(rows, RECORDS, "SPEC record assignments");
        assert!(
            !section.contains("Existing overlaps"),
            "SPEC still lists overlap exceptions"
        );

        let oracle = include_str!("../../tools/imij-oracle/src/registry.rs");
        let mut feature_ids: Vec<(String, u8)> = Vec::new();
        let mut record_ids = Vec::new();
        let mut metadata = Vec::new();
        for line in oracle.lines() {
            if let Some(comment) = line.strip_prefix("// ") {
                metadata = comment.split('|').collect();
            }
            if let Some(declaration) = line.strip_prefix("pub(crate) const ") {
                let (name, value) = declaration.split_once(": ").unwrap();
                let (kind, value) = value.split_once(" = ").unwrap();
                let value = value.strip_suffix(';').unwrap();
                if kind == "u8" {
                    assert_eq!(metadata[0], "feature");
                    let id = value.parse::<u8>().unwrap();
                    assert!(
                        FEATURES.contains(&(metadata[1], id)),
                        "oracle feature meaning"
                    );
                    feature_ids.push((name.to_string(), id));
                } else {
                    assert_eq!(kind, "u64");
                    assert_eq!(metadata[0], "record");
                    record_ids.push(Record {
                        name,
                        id: value.parse().unwrap(),
                        lists: metadata[1],
                        meaning: metadata[2],
                        class: metadata[3],
                        feature: metadata[4],
                    });
                }
            }
        }
        let expected: Vec<_> = FEATURE_IDS
            .iter()
            .map(|&(name, id)| (name.to_string(), id))
            .collect();
        assert_eq!(feature_ids, expected, "oracle feature ids");
        assert_eq!(record_ids, RECORDS, "oracle record ids and meanings");
    }
}
