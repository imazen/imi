//! imij-seeds: seed-table banks for IMI (training data, bank generation,
//! A/B cells, timing). See benchmarks/entropy_seeds_2026-10-07/README.md.
//!
//!   imij-seeds collect [GRID] [--min-ratio R] < image.ppm
//!   imij-seeds train OUT.bin [TRAIN] [--val VAL.tsv] < records.tsv
//!   imij-seeds dump BANK.bin
//!   imij-seeds eval [GRID] [--bank N] < image.ppm
//!   imij-seeds dtime FILE REPS
//!   imij-seeds etime REPS [GRID] [--bank N] < image.ppm
//!
//! `collect` and `eval` read one binary PPM (P6, 8-bit RGB) from stdin and
//! encode it lossily once per grid cell, one encoder thread. GRID flags
//! (comma lists): --q 1,2,.. --color xyb,ycocg --mode rl,sh,tier
//! --effort 7.5,10 (tier = `DecodeSpeed::Balanced`, the encoder picks);
//! eval also takes --bank N (full-width bank identity) and
//! --write DIR (keeps both files of each cell).
//!
//! collect, TSV per cell: `C color mode effort q bytes`, then per table
//! blob `G role mode bits table_bits tables`, then per table whose table
//! bits are at least R (default 0.01) x its data bits
//! `T role kind k,m,depth table_bits data_bits sym:count,...`.
//!
//! train reads T lines, trains each kind (lossless: each token scheme with
//! enough tables) and writes the bank blob (layout: `imij::seeds`, `Bank`);
//! with --val, also prints the estimated saving on those tables; dump prints
//! a bank blob as text. TRAIN flags:
//! --k K --k-kind KIND[/k.m.depth]=K,.. --prec P --prior F --candidates N --lloyd N
//! --max-recs N --min-recs N --kinds 0,1,4,5,6 --threads N.
//!
//! eval, TSV per cell: `E color mode effort q bytes_off bytes_on entropy_off
//! entropy_on bank_id seed_scope exact seeds enc_us_off enc_us_on` where exact = 1
//! when the seeded file decodes to the same pixels as the unseeded one and
//! seeds lists `role.kind:count:est_bits` per role and kind (tables the
//! final file seeds; est_bits = the encoder's estimate of bits saved).
//!
//! dtime decodes FILE REPS times and prints `min_ns median_ns`.
//!
//!   imij-seeds etime REPS [GRID] [--bank N] < image.ppm
//!
//! times encodes without seeds, with seeds (exact, the default) and with
//! seeds trusting the estimate (`seed_exact = false`), interleaved REPS times; per cell
//! `T color mode effort q ns_off ns_exact ns_estimate bytes_off bytes_exact
//! bytes_estimate` (minimum times).

mod train;

use imij::{
    Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, LossyColor, LossyEntropy, Metadata,
    SampleType,
};
use std::collections::BTreeMap;
use std::io::{BufRead, Read, Write};

