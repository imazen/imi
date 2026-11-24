//! In-process RD cells for rendition ladders (imazen-26), parallel over
//! renditions: `ladder-batch MANIFEST GRID OUT THREADS [STORE | -]`.
//!
//! MANIFEST: tab-separated `path sha256 class split id` per rendition (an
//! RGB8 PNG at its native size, no crop). GRID: a JSON array of cell
//! configurations, validated like `encode-rgb`'s. Every (rendition, config)
//! cell is encoded (IMI with one encoder thread; its bytes do not depend on
//! the thread count), decoded with the cell's own decoder and scored with
//! the same metrics and settings as `score-rgb` (CVVDP FHD, SSIMULACRA2,
//! Butteraugli max/3-norm, still VMAF). A metric that cannot score a size
//! (CVVDP or VMAF on the smallest renditions) is written as NA. One row per
//! cell is appended to OUT (TSV) as each rendition completes; cells already
//! in OUT (same PNG sha256 and config) are skipped, so a run resumes.
//! Encoded files are kept content-addressed as STORE/<sha256>.<ext>, so a
//! later metric can re-score a study without re-encoding (the rule that every
//! sweep persists its encoded variants). STORE defaults to `encoded/` beside
//! OUT; `-` keeps nothing (tests and throwaway runs only).
//! Cells of one rendition that decode to the same pixels (e.g. IMI efforts
//! below the trellis) reuse that decode's scores.
use crate::quality::perceptual;
use crate::rd::{
    Result, center_crop, check_rgb, decode_cell, encode_cell, hash, input_key, validate_config,
};
use rayon::prelude::*;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

/// A typed or cropped cell source: (rendition path, input kind, crop).
type SourceKey = (String, String, Option<u64>);
/// Converted pixels of one source and their dimensions.
type SourcePixels = (Vec<u8>, u32, u32);

const HEADER: &str = "rendition\tpng_sha256\tclass\tsplit\tid\twidth\theight\tconfig\tencoded_bytes\tencoded_sha256\tdecoded_sha256\tencode_seconds\tssimulacra2\tbutteraugli_max\tbutteraugli_p3\tcvvdp_fhd_jod\tvmaf_v1_fhd";

struct Entry {
    path: String,
    sha256: String,
    class: String,
    split: String,
    id: String,
}

/// The metrics of `quality::WarmScorer`, each optional.
struct Scorer {
    w: u32,
    h: u32,
    cv: Option<cvvdp::Cvvdp>,
    ssim: fast_ssim2::Ssimulacra2Reference,
    ba: butteraugli::ButteraugliReference,
    original: Vec<u8>,
}

impl Scorer {
    fn new(original: Vec<u8>, w: u32, h: u32) -> Result<Self> {
        let cv = perceptual::cvvdp_fhd(w, h).ok().and_then(|mut cv| {
            cv.warm_reference(&original).ok()?;
            Some(cv)
        });
        let source = fast_ssim2::PixelSlice::new(
            &original,
            w,
            h,
            w as usize * 3,
            fast_ssim2::PixelDescriptor::RGB8_SRGB,
        )?;
        let ssim = fast_ssim2::Ssimulacra2Reference::new(&source)?;
        let ba = butteraugli::ButteraugliReference::new(
            &original,
            w as usize,
            h as usize,
            butteraugli::ButteraugliParams::default().with_compute_diffmap(true),
        )?;
        Ok(Self {
            w,
            h,
            cv,
            ssim,
            ba,
            original,
        })
    }

    /// [ssimulacra2, butteraugli max, butteraugli 3-norm, CVVDP JOD, VMAF].
    fn score(&mut self, decoded: &[u8]) -> Result<[Option<f64>; 5]> {
        let (w, h) = (self.w, self.h);
        let mut map = Vec::new();
        let cv = match self.cv.as_mut() {
            Some(cv) => cv
                .score_with_warm_ref_diffmap(decoded, &mut map)
                .ok()
                .map(f64::from),
            None => None,
        };
        let vmaf = perceptual::vmaf_still(&self.original, decoded, w as usize, h as usize)
            .ok()
            .flatten();
        let distorted = fast_ssim2::PixelSlice::new(
            decoded,
            w,
            h,
            w as usize * 3,
            fast_ssim2::PixelDescriptor::RGB8_SRGB,
        )?;
        let ssim2 = self.ssim.compare(&distorted)?;
        let ba = self.ba.compare(decoded)?;
        Ok([Some(ssim2), Some(ba.score), Some(ba.pnorm_3), cv, vmaf])
    }
}

