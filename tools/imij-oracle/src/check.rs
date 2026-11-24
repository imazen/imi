//! `imij-oracle check`: decode a sealed fixture set and compare SHA-256
//! digests against its manifest.
//!
//! A fixture is *in scope* when its `lossless` column is `true` and no frame
//! is lossy; those must decode to exactly the manifest's `.raw` digest.
//! A successful decode outside that source-exact scope must also match its
//! reconstructed `.raw` digest (including near-lossless predictive coding).
//! Unsupported lossy coding remains outside this decoder's coverage.

use crate::error::{Error, Result};
use crate::sha256::hex_digest;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub struct CheckReport {
    pub text: String,
    pub failures: usize,
}

struct Fixture {
    name: String,
    in_scope: bool,
}

pub fn check_set(path: &Path) -> Result<CheckReport> {
    check_set_with_decode(path, crate::decode)
}

/// Check sealed inputs using a caller-provided decode path (for migration tests).
pub fn check_set_with_decode(
    path: &Path,
    decode: impl Fn(&[u8]) -> Result<crate::Decoded>,
) -> Result<CheckReport> {
    let dir: PathBuf = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()
            .ok_or_else(|| Error::corrupt("manifest path has no parent"))?
            .to_path_buf()
    };
    let manifest_path = if path.is_dir() {
        dir.join("sha256.json")
    } else {
        path.to_path_buf()
    };
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|err| Error::corrupt(format!("read {}: {err}", manifest_path.display())))?;
    let manifest = parse_manifest(&manifest_text)?;

    let fixtures_path = dir.join("fixtures.tsv");
    let fixtures_text = std::fs::read_to_string(&fixtures_path)
        .map_err(|err| Error::corrupt(format!("read {}: {err}", fixtures_path.display())))?;
    let fixtures = parse_fixtures(&fixtures_text)?;

    let mut report = String::new();
    let _ = writeln!(report, "sealed set: {}", dir.display());
    let _ = writeln!(report, "fixtures: {}", fixtures.len());

    // Verify every sealed input against the manifest before decoding.
    let mut sealed_ok = true;
    let mut check_input = |relative: &str, bytes: &[u8]| match manifest.get(relative) {
        Some(expected) if hex_digest(bytes) == *expected => {}
        Some(_) => {
            sealed_ok = false;
            let _ = writeln!(report, "SEALED DATA CHANGED: {relative}");
        }
        None => {
            sealed_ok = false;
            let _ = writeln!(report, "NOT IN MANIFEST: {relative}");
        }
    };
    check_input("fixtures.tsv", fixtures_text.as_bytes());
    let mut in_scope = 0usize;
    let mut out_of_scope = 0usize;
    let mut passed = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut kind_counts: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut out_of_scope_unsupported = 0usize;
    let mut out_of_scope_other: Vec<String> = Vec::new();
    let mut reconstructed = 0usize;
    let mut reconstructed_passed = 0usize;

    for fixture in &fixtures {
        let imi_path = dir.join(format!("{}.imi", fixture.name));
        let raw_relative = format!("{}.raw", fixture.name);
        let imi = match std::fs::read(&imi_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                failures.push(format!(
                    "{}: read {}: {err}",
                    fixture.name,
                    imi_path.display()
                ));
                continue;
            }
        };
        check_input(&format!("{}.imi", fixture.name), &imi);
        if !fixture.in_scope {
            out_of_scope += 1;
            match decode(&imi) {
                Err(Error::Unsupported(_)) => out_of_scope_unsupported += 1,
                Err(other) => out_of_scope_other.push(format!("{}: {other}", fixture.name)),
                Ok(decoded) => {
                    reconstructed += 1;
                    match verify_decoded(&decoded, manifest.get(&raw_relative)) {
                        Ok(()) => reconstructed_passed += 1,
                        Err(err) => failures.push(format!("{}: {err}", fixture.name)),
                    }
                }
            }
            continue;
        }
        in_scope += 1;
        let kind = fixture.name.split('-').next().unwrap_or("?").to_string();
        let entry = kind_counts.entry(kind).or_insert((0, 0));
        match decode(&imi) {
            Ok(decoded) => {
                let mut raw = Vec::new();
                for frame in &decoded.frames {
                    raw.extend_from_slice(frame);
                }
                let digest = hex_digest(&raw);
                match manifest.get(&raw_relative) {
                    Some(expected) if *expected == digest => {
                        passed += 1;
                        entry.0 += 1;
                    }
                    Some(expected) => {
                        entry.1 += 1;
                        failures.push(format!(
                            "{}: pixel mismatch (got {digest}, want {expected})",
                            fixture.name
                        ));
                    }
                    None => {
                        entry.1 += 1;
                        failures.push(format!(
                            "{}: no manifest entry for {}",
                            fixture.name, raw_relative
                        ));
                    }
                }
            }
            Err(err) => {
                entry.1 += 1;
                failures.push(format!("{}: {err}", fixture.name));
            }
        }
    }

    let _ = writeln!(
        report,
        "source-exact scope: {in_scope}, outside source-exact scope: {out_of_scope}"
    );
    let _ = writeln!(report, "in-scope passed: {passed}/{in_scope}");
    let _ = writeln!(
        report,
        "additional reconstructed-output hashes passed: {reconstructed_passed}/{reconstructed}"
    );
    for (kind, (ok, bad)) in &kind_counts {
        let _ = writeln!(report, "  {kind}: {ok} passed, {bad} failed");
    }
    let _ = writeln!(
        report,
        "outside source-exact scope rejected as Unsupported: {out_of_scope_unsupported}/{out_of_scope}"
    );
    for example in out_of_scope_other.iter().take(5) {
        let _ = writeln!(report, "  note (outside source-exact scope): {example}");
    }
    for failure in failures.iter().take(20) {
        let _ = writeln!(report, "FAIL {failure}");
    }
    if failures.len() > 20 {
        let _ = writeln!(report, "... and {} more failures", failures.len() - 20);
    }
    let _ = writeln!(report, "failures: {}", failures.len());
    let mut total_failures = failures.len();
    if !sealed_ok {
        total_failures += 1;
        let _ = writeln!(report, "sealed data verification FAILED");
    }
    Ok(CheckReport {
        text: report,
        failures: total_failures,
    })
}