fn read_ppm() -> (u32, u32, Vec<u8>) {
    let mut all = Vec::new();
    std::io::stdin().read_to_end(&mut all).unwrap();
    // P6 <ws> W <ws> H <ws> 255 <one ws> data; no comments.
    let mut fields = Vec::new();
    let mut i = 0;
    while fields.len() < 4 {
        while all[i].is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while !all[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(String::from_utf8(all[s..i].to_vec()).unwrap());
    }
    assert_eq!(fields[0], "P6", "binary PPM expected");
    assert_eq!(fields[3], "255", "8-bit PPM expected");
    let w: u32 = fields[1].parse().unwrap();
    let h: u32 = fields[2].parse().unwrap();
    let data = all[i + 1..].to_vec();
    assert_eq!(data.len(), (w * h * 3) as usize, "PPM size");
    (w, h, data)
}

struct Grid {
    q: Vec<f32>,
    color: Vec<LossyColor>,
    mode: Vec<&'static str>,
    effort: Vec<f32>,
    min_ratio: f64,
    bank: u64,
    write: Option<String>,
}

fn grid(args: &[String]) -> Grid {
    let mut g = Grid {
        q: vec![5.0, 30.0, 90.0],
        color: vec![LossyColor::Xyb],
        mode: vec!["rl"],
        effort: vec![7.5],
        min_ratio: 0.01,
        bank: 1,
        write: None,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = it.next().expect("flag value");
        let list = v.split(',');
        match a.as_str() {
            "--q" => g.q = list.map(|s| s.parse().unwrap()).collect(),
            "--effort" => g.effort = list.map(|s| s.parse().unwrap()).collect(),
            "--min-ratio" => g.min_ratio = v.parse().unwrap(),
            "--bank" => g.bank = v.parse().unwrap(),
            "--write" => g.write = Some(v.clone()),
            "--color" => {
                g.color = list
                    .map(|s| match s {
                        "xyb" => LossyColor::Xyb,
                        "ycocg" => LossyColor::YCoCg,
                        _ => panic!("color {s}"),
                    })
                    .collect()
            }
            "--mode" => {
                g.mode = list
                    .map(|s| match s {
                        "rl" => "rl",
                        "sh" => "sh",
                        "tier" => "tier",
                        _ => panic!("mode {s}"),
                    })
                    .collect()
            }
            _ => panic!("unknown flag {a}"),
        }
    }
    g
}

fn color_name(c: LossyColor) -> &'static str {
    match c {
        LossyColor::Xyb => "xyb",
        LossyColor::YCoCg => "ycocg",
        _ => "other",
    }
}

fn config(color: LossyColor, mode: &str, effort: f32, q: f32) -> ImiEncoderConfig {
    let cfg = imij::ImiEncoderConfig::lossy(q)
        .tune_lossy(|t| t.color = color)
        .with_effort(effort)
        .with_threads(1);
    match mode {
        "sh" => cfg.tune_lossy(|t| t.entropy = LossyEntropy::SplitHuf),
        "tier" => cfg.with_decode_speed(DecodeSpeed::Balanced),
        _ => cfg.tune_lossy(|t| t.entropy = LossyEntropy::RunLevel),
    }
}

