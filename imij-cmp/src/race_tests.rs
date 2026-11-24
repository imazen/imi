//! Lab switches travel with each cell's encode (benchmarks/raceaudit_2026-10-09).
//!
//! Before the fix, `encode_cell` set the lab switches in process-global
//! atomics, so `ladder-batch`'s parallel renditions saw each other's values:
//! 6.6% of the AQ + trellis study's VALIDATE cells came out with another
//! cell's switch. These tests pin what the fix guarantees: a cell's bytes
//! depend on its own configuration only, whatever the process-global switches
//! hold and however many cells encode at once.
use crate::rd::encode_cell;
use serde_json::{Value, json};
use std::collections::HashMap;

/// A 96x72 image with edges, texture and flat areas (small, so the table
/// counts and the trellis both matter).
fn image() -> (Vec<u8>, u32, u32) {
    let (w, h) = (96u32, 72u32);
    let mut px = Vec::with_capacity((w * h * 3) as usize);
    let mut seed = 0x2545_f491_u32;
    for y in 0..h {
        for x in 0..w {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed % 23) as i32 - 11;
            let ramp = (x * 255 / w) as i32;
            let disc = if (x as i32 - 40).pow(2) + (y as i32 - 30).pow(2) < 300 {
                180
            } else {
                0
            };
            let stripes = if (x / 6) % 2 == 0 && y > 48 { 90 } else { 0 };
            let v = |c: i32| (c + noise).clamp(0, 255) as u8;
            px.extend([
                v(ramp / 2 + disc),
                v(200 - disc + stripes),
                v((y * 3) as i32 + stripes),
            ]);
        }
    }
    (px, w, h)
}

/// Encoder-ablation cells that differ only in lab switches (and their AQ /
/// tier companions), each switch taking several values.
fn grid() -> Vec<Value> {
    let base = |q: f64, entropy: &str| {
        json!({"codec":"imij","quality":q,"color":"xyb","entropy":entropy,"stripe_block_rows":8,
               "rdoq_lambda":0,"effort":10,"threads":6,"experiment":"encoder-ablation",
               "dc_step":[0,0],"dering":true,"dc_nearest":true})
    };
    let field = [
        ("aq_field_strength", json!(2.0)),
        ("aq_field_levels", json!(0)),
        ("aq_field_cell", json!(1)),
        ("aq_field_relative", json!(true)),
        ("aq_field_fade", json!(false)),
    ];
    let mut out = Vec::new();
    for q in [12.0, 45.0] {
        out.push(base(q, "runlevel"));
        out.push(base(q, "splithuf"));
        let mut c = base(q, "splithuf");
        c["trellis_rate"] = json!("none");
        out.push(c);
        let mut c = base(q, "splithuf");
        c["trellis_rate"] = json!("huf");
        c["trellis_channel_shift"] = json!([1.0, 2.0, 2.0]);
        out.push(c);
        let mut c = base(q, "runlevel");
        c["trellis_channel_shift"] = json!([0.0, 4.0, 4.0]);
        out.push(c);
        let mut c = base(q, "runlevel");
        for (k, v) in &field {
            c[*k] = v.clone();
        }
        out.push(c.clone());
        c["trellis_aq_exp"] = json!(2.0);
        out.push(c);
        let mut c = base(q, "runlevel");
        c["dc_step"] = json!([48, 80]);
        c["dering"] = json!(false);
        c["dc_nearest"] = json!(false);
        out.push(c);
        for mode in [0, 1, 2] {
            let mut c = base(q, "runlevel");
            c["table_k_mode"] = json!(mode);
            out.push(c);
            let mut c = base(q, "splithuf");
            c["table_k_mode"] = json!(mode);
            out.push(c);
        }
    }
    for c in &out {
        crate::rd::validate_config(c).unwrap();
    }
    out
}

/// Every cell encoded alone, one encoder thread, process-global switches at
/// their defaults.
fn reference(grid: &[Value], raw: &[u8], w: u32, h: u32) -> Vec<Vec<u8>> {
    grid.iter()
        .map(|c| encode_cell(c, raw, w, h, 1).unwrap().0)
        .collect()
}