/// A decoded source PNG in its native layout. The byte-ladder PNGs are
/// 8-bit RGB; the split-bound typed cells (2026-10-10) also take 8-bit
/// gray/RGBA and 16-bit RGB PNGs.
struct PngSource {
    bytes: Vec<u8>,
    w: u32,
    h: u32,
    ch: usize,
    bps: usize,
}

fn load_png_source(entry: &Entry) -> Result<PngSource> {
    let data = std::fs::read(&entry.path)?;
    if hash(&data) != entry.sha256 {
        return Err(format!("{}: PNG sha256 differs from the manifest", entry.path).into());
    }
    let image = zenpng::decode(
        &data,
        &zenpng::PngDecodeConfig::default(),
        &enough::Unstoppable,
    )?;
    let (w, h) = (image.info.width, image.info.height);
    let bytes = image.pixels.copy_to_contiguous_bytes();
    let bps = match image.info.bit_depth {
        8 => 1,
        16 => 2,
        d => return Err(format!("{}: {d}-bit PNG", entry.path).into()),
    };
    if w == 0 || h == 0 || bytes.len() % (w as usize * h as usize) != 0 {
        return Err(format!("{}: ragged PNG bytes", entry.path).into());
    }
    let ch = bytes.len() / (w as usize * h as usize) / bps;
    if !matches!(ch, 1..=4) {
        return Err(format!("{}: {ch}-channel PNG", entry.path).into());
    }
    Ok(PngSource {
        bytes,
        w,
        h,
        ch,
        bps,
    })
}

/// One cell's bytes: the source PNG converted to the cell's `input` layout
/// and cropped to `crop` when present. Conventions (split-bound study;
/// README "Channel and sample types"): "gray" takes a gray source as-is or
/// the BT.709 luma of an RGB8 one, rounded to nearest; "rgba" takes an RGBA
/// source as-is or appends alpha = 255 - luma to an RGB8 one; "f32" is the
/// PQ EOTF of a 16-bit PQ source on the production transfer's linear scale
/// (`Transfer::Pq`; 1.0 = 203 nits, 49.25 at code 65535).
fn cell_source<'a>(src: &'a PngSource, cfg: &Value) -> Result<(Cow<'a, [u8]>, u32, u32)> {
    let n = cfg.get("crop").and_then(Value::as_u64).map(|n| n as u32);
    let cropped = match n {
        Some(n) => Some(center_crop(&src.bytes, src.w, src.h, src.ch, src.bps, n)?),
        None => None,
    };
    let (w, h) = match n {
        Some(n) => (n, n),
        None => (src.w, src.h),
    };
    let base: &[u8] = cropped.as_deref().unwrap_or(&src.bytes);
    let need = |what: &str| -> Box<dyn std::error::Error> {
        format!(
            "input {what} needs a {}-bit {}-channel source PNG",
            src.bps * 8,
            src.ch
        )
        .into()
    };
    // `None`: the (possibly cropped) source bytes pass through unchanged.
    let converted: Option<Vec<u8>> = match input_key(cfg)? {
        "rgb8" => {
            check_rgb(base, w, h)?;
            None
        }
        "gray" => {
            if src.ch == 2 && src.bps == 1 {
                return Err("gray input takes no alpha PNGs".into());
            }
            if src.ch != 1 && !(src.ch == 3 && src.bps == 1) {
                return Err(need("gray"));
            }
            (src.ch == 3).then(|| {
                let mut out = Vec::with_capacity((w * h) as usize);
                for px in base.as_chunks::<3>().0 {
                    out.push(luma(px));
                }
                out
            })
        }
        "rgba" => {
            if src.ch == 4 {
                None
            } else {
                if !(src.ch == 3 && src.bps == 1) {
                    return Err(need("rgba"));
                }
                let mut out = Vec::with_capacity((w * h * 4) as usize);
                for px in base.as_chunks::<3>().0 {
                    out.extend_from_slice(&[px[0], px[1], px[2], 255 - luma(px)]);
                }
                Some(out)
            }
        }
        "u16" => {
            if !(src.ch == 3 && src.bps == 2) {
                return Err(need("u16"));
            }
            None
        }
        "f32" => {
            if !(src.ch == 3 && src.bps == 2) {
                return Err(need("f32"));
            }
            if cfg.get("transfer").and_then(Value::as_str) != Some("pq") {
                return Err("f32 input requires transfer \"pq\"".into());
            }
            let mut out = Vec::with_capacity((w * h * 12) as usize);
            for p in base.as_chunks::<2>().0 {
                out.extend_from_slice(&pq_linear(u16::from_ne_bytes(*p)).to_ne_bytes());
            }
            Some(out)
        }
        other => return Err(format!("unknown input layout {other:?}").into()),
    };
    let out = match converted {
        Some(v) => Cow::Owned(v),
        None => match cropped {
            Some(c) => Cow::Owned(c),
            None => Cow::Borrowed(src.bytes.as_slice()),
        },
    };
    Ok((out, w, h))
}