/// Runs `f` for every grid cell.
fn cells(g: &Grid, mut f: impl FnMut(LossyColor, &'static str, f32, f32)) {
    for &color in &g.color {
        for &mode in &g.mode {
            for &effort in &g.effort {
                for &q in &g.q {
                    f(color, mode, effort, q);
                }
            }
        }
    }
}

fn collect(args: &[String]) {
    let g = grid(args);
    let (w, h, px) = read_ppm();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let out = std::io::stdout();
    let mut o = std::io::BufWriter::new(out.lock());
    cells(&g, |color, mode, effort, q| {
        let mut cfg = config(color, mode, effort, q);
        // Explicit tables only: the training data are what the encoder
        // builds before any seed replaces a table.
        cfg = cfg.tune_lossy(|t| {
            t.lab.seed_bank = 0;
        });
        let (bytes, cap) = imij::seeds::capture(|| {
            cfg.encode_samples(
                imij::ImageRef::new(&px, w as usize * 3, &info),
                &Metadata::default(),
            )
            .unwrap()
        });
        writeln!(
            o,
            "C\t{}\t{mode}\t{effort}\t{q}\t{}",
            color_name(color),
            bytes.len()
        )
        .unwrap();
        for gr in &cap.groups {
            writeln!(
                o,
                "G\t{}\t{}\t{}\t{}\t{}",
                gr.role, gr.mode, gr.bits, gr.table_bits, gr.tables
            )
            .unwrap();
        }
        for t in &cap.tables {
            if (t.table_bits as f64) < g.min_ratio * t.data_bits {
                continue;
            }
            let counts: Vec<String> = t
                .counts
                .iter()
                .enumerate()
                .filter(|(_, c)| **c > 0)
                .map(|(s, c)| format!("{s}:{c}"))
                .collect();
            writeln!(
                o,
                "T\t{}\t{}\t{},{},{}\t{}\t{:.1}\t{}",
                t.role,
                t.kind,
                t.tc[0],
                t.tc[1],
                t.tc[2],
                t.table_bits,
                t.data_bits,
                counts.join(",")
            )
            .unwrap();
        }
    });
}

/// T records grouped by (kind, tc).
type Pools = BTreeMap<(u8, [u8; 3]), Vec<train::Rec>>;

fn read_recs(src: impl BufRead) -> Pools {
    let mut pools = Pools::new();
    for line in src.lines() {
        let line = line.unwrap();
        let p: Vec<&str> = line.split('\t').collect();
        if p.first() != Some(&"T") {
            continue;
        }
        let kind: u8 = p[2].parse().unwrap();
        let tc: Vec<u8> = p[3].split(',').map(|x| x.parse().unwrap()).collect();
        let table_bits: f64 = p[4].parse().unwrap();
        let data_bits: f64 = p[5].parse().unwrap();
        if p[6].is_empty() {
            // An empty cluster's table codes nothing.
            continue;
        }
        let syms = p[6]
            .split(',')
            .map(|sc| {
                let (s, c) = sc.split_once(':').unwrap();
                (s.parse().unwrap(), c.parse().unwrap())
            })
            .collect();
        pools
            .entry((kind, [tc[0], tc[1], tc[2]]))
            .or_default()
            .push(train::Rec {
                syms,
                explicit: table_bits + data_bits,
            });
    }
    pools
}

const KIND_NAMES: [&str; 7] = [
    "RlEob", "RlAc", "PkCount", "PkWidth", "HufCount", "HufWidth", "Lossless",
];

fn train(args: &[String]) {
    let out_bin = &args[0];
    let mut cfg = train::TrainCfg::default();
    let mut max_recs = 400_000usize;
    let mut min_recs = 2_000usize;
    let mut kinds: Vec<u8> = vec![0, 1, 4, 5, 6];
    // KIND or KIND/k.m.depth -> seed count.
    let mut k_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut val: Option<String> = None;
    let mut bank_id = 1u8;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let v = it.next().expect("flag value");
        match a.as_str() {
            "--k" => cfg.k = v.parse().unwrap(),
            "--prec" => cfg.prec = v.parse().unwrap(),
            "--candidates" => cfg.candidates = v.parse().unwrap(),
            "--lloyd" => cfg.lloyd = v.parse().unwrap(),
            "--scale" => cfg.scale = v.parse().unwrap(),
            "--prior" => cfg.prior = v.parse().unwrap(),
            "--threads" => cfg.threads = v.parse().unwrap(),
            "--max-recs" => max_recs = v.parse().unwrap(),
            "--min-recs" => min_recs = v.parse().unwrap(),
            "--kinds" => kinds = v.split(',').map(|x| x.parse().unwrap()).collect(),
            "--k-kind" => {
                for kv in v.split(',') {
                    let (k, n) = kv.split_once('=').unwrap();
                    k_kind.insert(k.to_string(), n.parse().unwrap());
                }
            }
            "--val" => val = Some(v.clone()),
            "--id" => bank_id = v.parse().unwrap(),
            _ => panic!("unknown flag {a}"),
        }
    }
    let pools = read_recs(std::io::stdin().lock());
    let vals = val.map(|p| read_recs(std::io::BufReader::new(std::fs::File::open(p).unwrap())));
    let mut trained = Vec::new();
    eprintln!(
        "kind\ttc\trecs\tused\tseeds\tbytes\ttrain_saving_bits\ttrain_using\tval_recs\tval_saving_bits\tval_using\tserved_max_min"
    );
    for ((kind, tc), recs) in &pools {
        if !kinds.contains(kind) || recs.len() < min_recs {
            continue;
        }
        // Deterministic stride sample.
        let sample: Vec<train::Rec> = if recs.len() > max_recs {
            (0..max_recs)
                .map(|i| recs[i * recs.len() / max_recs].clone())
                .collect()
        } else {
            recs.clone()
        };
        let alpha = train::alphabet(*kind, *tc);
        let mut kc = cfg.clone();
        kc.k = k_kind
            .get(&format!("{kind}/{}.{}.{}", tc[0], tc[1], tc[2]))
            .or_else(|| k_kind.get(&kind.to_string()))
            .copied()
            .unwrap_or(cfg.k);
        let t = train::train(*kind, alpha, &sample, &kc);
        let bytes: usize = t.seeds.iter().map(|s| s.len()).sum();
        let (vrecs, vs, vn) = match vals.as_ref().and_then(|v| v.get(&(*kind, *tc))) {
            Some(v) => {
                let (s, n) = train::evaluate(*kind, alpha, &t.seeds, v);
                (v.len(), s, n)
            }
            None => (0, 0.0, 0),
        };
        eprintln!(
            "{}\t{},{},{}\t{}\t{}\t{}\t{}\t{:.0}\t{}\t{}\t{:.0}\t{}\t{}/{}",
            KIND_NAMES[*kind as usize],
            tc[0],
            tc[1],
            tc[2],
            recs.len(),
            sample.len(),
            t.seeds.len(),
            bytes,
            t.saving,
            t.using,
            vrecs,
            vs,
            vn,
            t.served.first().unwrap_or(&0),
            t.served.last().unwrap_or(&0)
        );
        trained.push((*kind, *tc, t, recs.len(), sample.len()));
    }
    // The bank blob (layout: imij/src/seeds.rs, `Bank`): header, directory
    // of kinds in increasing (kind, tc), 4-byte aligned sections.
    trained.sort_by_key(|t| (t.0, t.1));
    let kinds = trained.len();
    let mut blob = Vec::new();
    blob.extend_from_slice(b"IMSB");
    blob.extend_from_slice(&[1, bank_id, kinds as u8, 0]);
    let mut at = 8 + 12 * kinds;
    let mut sections = Vec::new();
    for (kind, tc, t, _, _) in &trained {
        blob.extend_from_slice(&[*kind, tc[0], tc[1], tc[2]]);
        blob.extend_from_slice(&(t.seeds.len() as u32).to_le_bytes());
        blob.extend_from_slice(&(at as u32).to_le_bytes());
        let mut sec = Vec::new();
        let mut off = 0u32;
        sec.extend_from_slice(&off.to_le_bytes());
        for s in &t.seeds {
            off += s.len() as u32;
            sec.extend_from_slice(&off.to_le_bytes());
        }
        for s in &t.seeds {
            sec.extend_from_slice(s);
        }
        sec.resize(sec.len().next_multiple_of(4), 0);
        at += sec.len();
        sections.push(sec);
    }
    for sec in sections {
        blob.extend_from_slice(&sec);
    }
    std::fs::write(out_bin, &blob).unwrap();
    eprintln!("bank blob {} bytes", blob.len());
}

