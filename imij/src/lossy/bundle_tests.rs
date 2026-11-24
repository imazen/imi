//! The normative bundle (SPEC "Normative bundle"): every artifact the
//! format's text delegates to, by content hash. This test computes each
//! artifact's XXH3-128 and checks it against the SPEC table, so a change to a
//! delegated byte, table or source file fails here until the SPEC says so.
use super::*;
use alloc::string::String;

/// Source text with CRLF line endings normalized to LF (a Windows checkout
/// hashes like any other).
fn lf(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for (i, &b) in text.iter().enumerate() {
        if b == b'\r' && text.get(i + 1) == Some(&b'\n') {
            continue;
        }
        out.push(b);
    }
    out
}

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// (the SPEC row's first cell, the artifact's bytes).
fn artifacts() -> Vec<(&'static str, Vec<u8>)> {
    let bases: Vec<f32> = (0..=4u8)
        .flat_map(|m| {
            let qp = QParams::from_quality(90.0, LossyColor::YCoCg, m);
            (0..3).flat_map(move |c| qp.base(c))
        })
        .collect();
    vec![
        (
            "`imij/src/seeds/bank1.bin`",
            include_bytes!("../seeds/bank1.bin").to_vec(),
        ),
        ("`metadata_pack::SEED`", crate::metadata_pack::SEED.to_vec()),
        ("`srgb_table::SRGB8_LOG`", srgb_table::SRGB8_LOG.to_vec()),
        (
            "`Transfer::Srgb.table()`",
            f32_bytes(&transfer::Transfer::Srgb.table()),
        ),
        (
            "`Transfer::Pq.table()`",
            f32_bytes(&transfer::Transfer::Pq.table()),
        ),
        (
            "`Transfer::Hlg.table()`",
            f32_bytes(&transfer::Transfer::Hlg.table()),
        ),
        ("`QParams::base`, matrices 0-4", f32_bytes(&bases)),
        ("`imij/src/bits.rs`", lf(include_bytes!("../bits.rs"))),
        ("`imij/src/lossy/coef.rs`", lf(include_bytes!("coef.rs"))),
        ("`imij/src/lossy/huf.rs`", lf(include_bytes!("huf.rs"))),
        (
            "`imij/src/lossy/subblock.rs`",
            lf(include_bytes!("subblock.rs")),
        ),
        ("`imij/src/lossy/dct.rs`", lf(include_bytes!("dct.rs"))),
        ("`imij/src/lossy/color.rs`", lf(include_bytes!("color.rs"))),
        (
            "`imij/src/lossy/transfer.rs`",
            lf(include_bytes!("transfer.rs")),
        ),
    ]
}

/// The bundle table's rows: (first cell, XXH3-128 cell).
fn spec_rows() -> Vec<(String, String)> {
    let spec = include_str!("../../../SPEC.md");
    let start = spec
        .find("\n## Normative bundle")
        .expect("SPEC has a normative bundle section");
    spec[start + 1..]
        .lines()
        .skip(1)
        .take_while(|l| !l.starts_with('#'))
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            let cells: Vec<&str> = l.split(" | ").collect();
            (
                cells[0].trim_start_matches("| ").to_string(),
                cells[1].trim().to_string(),
            )
        })
        .collect()
}

#[test]
fn normative_bundle_matches_spec() {
    let rows = spec_rows();
    let mut wrong = Vec::new();
    let arts = artifacts();
    for (name, bytes) in &arts {
        let got = alloc::format!("{:032x}", xxhash_rust::xxh3::xxh3_128(bytes));
        let listed = rows.iter().find(|r| r.0.starts_with(name)).map(|r| &r.1);
        if listed != Some(&got) {
            wrong.push(alloc::format!("{name}: SPEC {listed:?}, computed {got}"));
        }
    }
    assert!(wrong.is_empty(), "normative bundle:\n{}", wrong.join("\n"));
    assert_eq!(
        rows.len(),
        arts.len(),
        "every SPEC row has an artifact here"
    );
}