/// BT.709 luma of 8-bit code values, rounded to nearest.
fn luma(px: &[u8; 3]) -> u8 {
    (0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// SMPTE ST 2084 EOTF on the production transfer's linear scale
/// (`imij/src/lossy/transfer.rs`, `Transfer::Pq`, same constants: 1.0 =
/// about 203 nits, 49.25 at code 65535).
fn pq_linear(v: u16) -> f32 {
    const M1: f64 = 0.159_301_757_812_5;
    const M2: f64 = 78.843_75;
    const C1: f64 = 0.835_937_5;
    const C2: f64 = 18.851_562_5;
    const C3: f64 = 18.687_5;
    const PEAK: f64 = 49.25;
    let e = (v as f64 / 65535.0).clamp(0.0, 1.0);
    let p = e.powf(1.0 / M2);
    let num = (p - C1).max(0.0);
    ((num / (C2 - C3 * p)).powf(1.0 / M1) * PEAK) as f32
}

/// Compact JSON with sorted keys (serde_json's default map is ordered).
fn canonical(cfg: &Value) -> Result<String> {
    Ok(serde_json::to_string(cfg)?)
}

fn rendition(
    entry: &Entry,
    grid: &[(Value, String)],
    done: &HashSet<(String, String)>,
    store: Option<&Path>,
    table_stats: Option<&Mutex<std::fs::File>>,
    side_stats: Option<&Mutex<std::fs::File>>,
) -> Result<String> {
    let todo: Vec<&(Value, String)> = grid
        .iter()
        .filter(|(_, key)| !done.contains(&(entry.sha256.clone(), key.clone())))
        .collect();
    if todo.is_empty() {
        return Ok(String::new());
    }
    let src = load_png_source(entry)?;
    // The split-revisit study (2026-10-10): `LADDER_SKIP_METRICS=1` skips the perceptual
    // scorers (their columns are NA) and still decodes every cell, so the
    // decoded sha256 still proves pixel identity across configurations.
    // Typed or cropped cells (split-bound study) have no RGB8 reference at
    // the cell's dimensions and skip them too.
    let typed =
        |cfg: &Value| input_key(cfg).is_ok_and(|i| i != "rgb8") || cfg.get("crop").is_some();
    let mut scorer = if std::env::var_os("LADDER_SKIP_METRICS").is_some()
        || todo.iter().any(|(cfg, _)| typed(cfg))
    {
        None
    } else {
        Some(Scorer::new(src.bytes.clone(), src.w, src.h)?)
    };
    let name = Path::new(&entry.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut rows = String::new();
    let na = |v: Option<f64>| v.map_or("NA".to_string(), |v| format!("{v}"));
    let mut scored: std::collections::HashMap<String, [Option<f64>; 5]> = Default::default();
    // Typed or cropped cell sources repeat heavily across a grid (same
    // input, crop and dimensions); convert and keep them once per rendition.
    let mut source_cache: std::collections::HashMap<SourceKey, SourcePixels> = Default::default();
    for (cfg, key) in todo {
        let typed_cell = typed(cfg);
        let owned;
        let (raw, w, h): (&[u8], u32, u32) = if typed_cell {
            let ck = (
                input_key(cfg)?.to_owned(),
                cfg.get("transfer")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                cfg.get("crop").and_then(Value::as_u64),
            );
            if !source_cache.contains_key(&ck) {
                let (v, w, h) = cell_source(&src, cfg)?;
                source_cache.insert(ck.clone(), (v.into_owned(), w, h));
            }
            let (v, w, h) = &source_cache[&ck];
            (v.as_slice(), *w, *h)
        } else {
            let (v, w, h) = cell_source(&src, cfg)?;
            match v {
                Cow::Borrowed(b) => (b, w, h),
                Cow::Owned(b) => {
                    owned = b;
                    (&owned[..], w, h)
                }
            }
        };
        let start = Instant::now();
        let (data, _) = encode_cell(cfg, raw, w, h, 1)?;
        let seconds = start.elapsed().as_secs_f64();
        // IMIJ_LADDER_TABLE_STATS=PATH: the AC entropy-table bytes the encode
        // just wrote (split-bound study, 2026-10-10), one JSON line per IMI
        // cell keyed like the OUT row. Read on the worker that encoded, so
        // concurrent renditions do not mix; do not combine with
        // IMIJ_CELL_SIDE_STATS (that path consumes the same reading).
        if cfg["codec"] == "imij"
            && let Some(f) = table_stats
        {
            let [mode, table_bytes] = imij::lab::lossy_table_stats();
            let line = serde_json::json!({
                "png_sha256": entry.sha256,
                "config": key,
                "mode": mode,
                "ac_table_bytes": table_bytes,
            });
            let mut f = f.lock().unwrap();
            writeln!(f, "{line}").unwrap();
            f.flush().unwrap();
        }
        // IMIJ_LADDER_SIDE_STATS=PATH: the encode's entropy-work counts
        // (split-bound study, 2026-10-10) as JSON lines keyed like the OUT
        // row. These counters are process-global last-writer-wins, so this
        // env requires `THREADS` 1.
        if cfg["codec"] == "imij"
            && let Some(f) = side_stats
        {
            let [tokens, groups, coded_bc] = imij::lab::lossy_side_stats();
            let line = serde_json::json!({
                "png_sha256": entry.sha256,
                "config": key,
                "tokens": tokens,
                "groups": groups,
                "coded_bc": coded_bc,
            });
            let mut f = f.lock().unwrap();
            writeln!(f, "{line}").unwrap();
            f.flush().unwrap();
        }
        let encoded_sha256 = hash(&data);
        if let Some(dir) = store {
            let ext = match cfg["codec"].as_str() {
                Some("imij") => "imi",
                Some(c) => c,
                None => "bin",
            };
            let path = dir.join(format!("{encoded_sha256}.{ext}"));
            if !path.exists() {
                let tmp = dir.join(format!("{encoded_sha256}.{ext}.tmp{}", std::process::id()));
                std::fs::write(&tmp, &data)?;
                std::fs::rename(&tmp, &path)?;
            }
        }
        // IMIJ_LADDER_AQ_COST=PATH: the AQ channel's cost under alternative
        // codings (steer.rs `aq-cost`) for every IMI file that has one, as
        // JSON lines keyed by source and config (no file is stored).
        if cfg["codec"] == "imij"
            && let Some(path) = std::env::var_os("IMIJ_LADDER_AQ_COST")
            && let Some(mut row) = crate::steer::aq_cost_row(&data)?
        {
            row["png_sha256"] = serde_json::json!(entry.sha256);
            row["config"] = serde_json::json!(key);
            row["encoded_sha256"] = serde_json::json!(encoded_sha256);
            let line = format!("{row}\n");
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?;
            std::io::Write::write_all(&mut f, line.as_bytes())?;
        }
        let decoded = decode_cell(cfg, &data, w, h)?;
        let decoded_sha256 = hash(&decoded);
        let [ssim2, bamax, bap3, cv, vmaf] = match scorer.as_mut() {
            None => [None; 5],
            Some(scorer) => match scored.get(&decoded_sha256) {
                Some(s) => *s,
                None => {
                    let s = scorer.score(&decoded)?;
                    scored.insert(decoded_sha256.clone(), s);
                    s
                }
            },
        };
        rows.push_str(&format!(
            "{name}\t{}\t{}\t{}\t{}\t{w}\t{h}\t{key}\t{}\t{encoded_sha256}\t{}\t{seconds:.6}\t{}\t{}\t{}\t{}\t{}\n",
            entry.sha256,
            entry.class,
            entry.split,
            entry.id,
            data.len(),
            decoded_sha256,
            na(ssim2),
            na(bamax),
            na(bap3),
            na(cv),
            na(vmaf),
        ));
    }
    Ok(rows)
}

pub(crate) fn command(a: &[String]) -> Result<()> {
    if a.len() != 6 && a.len() != 7 {
        return Err("ladder-batch MANIFEST GRID OUT THREADS [STORE | -]".into());
    }
    let entries: Vec<Entry> = std::fs::read_to_string(&a[2])?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() != 5 {
                return Err(format!("manifest line needs 5 fields: {l}").into());
            }
            Ok(Entry {
                path: f[0].into(),
                sha256: f[1].into(),
                class: f[2].into(),
                split: f[3].into(),
                id: f[4].into(),
            })
        })
        .collect::<Result<_>>()?;
    let grid: Vec<Value> = serde_json::from_slice(&std::fs::read(&a[3])?)?;
    let mut keyed = Vec::with_capacity(grid.len());
    for cfg in grid {
        validate_config(&cfg)?;
        let key = canonical(&cfg)?;
        if key.contains('\t') || keyed.iter().any(|(_, k)| *k == key) {
            return Err("duplicate or tab-containing config".into());
        }
        keyed.push((cfg, key));
    }
    let out_path = Path::new(&a[4]);
    let threads: usize = a[5].parse()?;
    let store_dir = match a.get(6).map(String::as_str) {
        Some("-") => {
            eprintln!("ladder-batch: STORE -: encoded bytes are not kept");
            None
        }
        Some(dir) => Some(std::path::PathBuf::from(dir)),
        None => Some(out_path.parent().unwrap_or(Path::new(".")).join("encoded")),
    };
    let store = store_dir.as_deref();
    if let Some(dir) = store {
        std::fs::create_dir_all(dir)?;
    }
    // IMIJ_LADDER_TABLE_STATS=PATH: per-cell AC entropy-table bytes of every
    // IMI encode, as JSON lines (append; keys repeat on a re-run, take the
    // last). Needs IMI's `lab` feature, which this crate enables.
    let stats = match std::env::var_os("IMIJ_LADDER_TABLE_STATS") {
        Some(p) => Some(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)?,
        )),
        None => None,
    };
    let side = match std::env::var_os("IMIJ_LADDER_SIDE_STATS") {
        Some(p) => Some(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)?,
        )),
        None => None,
    };
    let mut done = HashSet::new();
    if out_path.exists() {
        let text = std::fs::read_to_string(out_path)?;
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return Err("existing OUT has another header".into());
        }
        for line in lines {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != HEADER.split('\t').count() {
                return Err("truncated row in OUT; inspect before resuming".into());
            }
            done.insert((f[1].to_string(), f[7].to_string()));
        }
    } else {
        std::fs::write(out_path, format!("{HEADER}\n"))?;
    }
    let out = Mutex::new(std::fs::OpenOptions::new().append(true).open(out_path)?);
    let failures = Mutex::new(Vec::new());
    let completed = std::sync::atomic::AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;
    pool.install(|| {
        entries.par_iter().for_each(|entry| {
            match rendition(entry, &keyed, &done, store, stats.as_ref(), side.as_ref()) {
                Ok(rows) => {
                    let mut f = out.lock().unwrap();
                    f.write_all(rows.as_bytes()).unwrap();
                    f.flush().unwrap();
                }
                Err(e) => failures
                    .lock()
                    .unwrap()
                    .push(format!("{}: {e}", entry.path)),
            }
            let n = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            eprintln!("[{n}/{}] {}", entries.len(), entry.path);
        })
    });
    let failures = failures.into_inner().unwrap();
    for f in &failures {
        eprintln!("failed {f}");
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("{} failed renditions", failures.len()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png_source(bytes: Vec<u8>, w: u32, h: u32, ch: usize, bps: usize) -> PngSource {
        PngSource {
            bytes,
            w,
            h,
            ch,
            bps,
        }
    }

    /// A gradient RGB8 source of `w` x `h`.
    fn rgb8(w: u32, h: u32) -> PngSource {
        let mut bytes = Vec::new();
        for y in 0..h {
            for x in 0..w {
                bytes.extend_from_slice(&[(x * 3) as u8, (y * 5) as u8, 200]);
            }
        }
        png_source(bytes, w, h, 3, 1)
    }

    #[test]
    fn center_crop_takes_the_centered_window() {
        let src = rgb8(4, 4);
        let out = center_crop(&src.bytes, 4, 4, 3, 1, 2).unwrap();
        // Rows 1 and 2, columns 1 and 2.
        assert_eq!(out, vec![3, 5, 200, 6, 5, 200, 3, 10, 200, 6, 10, 200]);
        assert!(center_crop(&src.bytes, 4, 4, 3, 1, 5).is_err());
        assert!(center_crop(&src.bytes, 4, 4, 3, 1, 0).is_err());
    }

    #[test]
    fn gray_and_rgba_convert_deterministically() {
        let src = rgb8(2, 1); // (0,0,200) and (3,0,200)
        let gray = cell_source(&src, &json!({"input":"gray"})).unwrap();
        assert_eq!(gray.0.as_ref(), &[luma(&[0, 0, 200]), luma(&[3, 0, 200])]);
        assert_eq!(gray.0[0], 14); // 0.0722 * 200 = 14.44 -> 14
        let rgba = cell_source(&src, &json!({"input":"rgba"})).unwrap();
        assert_eq!(rgba.0.len(), 8);
        assert_eq!(&rgba.0[..4], &[0, 0, 200, 255 - 14]);
        assert_eq!(&rgba.0[4..], &[3, 0, 200, 255 - luma(&[3, 0, 200])]);
        // A gray source passes through as gray.
        let g = png_source(vec![7, 9], 2, 1, 1, 1);
        assert_eq!(
            cell_source(&g, &json!({"input":"gray"}))
                .unwrap()
                .0
                .as_ref(),
            &[7, 9]
        );
        // An RGB source asked for `u16` is rejected.
        assert!(cell_source(&src, &json!({"input":"u16"})).is_err());
    }

    #[test]
    fn f32_is_the_pq_eotf_on_the_production_scale() {
        assert_eq!(pq_linear(0), 0.0);
        assert!((pq_linear(65535) - 49.25).abs() < 1e-3);
        assert!((pq_linear(32768) - 0.454_344_85).abs() < 1e-5);
        assert!(pq_linear(16384) > 0.0 && pq_linear(16384) < pq_linear(32768));
        // 16-bit RGB with transfer pq converts; without it, it is refused.
        let mut bytes = Vec::new();
        for v in [0u16, 16384, 32768, 65535] {
            bytes.extend_from_slice(&v.to_ne_bytes());
            bytes.extend_from_slice(&v.to_ne_bytes());
            bytes.extend_from_slice(&v.to_ne_bytes());
        }
        let src = png_source(bytes, 4, 1, 3, 2);
        let out = cell_source(&src, &json!({"input":"f32","transfer":"pq"})).unwrap();
        assert_eq!(out.0.len(), 4 * 3 * 4);
        let v: Vec<f32> = out
            .0
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_ne_bytes(*b))
            .collect();
        assert_eq!(v[0], 0.0);
        assert_eq!(v[11], pq_linear(65535));
        assert!((v[11] - 49.25).abs() < 1e-3);
        assert!(cell_source(&src, &json!({"input":"f32"})).is_err());
    }
}