/// Readable text of a bank blob: header, then per kind one line per seed
/// (rANS: precision and `symbol:weight` of nonzero weights; Huffman:
/// `symbol:length`).
fn dump(args: &[String]) {
    let b = std::fs::read(&args[0]).unwrap();
    let u32at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize;
    assert_eq!(&b[..4], b"IMSB");
    let k = b[6] as usize;
    println!(
        "IMIJ seed bank {}, layout {}, {} kinds, {} bytes",
        b[5],
        b[4],
        k,
        b.len()
    );
    for e in 0..k {
        let d = 8 + 12 * e;
        let (kind, tc) = (b[d], [b[d + 1], b[d + 2], b[d + 3]]);
        let (n, at) = (u32at(d + 4), u32at(d + 8));
        let data = at + 4 * (n + 1);
        let bytes: usize = u32at(at + 4 * n);
        println!(
            "\n{} (k, m, depth) {:?}: {n} seeds, {bytes} bytes",
            KIND_NAMES[kind as usize], tc
        );
        let alpha = train::alphabet(kind, tc);
        for i in 0..n {
            let seed = &b[data + u32at(at + 4 * i)..data + u32at(at + 4 * (i + 1))];
            println!("{i:3}: {}", train::describe(kind, alpha, seed));
        }
    }
}