/// Sets every process-global encoder switch away from its default.
fn scramble_globals() {
    imij::lab::set_lossy_dc_step(96, 32);
    imij::lab::set_lossy_dering(false);
    imij::lab::set_lossy_dc_nearest(false);
    imij::lab::set_lossy_dc_stripes(1);
    imij::lab::set_lossy_dc_lanes(3);
    imij::lab::set_trellis_aq_lambda_exp(-3.0);
    imij::lab::set_trellis_channel_shift([5.0, -2.0, 6.0]);
    imij::lab::set_trellis_huf_rate(false);
    imij::lab::set_table_k_mode(2);
}

fn restore_globals() {
    imij::lab::set_lossy_dc_step(0, 0);
    imij::lab::set_lossy_dering(true);
    imij::lab::set_lossy_dc_nearest(true);
    imij::lab::set_lossy_dc_stripes(0);
    imij::lab::set_lossy_dc_lanes(0);
    imij::lab::set_trellis_aq_lambda_exp(0.0);
    imij::lab::set_trellis_channel_shift([0.0; 3]);
    imij::lab::set_trellis_huf_rate(true);
    imij::lab::reset_table_k_mode();
}

/// One test, so no other test of this binary touches the globals meanwhile.
#[test]
fn mixed_switch_grid_matches_cells_encoded_alone() {
    let (raw, w, h) = image();
    let grid = grid();
    let want = reference(&grid, &raw, w, h);
    // The grid exercises the switches: each one changes the bytes of the cell
    // that sets it (against its base cell) at one quality at least.
    // `grid()` lays out 14 cells per quality in this order.
    const PER_Q: usize = 14;
    assert_eq!(grid.len(), 2 * PER_Q);
    let changes = |base: usize, other: &[usize]| {
        (0..2).any(|q| {
            other
                .iter()
                .any(|&o| want[q * PER_Q + base] != want[q * PER_Q + o])
        })
    };
    for (what, base, other) in [
        ("split-Huffman rate model off", 1, &[2][..]),
        ("split-Huffman channel offsets", 1, &[3]),
        ("run-level channel offsets", 0, &[4]),
        ("AQ lambda exponent", 5, &[6]),
        ("DC steps, deringing, DC rounding", 0, &[7]),
        ("run-level table-count modes", 0, &[8, 10, 12]),
        ("split-Huffman table-count modes", 1, &[9, 11, 13]),
    ] {
        assert!(
            changes(base, other),
            "{what} changes no bytes; the grid does not test it"
        );
    }

    // 1. The process-global switches no longer reach a cell.
    scramble_globals();
    for (c, want) in grid.iter().zip(&want) {
        for threads in [1, 4] {
            let (got, _) = encode_cell(c, &raw, w, h, threads).unwrap();
            assert!(
                &got == want,
                "global switches leaked into {c} ({threads} encoder threads)"
            );
        }
    }
    restore_globals();

    // 2. Many cells at once, in different orders on every thread, with one and
    // with several encoder threads: each cell keeps its own bytes.
    let jobs: Vec<usize> = (0..grid.len()).collect();
    for imij_threads in [1, 4] {
        let results: Vec<HashMap<usize, Vec<Vec<u8>>>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|t| {
                    let (grid, raw, jobs) = (&grid, &raw, &jobs);
                    s.spawn(move || {
                        let mut got: HashMap<usize, Vec<Vec<u8>>> = HashMap::new();
                        for round in 0..3 {
                            let mut order = jobs.clone();
                            let shift = (t * 5 + round * 3) % order.len();
                            order.rotate_left(shift);
                            if (t + round) % 2 == 1 {
                                order.reverse();
                            }
                            for i in order {
                                let (bytes, _) =
                                    encode_cell(&grid[i], raw, w, h, imij_threads).unwrap();
                                got.entry(i).or_default().push(bytes);
                            }
                        }
                        got
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for got in results {
            for (i, files) in got {
                for f in files {
                    assert!(
                        f == want[i],
                        "cell {} differs from its lone encode ({imij_threads} encoder threads)",
                        grid[i]
                    );
                }
            }
        }
    }
}

/// imij-cmp never sets a process-global lab switch: every switch a cell
/// needs goes in its `ImiEncoderConfig::lab`.
#[test]
fn no_process_global_lab_switches() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") || path.ends_with("race_tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if let Some(at) = code.find("lab::set_") {
                let rest = &code[at + "lab::set_".len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if rest[name.len()..].trim_start().starts_with('(') {
                    offenders.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "process-global lab switches set in imij-cmp:\n{}",
        offenders.join("\n")
    );
}