fn verify_decoded(decoded: &crate::Decoded, expected: Option<&String>) -> Result<()> {
    let mut raw = Vec::new();
    for frame in &decoded.frames {
        raw.extend_from_slice(frame);
    }
    let digest = hex_digest(&raw);
    match expected {
        Some(expected) if *expected == digest => Ok(()),
        Some(expected) => Err(Error::corrupt(format!(
            "pixel mismatch (got {digest}, want {expected})"
        ))),
        None => Err(Error::corrupt("no manifest entry for reconstructed raw")),
    }
}

fn parse_fixtures(text: &str) -> Result<Vec<Fixture>> {
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| Error::corrupt("empty fixtures.tsv"))?;
    let columns: Vec<&str> = header.split('\t').collect();
    let lossless_col = columns
        .iter()
        .position(|name| *name == "lossless")
        .ok_or_else(|| Error::corrupt("fixtures.tsv has no lossless column"))?;
    let frames_col = columns.iter().position(|name| *name == "frames");
    let mut fixtures = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() <= lossless_col {
            return Err(Error::corrupt("short fixtures.tsv row"));
        }
        let frames = frames_col
            .and_then(|col| fields.get(col))
            .copied()
            .unwrap_or("");
        let in_scope = fields[lossless_col] == "true" && !frames.contains("Some(Lossy)");
        fixtures.push(Fixture {
            name: fields[0].to_string(),
            in_scope,
        });
    }
    Ok(fixtures)
}