fn eval(args: &[String]) {
    let g = grid(args);
    let (w, h, px) = read_ppm();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let out = std::io::stdout();
    let mut o = std::io::BufWriter::new(out.lock());
    cells(&g, |color, mode, effort, q| {
        // The default encoder seeds from bank 1; `off` writes the files of an
        // encoder without seed tables.
        let mut on = config(color, mode, effort, q);
        if g.bank != 1 {
            on = on.tune_lossy(|t| t.lab.seed_bank = g.bank);
        }
        let off = on.clone().tune_lossy(|t| t.lab.seed_bank = 0);
        let t0 = std::time::Instant::now();
        let a = off
            .encode_samples(
                imij::ImageRef::new(&px, w as usize * 3, &info),
                &Metadata::default(),
            )
            .unwrap();
        let t1 = std::time::Instant::now();
        let (b, cap) = imij::seeds::capture(|| {
            on.encode_samples(
                imij::ImageRef::new(&px, w as usize * 3, &info),
                &Metadata::default(),
            )
            .unwrap()
        });
        let t2 = std::time::Instant::now();
        let (ea, ..) = imij::seeds::lossy_entropy_seed(&a).unwrap();
        let (eb, bank_id, scope) = imij::seeds::lossy_entropy_seed(&b).unwrap();
        if let Some(dir) = &g.write {
            let stem = format!("{dir}/{}-{mode}-e{effort}-q{q}", color_name(color));
            std::fs::write(format!("{stem}-off.imi"), &a).unwrap();
            std::fs::write(format!("{stem}-on.imi"), &b).unwrap();
        }
        let da = imij::decode(&a).unwrap().1;
        let db = imij::decode(&b).unwrap().1;
        let exact = (da == db) as u8;
        // Seeds the final file kept: blobs in the selected scope, AC
        // kinds of the entropy mode written (the tier codes both modes).
        let kept = |r: u8, k: u8| match r {
            0 => scope & 1 != 0 && if eb == 3 { k == 4 || k == 5 } else { k <= 1 },
            1 => scope & 2 != 0,
            _ => scope & 4 != 0,
        };
        let mut by: BTreeMap<(u8, u8), (u32, f64)> = BTreeMap::new();
        for s in cap.seeds.iter().filter(|s| kept(s.role, s.kind)) {
            let e = by.entry((s.role, s.kind)).or_default();
            e.0 += 1;
            e.1 += s.gain;
        }
        let seeds: Vec<String> = by
            .iter()
            .map(|((r, k), (n, gb))| format!("{r}.{k}:{n}:{gb:.0}"))
            .collect();
        writeln!(
            o,
            "E\t{}\t{mode}\t{effort}\t{q}\t{}\t{}\t{ea}\t{eb}\t{bank_id}\t{scope}\t{exact}\t{}\t{}\t{}",
            color_name(color),
            a.len(),
            b.len(),
            if seeds.is_empty() {
                "-".into()
            } else {
                seeds.join(",")
            },
            (t1 - t0).as_micros(),
            (t2 - t1).as_micros()
        )
        .unwrap();
    });
}

fn etime(args: &[String]) {
    let reps: usize = args[0].parse().unwrap();
    let g = grid(&args[1..]);
    let (w, h, px) = read_ppm();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    cells(&g, |color, mode, effort, q| {
        let off = config(color, mode, effort, q).tune_lossy(|t| t.lab.seed_bank = 0);
        let exact = off.clone().tune_lossy(|t| t.lab.seed_bank = g.bank);
        let est = exact.clone().tune_lossy(|t| {
            t.lab.seed_bank = g.bank;
            t.lab.seed_exact = false;
        });
        // Interleaved, minimum of each.
        let mut best = [u64::MAX; 3];
        let mut bytes = [0usize; 3];
        for _ in 0..reps {
            for (i, cfg) in [&off, &exact, &est].into_iter().enumerate() {
                let t = std::time::Instant::now();
                let f = cfg
                    .encode_samples(
                        imij::ImageRef::new(&px, w as usize * 3, &info),
                        &Metadata::default(),
                    )
                    .unwrap();
                best[i] = best[i].min(t.elapsed().as_nanos() as u64);
                bytes[i] = f.len();
            }
        }
        println!(
            "T\t{}\t{mode}\t{effort}\t{q}\t{}\t{}\t{}\t{}\t{}\t{}",
            color_name(color),
            best[0],
            best[1],
            best[2],
            bytes[0],
            bytes[1],
            bytes[2]
        );
    });
}

