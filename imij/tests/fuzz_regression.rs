//! Fuzz crash regression suite.
//!
//! Runs every file in `fuzz/regression/` and every seed in `fuzz/seeds/`
//! through every fuzz target's logic (`fuzz/fuzz_targets/*_core.rs`, included
//! here so the stable replay shares one source with the nightly fuzz bins).
//! Each input must not panic; the `roundtrip_*` targets additionally assert
//! their pixel oracles, and `region`/`differential` their parity contracts.
//!
//! A second test runs the `differential` routes in child processes with the
//! documented `IMIJ_SCALAR=1` and `IMIJ_NO_AVX512=1` switches and compares
//! their per-input hashes against this process's default-tier hashes, so
//! scalar/AVX2/default outputs are checked bit-identical on every regression
//! seed and seed without needing fuzzing infrastructure.
//!
//! Reproduces what the eight fuzz targets do; failures here mean a codec
//! regression or a stale false assertion in a harness, never a missing
//! nightly toolchain. To add a regression: drop the `cargo fuzz tmin` output
//! into `fuzz/regression/`; to add a seed, add the file to `fuzz/seeds/`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

mod header_impl {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fuzz/fuzz_targets/header_core.rs"
    ));
}
mod decode_impl {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fuzz/fuzz_targets/decode_core.rs"
    ));
}
mod region_impl {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fuzz/fuzz_targets/region_core.rs"
    ));
}
mod frames_impl {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fuzz/fuzz_targets/frames_core.rs"
    ));
}
mod checksum_impl {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fuzz/fuzz_targets/checksum_core.rs"
    ));
}
mod roundtrip_lossless_impl {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fuzz/fuzz_targets/roundtrip_lossless_core.rs"
    ));
}
mod roundtrip_lossy_impl {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fuzz/fuzz_targets/roundtrip_lossy_core.rs"
    ));
}
mod differential_impl {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fuzz/fuzz_targets/differential_core.rs"
    ));
}

/// Inputs as `(name, bytes)`: regression artifacts first, then seeds, sorted
/// per directory so the replay order is stable.
fn inputs() -> Vec<(String, Vec<u8>)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fuzz");
    let mut all = Vec::new();
    for dir in ["regression", "seeds"] {
        let path = root.join(dir);
        let mut entries: Vec<_> = fs::read_dir(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            // The directory's README documents it; it is not an input.
            .filter(|e| e.file_name() != "README.md")
            .map(|e| e.path())
            .collect();
        entries.sort();
        for entry in entries {
            let name = format!(
                "{dir}/{}",
                entry.file_name().and_then(|n| n.to_str()).unwrap_or("?")
            );
            let data = fs::read(&entry).unwrap_or_else(|e| panic!("read {name}: {e}"));
            all.push((name, data));
        }
    }
    all
}

#[test]
fn fuzz_regression_seeds_do_not_panic() {
    let all = inputs();
    assert!(
        !all.is_empty(),
        "fuzz/seeds/ and fuzz/regression/ are empty; at least the curated seeds must be present"
    );
    let mut regression = 0;
    for (name, data) in &all {
        // Every entry point may return Err but must not panic. The panicking
        // oracles live inside the shared cores.
        header_impl::run(data);
        decode_impl::run(data);
        region_impl::run(data);
        frames_impl::run(data);
        checksum_impl::run(data);
        roundtrip_lossless_impl::run(data);
        roundtrip_lossy_impl::run(data);
        differential_impl::run(data);
        if name.starts_with("regression/") {
            regression += 1;
        }
        eprintln!("ok: {name} ({} bytes)", data.len());
    }
    eprintln!(
        "replayed {} inputs ({} regression artifacts, {} seeds) through 8 targets",
        all.len(),
        regression,
        all.len() - regression
    );
}