/// Parse the flat `{"files": {name: digest, ...}, ...}` manifest shape that
/// the sealing tool writes.
fn parse_manifest(text: &str) -> Result<BTreeMap<String, String>> {
    let mut cursor = 0usize;
    let bytes = text.as_bytes();
    let skip_ws = |cursor: &mut usize| {
        while *cursor < bytes.len() && (bytes[*cursor] as char).is_whitespace() {
            *cursor += 1;
        }
    };
    let expect = |cursor: &mut usize, ch: u8| -> Result<()> {
        skip_ws(cursor);
        if bytes.get(*cursor) == Some(&ch) {
            *cursor += 1;
            Ok(())
        } else {
            Err(Error::corrupt("malformed manifest"))
        }
    };
    let read_string = |cursor: &mut usize| -> Result<String> {
        skip_ws(cursor);
        if bytes.get(*cursor) != Some(&b'"') {
            return Err(Error::corrupt("malformed manifest key"));
        }
        *cursor += 1;
        let start = *cursor;
        while *cursor < bytes.len() && bytes[*cursor] != b'"' {
            if bytes[*cursor] == b'\\' {
                *cursor += 1;
            }
            *cursor += 1;
        }
        if bytes.get(*cursor) != Some(&b'"') {
            return Err(Error::corrupt("unterminated manifest string"));
        }
        let value = text[start..*cursor].to_string();
        *cursor += 1;
        Ok(value)
    };
    expect(&mut cursor, b'{')?;
    // Find the "files" member.
    let mut files: Option<BTreeMap<String, String>> = None;
    loop {
        skip_ws(&mut cursor);
        if bytes.get(cursor) == Some(&b'}') {
            break;
        }
        let key = read_string(&mut cursor)?;
        expect(&mut cursor, b':')?;
        if key == "files" {
            let mut map = BTreeMap::new();
            expect(&mut cursor, b'{')?;
            loop {
                skip_ws(&mut cursor);
                if bytes.get(cursor) == Some(&b'}') {
                    cursor += 1;
                    break;
                }
                let name = read_string(&mut cursor)?;
                expect(&mut cursor, b':')?;
                let digest = read_string(&mut cursor)?;
                map.insert(name, digest);
                skip_ws(&mut cursor);
                if bytes.get(cursor) == Some(&b',') {
                    cursor += 1;
                }
            }
            files = Some(map);
        } else {
            let _ = read_string(&mut cursor)?;
        }
        skip_ws(&mut cursor);
        if bytes.get(cursor) == Some(&b',') {
            cursor += 1;
        }
    }
    files.ok_or_else(|| Error::corrupt("manifest has no files map"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_small_manifest() {
        let text = "{\n  \"files\": {\n    \"a.imi\": \"aa\",\n    \"a.raw\": \"bb\"\n  },\n  \"provenance\": \"x\"\n}";
        let map = parse_manifest(text).unwrap();
        assert_eq!(map.get("a.imi").map(String::as_str), Some("aa"));
        assert_eq!(map.get("a.raw").map(String::as_str), Some("bb"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn classifies_lossy_out_of_scope() {
        let tsv = "name\tw\tlossless\tkind\tframes\na\t1\ttrue\tstill\t\nb\t1\tfalse\tstill\t\nc\t1\ttrue\tAnimation\t0,0,1,1,0,1,Some(Lossy);\n";
        let fixtures = parse_fixtures(tsv).unwrap();
        assert!(fixtures[0].in_scope);
        assert!(!fixtures[1].in_scope);
        assert!(!fixtures[2].in_scope);
    }

    #[test]
    fn supplemental_reconstruction_requires_the_exact_raw_hash() {
        let decoded = crate::Decoded {
            width: 1,
            height: 1,
            channels: 1,
            sample: 0,
            frames: vec![vec![17], vec![29]],
        };
        let expected = hex_digest(&[17, 29]);
        assert!(verify_decoded(&decoded, Some(&expected)).is_ok());
        let wrong_reference = hex_digest(&[17, 28]);
        assert!(matches!(verify_decoded(&decoded, Some(&wrong_reference)),
            Err(Error::Corrupt(message)) if message.starts_with("pixel mismatch")));
        assert!(matches!(
            verify_decoded(&decoded, None),
            Err(Error::Corrupt(_))
        ));
    }
}