fn dtime(args: &[String]) {
    let file = std::fs::read(&args[0]).unwrap();
    let reps: usize = args[1].parse().unwrap();
    let (info, px) = imij::decode(&file).unwrap();
    let mut out = vec![0u8; px.len()];
    let stride = info.row_bytes();
    let mut t = Vec::with_capacity(reps);
    for _ in 0..reps {
        let s = std::time::Instant::now();
        imij::decode_into(&file, &mut out, stride).unwrap();
        t.push(s.elapsed().as_nanos() as u64);
    }
    assert_eq!(out, px);
    t.sort_unstable();
    println!("{}\t{}", t[0], t[t.len() / 2]);
}

/// llstat: lossless byte accounting of one PPM (stdin) encoded with the
/// default lossless config. Prints one line:
/// `L bytes container head models model_bits table_bits tables framing
/// states words unattributed stripes palette_stripes` where every byte of
/// the file is in exactly one of container (signature, header, chunk
/// framing), head (payload head fields except the model blob), models (the
/// bit-packed model blob; `model_bits` / `table_bits` its bits and the
/// weight tables' bits, `tables` their count), framing (stripe records
/// except the lane streams), states (rANS initial states, 4 bytes per lane)
/// and words (rANS words); unattributed should be 0.
fn llstat() {
    let (w, h, px) = read_ppm();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::default();
    let (file, cap) = imij::seeds::capture(|| {
        cfg.encode_samples(
            imij::ImageRef::new(&px, w as usize * 3, &info),
            &Metadata::default(),
        )
        .unwrap()
    });
    let (model_bits, table_bits, tables) = cap
        .groups
        .iter()
        .filter(|g| g.mode == 255)
        .fold((0, 0, 0), |a, g| {
            (a.0 + g.bits, a.1 + g.table_bits, a.2 + g.tables)
        });
    let doc: serde_json::Value =
        serde_json::from_str(&imij::lab::codestream_trace(&file)).expect("trace JSON");
    assert_eq!(doc["decode"], "ok", "trace decode");
    let nodes = doc["nodes"].as_array().unwrap();
    let kind = |i: usize| nodes[i]["kind"].as_str().unwrap_or("");
    let parent = |i: usize| nodes[i]["parent"].as_u64().map(|p| p as usize);
    let (mut container, mut head, mut models, mut framing, mut states, mut words) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut stripes, mut pal) = (0u64, 0u64);
    for (i, n) in nodes.iter().enumerate() {
        let label = n["label"].as_str().unwrap_or("");
        if kind(i) == "stripe" {
            stripes += 1;
        }
        if label == "palette entries - 1" {
            pal += 1;
        }
        if kind(i) != "field" {
            continue;
        }
        let len = n["end"].as_u64().unwrap() - n["start"].as_u64().unwrap();
        // Nearest enclosing head or stripe span.
        let mut p = parent(i);
        let mut under = "";
        while let Some(q) = p {
            if matches!(kind(q), "head" | "stripe") {
                under = kind(q);
                break;
            }
            p = parent(q);
        }
        match under {
            "head" if label == "entropy models (bit-packed)" => models += len,
            "head" => head += len,
            "stripe" if label == "initial state" => states += len,
            "stripe" if label.starts_with("words") => words += len,
            "stripe" => framing += len,
            _ => container += len,
        }
    }
    let unattributed: u64 = doc["checks"]["unattributed"]
        .as_array()
        .map(|v| {
            v.iter()
                .map(|r| r[1].as_u64().unwrap() - r[0].as_u64().unwrap())
                .sum()
        })
        .unwrap_or(0);
    let problems = doc["checks"]["problems"].as_array().map_or(0, |v| v.len());
    assert_eq!(problems, 0, "trace self-check problems");
    let total = file.len() as u64;
    assert_eq!(
        container + head + models + framing + states + words + unattributed,
        total,
        "every byte in one class"
    );
    println!(
        "L\t{total}\t{container}\t{head}\t{models}\t{model_bits}\t{table_bits}\t{tables}\t{framing}\t{states}\t{words}\t{unattributed}\t{stripes}\t{pal}"
    );
}