/// Positive encoded seeds must reach their intended decode paths after a
/// format migration; a clean rejection alone cannot establish that coverage.
#[cfg(feature = "extended")]
#[test]
fn curated_encoded_seeds_retain_decode_coverage() {
    for (name, data) in inputs()
        .into_iter()
        .filter(|(n, _)| n.starts_with("seeds/"))
    {
        if name.starts_with("seeds/37-") || name.starts_with("seeds/38-") {
            assert!(
                matches!(imij::decode(&data), Err(imij::Error::Unsupported(_))),
                "{name}"
            );
            continue;
        }
        if name.starts_with("seeds/47-") || name.starts_with("seeds/48-") {
            assert!(
                matches!(imij::decode(&data), Err(imij::Error::Corrupt(_))),
                "{name}: expected Corrupt"
            );
            continue;
        }
        let (info, _) = imij::decode(&data).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let mut decoder =
            imij::FrameDecoder::new(&data).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let mut canvas = vec![0; info.checked_buffer_len().unwrap()];
        while decoder.next_frame().unwrap().is_some() {
            let frame = if decoder.kind() == imij::FrameKind::Animation {
                info.clone()
            } else {
                decoder.frame_info().unwrap()
            };
            canvas.resize(frame.checked_buffer_len().unwrap(), 0);
            decoder
                .decode_into(&mut canvas, frame.row_bytes(), 1)
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }
}

/// Test name invoked in child processes by [`tier_hashes_match_default`].
const CHILD_FILTER: &str = "tier_child_hashes";
/// Set in child processes to a label describing the forced tier.
const CHILD_ENV: &str = "IMIJ_FUZZ_TIER_CHILD";
/// Child output line prefix.
const CHILD_PREFIX: &str = "IMIJFUZZ ";

/// Child mode: print one line per input with the 1-thread and 4-thread route
/// hashes. A no-op in a normal test run.
#[test]
fn tier_child_hashes() {
    let Some(label) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    eprintln!("child tier {label:?}");
    for (name, data) in inputs() {
        let (one, four) = match differential_impl::hash_routes(&data) {
            Some(r) => r,
            None => {
                println!("{CHILD_PREFIX}{name} skip skip");
                continue;
            }
        };
        let show = |r: Result<u64, ()>| match r {
            Ok(h) => format!("{h:016x}"),
            Err(()) => "err".to_string(),
        };
        println!("{CHILD_PREFIX}{name} {} {}", show(one), show(four));
    }
}

fn tier_child(label: &str, env: (&str, &str)) -> HashMap<String, (String, String)> {
    let exe = std::env::current_exe().expect("current test binary");
    let out = std::process::Command::new(exe)
        .args([
            "--exact",
            CHILD_FILTER,
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD_ENV, label)
        .env(env.0, env.1)
        .output()
        .unwrap_or_else(|e| panic!("spawn tier child {label}: {e}"));
    assert!(
        out.status.success(),
        "tier child {label} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut map = HashMap::new();
    for line in stdout.lines() {
        // libtest may prefix the first printed line with "test <name> ... ".
        let Some(at) = line.find(CHILD_PREFIX) else {
            continue;
        };
        let rest = &line[at + CHILD_PREFIX.len()..];
        let mut it = rest.split_whitespace();
        let (Some(name), Some(one), Some(four)) = (it.next(), it.next(), it.next()) else {
            panic!("bad child line {line:?}");
        };
        map.insert(name.to_string(), (one.to_string(), four.to_string()));
    }
    assert!(!map.is_empty(), "tier child {label} produced no hashes");
    map
}

/// The documented tier switches (`IMIJ_SCALAR`, `IMIJ_NO_AVX512`) must select
/// bit-identical decoded pixels: for every seed and regression input, the
/// child's per-route hashes must equal this process's. Zero tolerance.
#[test]
fn tier_hashes_match_default() {
    if std::env::var_os(CHILD_ENV).is_some() {
        return;
    }
    let all = inputs();
    let our: HashMap<String, (String, String)> = all
        .iter()
        .map(|(name, data)| {
            let (one, four) = differential_impl::hash_routes(data)
                .map(|(a, b)| {
                    let show = |r: Result<u64, ()>| match r {
                        Ok(h) => format!("{h:016x}"),
                        Err(()) => "err".to_string(),
                    };
                    (show(a), show(b))
                })
                .unwrap_or_else(|| ("skip".to_string(), "skip".to_string()));
            (name.clone(), (one, four))
        })
        .collect();
    for (label, env) in [
        ("scalar", ("IMIJ_SCALAR", "1")),
        ("no_avx512", ("IMIJ_NO_AVX512", "1")),
    ] {
        let child = tier_child(label, env);
        for (name, expected) in &our {
            let got = child
                .get(name)
                .unwrap_or_else(|| panic!("tier child {label} omitted {name}"));
            assert_eq!(
                got, expected,
                "tier {label} and default disagree on {name} (1t, 4t)"
            );
        }
    }
}