/// Lossless table records of one PPM (stdin), default lossless config,
/// explicit tables: `C ll bytes`, `G` and `T` lines as `collect` writes.
fn llcollect() {
    let (w, h, px) = read_ppm();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let cfg = imij::ImiEncoderConfig::default();
    let (bytes, cap) = imij::seeds::capture(|| {
        cfg.encode_samples(
            imij::ImageRef::new(&px, w as usize * 3, &info),
            &Metadata::default(),
        )
        .unwrap()
    });
    let out = std::io::stdout();
    let mut o = std::io::BufWriter::new(out.lock());
    writeln!(o, "C\tll\t{}", bytes.len()).unwrap();
    for gr in &cap.groups {
        writeln!(
            o,
            "G\t{}\t{}\t{}\t{}\t{}",
            gr.role, gr.mode, gr.bits, gr.table_bits, gr.tables
        )
        .unwrap();
    }
    for t in &cap.tables {
        let counts: Vec<String> = t
            .counts
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 0)
            .map(|(s, c)| format!("{s}:{c}"))
            .collect();
        writeln!(
            o,
            "T\t{}\t{}\t{},{},{}\t{}\t{:.1}\t{}",
            t.role,
            t.kind,
            t.tc[0],
            t.tc[1],
            t.tc[2],
            t.table_bits,
            t.data_bits,
            counts.join(",")
        )
        .unwrap();
    }
}

/// lleval [--bank N] [--write DIR] < image.ppm: the default lossless config
/// without seed tables and with bank N (default 2, registered from
/// IMIJ_SEED_BANK), exact choice. Both files must decode to the source.
/// Prints `LE bytes_off bytes_on seeded tables_seeded enc_us_off enc_us_on`.
fn lleval(args: &[String]) {
    let (mut bank, mut write) = (2u64, None::<String>);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let v = it.next().expect("flag value");
        match a.as_str() {
            "--bank" => bank = v.parse().unwrap(),
            "--write" => write = Some(v.clone()),
            _ => panic!("unknown flag {a}"),
        }
    }
    let (w, h, px) = read_ppm();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let off = imij::ImiEncoderConfig::default();
    let mut on = off.clone();
    on = on.tune_lossless(|t| {
        t.seed_bank = bank;
    });
    let enc = |cfg: &imij::ImiEncoderConfig| {
        let t = std::time::Instant::now();
        let f = cfg
            .encode_samples(
                imij::ImageRef::new(&px, w as usize * 3, &info),
                &Metadata::default(),
            )
            .unwrap();
        (f, t.elapsed().as_micros())
    };
    let (a, ta) = enc(&off);
    let ((b, tb), cap) = imij::seeds::capture(|| enc(&on));
    assert_eq!(
        imij::decode(&a).unwrap().1,
        px,
        "unseeded file decodes to the source"
    );
    assert_eq!(
        imij::decode(&b).unwrap().1,
        px,
        "seeded file decodes to the source"
    );
    assert!(b.len() <= a.len(), "seeds made the file larger");
    let seeded = (a != b) as u8;
    let tables = if seeded == 1 { cap.seeds.len() } else { 0 };
    if let Some(d) = &write {
        std::fs::write(format!("{d}/ll-off.imi"), &a).unwrap();
        std::fs::write(format!("{d}/ll-on.imi"), &b).unwrap();
    }
    println!(
        "LE\t{}\t{}\t{seeded}\t{tables}\t{ta}\t{tb}",
        a.len(),
        b.len()
    );
}

fn main() {
    // A lab seed bank (legacy asset or full-width envelope) for both paths.
    if let Ok(p) = std::env::var("IMIJ_SEED_BANK") {
        let id = imij::seeds::register_bank(std::fs::read(&p).unwrap()).unwrap();
        eprintln!("registered seed bank {id} from {p}");
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("collect") => collect(&args[1..]),
        Some("train") => train(&args[1..]),
        Some("eval") => eval(&args[1..]),
        Some("dtime") => dtime(&args[1..]),
        Some("etime") => etime(&args[1..]),
        Some("dump") => dump(&args[1..]),
        Some("llstat") => llstat(),
        Some("llcollect") => llcollect(),
        Some("lleval") => lleval(&args[1..]),
        _ => {
            eprintln!("usage: see the header of tools/imij-seeds/src/main.rs");
            std::process::exit(2);
        }
    }
}
