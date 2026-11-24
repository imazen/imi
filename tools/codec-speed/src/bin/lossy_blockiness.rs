//! Lossy blockiness diagnosis (2026-10-17): encodes a real-image crop the way the
//! explorer's tile archives are built (512x256 independent IMI cells), and as one
//! continuous image, over a small set of settings; decodes natively; writes the
//! decoded rasters and a JSON line per setting with an explicit boundary-error
//! diagnostic. The diagnostic is a measurement of step excess at 8x8 block and
//! tile boundaries, not a perceptual metric: perceptual scores come from
//! `imij-cmp score-rgb` on the files this tool writes.
//!
//! encode CROP.rgb W H OUTDIR LABEL [SET]   run a setting set (default "main")
//! jpeg CROP.rgb W H OUTDIR LABEL           zenjpeg 4:2:0 and 4:4:4 quality ladder on one
//!                                          continuous image (context at matched quality)
//! edges SRC.rgb DEC.rgb W H [TW TH]        boundary diagnostic for two rasters
//! decode FILE.imi|FILE.imijset...         SHA-256 of the decoded pixels of every cell
//!                                          (run under IMIJ_SCALAR / IMIJ_NO_AVX512 for
//!                                          decoder tier parity on identical bytes)
//! deployed INDEX.bin CROP.rgb W H X0 Y0 Q  compare re-encoded tiles of a tile-aligned
//!                                          crop (origin X0,Y0 in the full image)
//!                                          with the published index's decoded hashes
use imij::{
    Channels, DecodeSpeed, ImageInfo, ImiEncoderConfig, LossyAqField, LossyColor, LossyQuality,
    Metadata, SampleType,
};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const TW: u32 = 512;
const TH: u32 = 256;

fn info(w: u32, h: u32) -> ImageInfo {
    ImageInfo::new(w, h, Channels::Rgb, SampleType::U8)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn fresh(p: impl AsRef<Path>) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(p)
}

#[derive(Clone, Copy)]
enum Q {
    Jpeg(f32),
    Scale(f32),
}
impl Q {
    fn quality(self) -> LossyQuality {
        match self {
            Q::Jpeg(q) => LossyQuality::JpegQuality(q),
            Q::Scale(s) => LossyQuality::Scale(s),
        }
    }
}

#[derive(Clone)]
struct Setting {
    id: String,
    tiled: bool,
    lossless: bool,
    q: Q,
    effort: f32,
    tier: Option<DecodeSpeed>,
    color: Option<LossyColor>,
    dering: Option<bool>,
    dc_nearest: Option<bool>,
    fdct_dense: bool,
    /// DC step multipliers in 1/64 units, luma then chroma (default XYB 48/48).
    dc_step: Option<[u8; 2]>,
    aq_gamma: f32,
    aq_field: Option<LossyAqField>,
}
impl Setting {
    /// The configuration `nasa_pyramid` uses for explorer tiles.
    fn production(q: f32, tiled: bool) -> Self {
        Self {
            id: format!("{}-q{q:.0}", if tiled { "tile" } else { "cont" }),
            tiled,
            lossless: false,
            q: Q::Jpeg(q),
            effort: 10.0,
            tier: Some(DecodeSpeed::Fast),
            color: None,
            dering: None,
            dc_nearest: None,
            fdct_dense: false,
            dc_step: None,
            aq_gamma: 0.0,
            aq_field: None,
        }
    }
    fn named(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }
    fn lossy_config(&self) -> ImiEncoderConfig {
        let mut cfg = imij::ImiEncoderConfig::lossy(90.0)
            .with_quality(self.q.quality())
            .with_effort(self.effort)
            .with_threads(1);
        if let Some(t) = self.tier {
            cfg = cfg.with_decode_speed(t);
        }
        if let Some(c) = self.color {
            cfg = cfg.tune_lossy(|t| t.color = c);
        }
        cfg = cfg.tune_lossy(|t| t.lab.switches.dering = self.dering);
        cfg = cfg.tune_lossy(|t| t.lab.switches.dc_nearest = self.dc_nearest);
        cfg = cfg.tune_lossy(|t| t.lab.switches.dc_step = self.dc_step);
        cfg = cfg.tune_lossy(|t| {
            t.aq_gamma = self.aq_gamma;
        });
        cfg = cfg.tune_lossy(|t| {
            t.lab.aq_field = self.aq_field;
        });
        cfg
    }
}

fn settings(set: &str) -> Result<Vec<Setting>> {
    let p = Setting::production;
    let mut v = Vec::new();
    match set {
        // Production tiles and the same pixels encoded as one image, three qualities,
        // plus the lossless control.
        "main" => {
            for q in [50.0, 70.0, 90.0] {
                v.push(p(q, true));
                v.push(p(q, false));
            }
            let mut l = p(70.0, true).named("tile-lossless");
            l.lossless = true;
            v.push(l);
        }
        // Single-factor ablations at JPEG-quality 70 on tiles.
        "ablate" => {
            let base = || p(70.0, true);
            let mut s = base();
            s.effort = 0.0;
            v.push(s.named("tile-q70-effort0"));
            let mut s = base();
            s.effort = 7.5;
            v.push(s.named("tile-q70-effort7.5"));
            let mut s = base();
            s.tier = Some(DecodeSpeed::Balanced);
            v.push(s.named("tile-q70-balanced"));
            let mut s = base();
            s.tier = Some(DecodeSpeed::Compact);
            v.push(s.named("tile-q70-compact"));
            let mut s = base();
            s.tier = None;
            s.color = Some(LossyColor::YCoCg);
            s.effort = 7.5;
            v.push(s.named("tile-q70-ycocg-effort7.5"));
            let mut s = base();
            s.dering = Some(false);
            v.push(s.named("tile-q70-dering-off"));
            let mut s = base();
            s.dc_nearest = Some(false);
            v.push(s.named("tile-q70-dc-deadzone"));
            let mut s = base();
            s.fdct_dense = true;
            v.push(s.named("tile-q70-fdct-dense"));
        }
        // Transform-arithmetic control: the finest quantizers (scale 116 is the maximum),
        // where a wrong normalization or rounding would show as a large error.
        "fine" => {
            for (id, s) in [("tile-scale116", 116.0), ("tile-scale100", 100.0)] {
                let mut x = p(70.0, true).named(id);
                x.q = Q::Scale(s);
                v.push(x);
            }
        }
        // Encoder-side candidates against block steps on flat areas, each at three
        // qualities so they can be compared with the production curve at matched rate.
        "cand" => {
            let field = |strength: f32| LossyAqField {
                strength,
                levels: 4,
                cell: 1,
                relative: true,
                fade: true,
            };
            for q in [60.0, 70.0, 80.0] {
                for (name, f) in [
                    (
                        "dc32",
                        (|s: &mut Setting| s.dc_step = Some([32, 32])) as fn(&mut Setting),
                    ),
                    ("dc16", |s| s.dc_step = Some([16, 16])),
                    ("aqact0.5", |s| s.aq_gamma = 0.5),
                    ("aqact1.0", |s| s.aq_gamma = 1.0),
                ] {
                    let mut x = p(q, true);
                    f(&mut x);
                    v.push(x.named(&format!("cand-{name}-q{q:.0}")));
                }
                for strength in [0.5, 1.0] {
                    let mut x = p(q, true);
                    x.aq_field = Some(field(strength));
                    v.push(x.named(&format!("cand-aqfield{strength}-q{q:.0}")));
                }
            }
        }
        // Quality ladder for matching decode quality against other codecs.
        "ladder" => {
            for q in [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 95.0] {
                v.push(p(q, true));
            }
        }
        _ => return Err("setting set: main | ablate | fine | ladder | cand".into()),
    }
    Ok(v)
}

struct Encoded {
    /// One entry per cell (a single entry when continuous).
    cells: Vec<Vec<u8>>,
    decoded: Vec<u8>,
}

fn encode_decode(raw: &[u8], w: u32, h: u32, s: &Setting) -> Result<Encoded> {
    let (cw_max, ch_max) = if s.tiled { (TW, TH) } else { (w, h) };
    let (cols, rows) = (w.div_ceil(cw_max), h.div_ceil(ch_max));
    let cfg = s.lossy_config();
    let jobs: Vec<(u32, u32)> = (0..rows)
        .flat_map(|r| (0..cols).map(move |c| (c, r)))
        .collect();
    let cells: Vec<(Vec<u8>, Vec<u8>, u32, u32, u32, u32)> = jobs
        .par_iter()
        .map(|&(c, r)| -> std::result::Result<_, imij::Error> {
            let (x, y) = (c * cw_max, r * ch_max);
            let (cw, chh) = ((w - x).min(cw_max), (h - y).min(ch_max));
            let stride = cw as usize * 3;
            let mut tile = vec![0u8; stride * chh as usize];
            for yy in 0..chh as usize {
                let src = ((y as usize + yy) * w as usize + x as usize) * 3;
                tile[yy * stride..(yy + 1) * stride].copy_from_slice(&raw[src..src + stride]);
            }
            let encoded = if s.lossless {
                let mut e = imij::ImiEncoderConfig::default();
                e = e.with_effort(2.0);
                e = e.with_threads(1);
                e.encode_samples(
                    imij::ImageRef::new(&tile, stride, &info(cw, chh)),
                    &Metadata::default(),
                )?
            } else {
                cfg.encode_samples(
                    imij::ImageRef::new(&tile, stride, &info(cw, chh)),
                    &Metadata::default(),
                )?
            };
            let (di, dec) = imij::decode(&encoded)?;
            if di != info(cw, chh) {
                return Err(imij::Error::InvalidInput("tile geometry"));
            }
            Ok((encoded, dec, x, y, cw, chh))
        })
        .collect::<std::result::Result<_, imij::Error>>()?;
    let mut decoded = vec![0u8; raw.len()];
    let mut out = Vec::with_capacity(cells.len());
    for (encoded, dec, x, y, cw, chh) in cells {
        for yy in 0..chh as usize {
            let d = ((y as usize + yy) * w as usize + x as usize) * 3;
            let n = cw as usize * 3;
            decoded[d..d + n].copy_from_slice(&dec[yy * n..(yy + 1) * n]);
        }
        out.push(encoded);
    }
    Ok(Encoded {
        cells: out,
        decoded,
    })
}

/// B-Y (`blue`) or R-Y chroma difference planes on 8-bit code values, same luma weights.
fn chroma(rgb: &[u8], blue: bool) -> Vec<f32> {
    rgb.chunks_exact(3)
        .map(|p| {
            let y = 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
            if blue {
                p[2] as f32 - y
            } else {
                p[0] as f32 - y
            }
        })
        .collect()
}

fn luma(rgb: &[u8]) -> Vec<f32> {
    rgb.chunks_exact(3)
        .map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32)
        .collect()
}

/// Per-position step excess along one axis: the mean over the orthogonal axis of
/// `|decoded step| - |source step|` between sample n-1 and n (luma, 8-bit code values).
fn step_excess(s: &[f32], d: &[f32], w: usize, h: usize, horizontal: bool) -> Vec<f64> {
    let n = if horizontal { w } else { h };
    let m = if horizontal { h } else { w };
    let at = |p: &[f32], i: usize, j: usize| {
        if horizontal {
            p[j * w + i]
        } else {
            p[i * w + j]
        }
    };
    (1..n)
        .map(|i| {
            let mut sum = 0.0f64;
            for j in 0..m {
                let ds = (at(s, i, j) - at(s, i - 1, j)).abs();
                let dd = (at(d, i, j) - at(d, i - 1, j)).abs();
                sum += (dd - ds) as f64;
            }
            sum / m as f64
        })
        .collect()
}

/// Mean absolute luma error per position index (column or row).
fn error_profile(s: &[f32], d: &[f32], w: usize, h: usize, horizontal: bool) -> Vec<f64> {
    let n = if horizontal { w } else { h };
    let m = if horizontal { h } else { w };
    (0..n)
        .map(|i| {
            let mut sum = 0.0f64;
            for j in 0..m {
                let k = if horizontal { j * w + i } else { i * w + j };
                sum += (d[k] - s[k]).abs() as f64;
            }
            sum / m as f64
        })
        .collect()
}

/// Steps where the source is flat (|source step| <= 1): `[count, sum of |decoded step|,
/// count of decoded steps >= 3]` at block edges (phase 0, tile edges excluded), at the other
/// seven phases, and at tile edges. A step of 3 or more code values on flat source is a visible
/// block step.
fn flat_steps(
    s: &[f32],
    d: &[f32],
    w: usize,
    h: usize,
    horizontal: bool,
    period: usize,
) -> [[f64; 3]; 3] {
    let n = if horizontal { w } else { h };
    let m = if horizontal { h } else { w };
    let mut acc = [[0.0f64; 3]; 3];
    for i in 1..n {
        let class = if i % period == 0 {
            2
        } else if i % 8 == 0 {
            0
        } else {
            1
        };
        for j in 0..m {
            let (a, b) = if horizontal {
                (j * w + i, j * w + i - 1)
            } else {
                (i * w + j, (i - 1) * w + j)
            };
            if (s[a] - s[b]).abs() <= 1.0 {
                let step = (d[a] - d[b]).abs() as f64;
                acc[class][0] += 1.0;
                acc[class][1] += step;
                acc[class][2] += f64::from(step >= 3.0);
            }
        }
    }
    acc
}

fn flat_json(a: &[[f64; 3]; 3]) -> String {
    let one = |c: &[f64; 3]| {
        format!(
            "{{\"n\":{},\"mean_step\":{:.4},\"rate_ge3\":{:.6}}}",
            c[0],
            c[1] / c[0].max(1.0),
            c[2] / c[0].max(1.0)
        )
    };
    format!(
        "{{\"block_edge\":{},\"other_phases\":{},\"tile_edge\":{}}}",
        one(&a[0]),
        one(&a[1]),
        one(&a[2])
    )
}

struct Axis {
    /// excess[i-1] is the step excess between sample i-1 and i.
    phase: [f64; 8],
    tile: f64,
    interior: f64,
    err_phase: [f64; 8],
    err_tile_edge: f64,
    err_other: f64,
}
fn axis(excess: &[f64], err: &[f64], period: usize) -> Axis {
    let mut sum = [0.0f64; 8];
    let mut cnt = [0usize; 8];
    let (mut ts, mut tc) = (0.0, 0usize);
    for (k, e) in excess.iter().enumerate() {
        let i = k + 1;
        if i % period == 0 {
            ts += e;
            tc += 1;
        } else {
            sum[i % 8] += e;
            cnt[i % 8] += 1;
        }
    }
    let phase: [f64; 8] = core::array::from_fn(|p| sum[p] / cnt[p].max(1) as f64);
    let interior = phase[1..].iter().sum::<f64>() / 7.0;
    let mut esum = [0.0f64; 8];
    let mut ecnt = [0usize; 8];
    let (mut te, mut tn) = (0.0, 0usize);
    let (mut oe, mut on) = (0.0, 0usize);
    for (i, e) in err.iter().enumerate() {
        esum[i % 8] += e;
        ecnt[i % 8] += 1;
        // The two samples on either side of a tile boundary.
        if i % period == 0 || (i + 1) % period == 0 {
            te += e;
            tn += 1;
        } else {
            oe += e;
            on += 1;
        }
    }
    Axis {
        phase,
        tile: ts / tc.max(1) as f64,
        interior,
        err_phase: core::array::from_fn(|p| esum[p] / ecnt[p].max(1) as f64),
        err_tile_edge: te / tn.max(1) as f64,
        err_other: oe / on.max(1) as f64,
    }
}

fn arr(a: &[f64; 8]) -> String {
    let v: Vec<String> = a.iter().map(|x| format!("{x:.4}")).collect();
    format!("[{}]", v.join(","))
}

/// JSON for the boundary diagnostic. `blockiness` = excess at block-edge phase 0
/// (tile boundaries excluded) minus the mean excess of phases 1..7; `tileiness` =
/// excess at tile boundaries minus the same phase-0 value. Both in 8-bit luma code values.
fn edge_json(src: &[u8], dec: &[u8], w: u32, h: u32, tw: u32, th: u32) -> String {
    let (w, h) = (w as usize, h as usize);
    let (s, d) = (luma(src), luma(dec));
    let hx = axis(
        &step_excess(&s, &d, w, h, true),
        &error_profile(&s, &d, w, h, true),
        tw as usize,
    );
    let vy = axis(
        &step_excess(&s, &d, w, h, false),
        &error_profile(&s, &d, w, h, false),
        th as usize,
    );
    let mae = s
        .iter()
        .zip(&d)
        .map(|(a, b)| (a - b).abs() as f64)
        .sum::<f64>()
        / s.len() as f64;
    let max = src
        .iter()
        .zip(dec)
        .map(|(a, b)| (*a as i32 - *b as i32).abs())
        .max()
        .unwrap_or(0);
    let (fh, fv) = (
        flat_steps(&s, &d, w, h, true, tw as usize),
        flat_steps(&s, &d, w, h, false, th as usize),
    );
    let one = |a: &Axis, f: &[[f64; 3]; 3]| {
        format!(
            "{{\"flat_source_steps\":{},\"excess_phase\":{},\"excess_tile_edge\":{:.4},\"excess_interior_mean\":{:.4},\
\"blockiness\":{:.4},\"tileiness\":{:.4},\"abs_err_phase\":{},\"abs_err_tile_edge_samples\":{:.4},\
\"abs_err_other_samples\":{:.4}}}",
            flat_json(f),
            arr(&a.phase),
            a.tile,
            a.interior,
            a.phase[0] - a.interior,
            a.tile - a.phase[0],
            arr(&a.err_phase),
            a.err_tile_edge,
            a.err_other
        )
    };
    // Chroma planes: flat-source steps only (same classes and threshold as luma).
    let chroma_json = [("cb", true), ("cr", false)].map(|(name, blue)| {
        let (cs, cd) = (chroma(src, blue), chroma(dec, blue));
        format!(
            "\"{name}\":{{\"cols\":{},\"rows\":{}}}",
            flat_json(&flat_steps(&cs, &cd, w, h, true, tw as usize)),
            flat_json(&flat_steps(&cs, &cd, w, h, false, th as usize))
        )
    });
    format!(
        "{{\"luma_mae\":{mae:.4},\"max_abs_err\":{max},\"chroma_flat_source_steps\":{{{},{}}},\"cols\":{},\"rows\":{}}}",
        chroma_json[0],
        chroma_json[1],
        one(&hx, &fh),
        one(&vy, &fv)
    )
}

fn read_raw(path: &str, w: u32, h: u32) -> Result<Vec<u8>> {
    let raw = std::fs::read(path)?;
    if raw.len() != w as usize * h as usize * 3 {
        return Err("raw RGB8 extent".into());
    }
    Ok(raw)
}

fn run_encode(a: &[String]) -> Result<()> {
    if a.len() < 5 {
        return Err("encode CROP.rgb W H OUTDIR LABEL [SET]".into());
    }
    let (w, h): (u32, u32) = (a[1].parse()?, a[2].parse()?);
    let raw = read_raw(&a[0], w, h)?;
    let out = Path::new(&a[3]);
    let label = &a[4];
    let set = a.get(5).map_or("main", String::as_str);
    std::fs::create_dir_all(out)?;
    let src_sha = sha(&raw);
    let mut jsonl = BufWriter::new(
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(out.join(format!("{label}.rows.jsonl")))?,
    );
    for s in settings(set)? {
        imij::lab::set_fdct_mode(u8::from(s.fdct_dense));
        let t = Instant::now();
        let e = encode_decode(&raw, w, h, &s)?;
        let secs = t.elapsed().as_secs_f64();
        imij::lab::set_fdct_mode(0);
        let name = format!("{label}.{}", s.id);
        let dec_path = out.join(format!("{name}.rgb"));
        if dec_path.exists() {
            return Err(format!("{} exists", dec_path.display()).into());
        }
        fresh(&dec_path)?.write_all(&e.decoded)?;
        // Encoded cells as an imij-set archive (tiles) or one file (continuous).
        let bytes: usize = e.cells.iter().map(Vec::len).sum();
        if s.tiled {
            let mut wr =
                imij_set::Writer::new(BufWriter::new(fresh(out.join(format!("{name}.imijset")))?));
            for (k, c) in e.cells.iter().enumerate() {
                let mut sum = imij_set::Summary::default();
                sum.width = TW;
                sum.height = TH;
                wr.insert(k as u64, c.len() as u64, 0, sum, &mut c.as_slice())?;
            }
            let (mut sink, _) = wr.finish()?;
            sink.flush()?;
        } else {
            fresh(out.join(format!("{name}.imi")))?.write_all(&e.cells[0])?;
        }
        let enc_sha = {
            let mut hsh = Sha256::new();
            for c in &e.cells {
                hsh.update((c.len() as u64).to_le_bytes());
                hsh.update(c);
            }
            hex(&hsh.finalize())
        };
        let edges = edge_json(&raw, &e.decoded, w, h, TW, TH);
        let exact = e.decoded == raw;
        let (q, scale) = match s.q {
            Q::Jpeg(q) => (format!("\"jpeg{q}\""), -1.0),
            Q::Scale(x) => ("\"scale\"".into(), x),
        };
        writeln!(
            jsonl,
            "{{\"label\":\"{label}\",\"id\":\"{}\",\"tiled\":{},\"lossless\":{},\"quality\":{q},\"scale\":{scale},\
\"effort\":{},\"bytes\":{bytes},\"bpp\":{:.4},\"cells\":{},\"enc_sha\":\"{enc_sha}\",\"dec_sha\":\"{}\",\
\"src_sha\":\"{src_sha}\",\"bit_exact\":{exact},\"encode_decode_s\":{secs:.2},\"w\":{w},\"h\":{h},\"edges\":{edges}}}",
            s.id,
            s.tiled,
            s.lossless,
            s.effort,
            bytes as f64 * 8.0 / (w as f64 * h as f64),
            e.cells.len(),
            sha(&e.decoded)
        )?;
        jsonl.flush()?;
        eprintln!("{name}: {bytes} B exact={exact} {secs:.1}s");
    }
    Ok(())
}

fn run_edges(a: &[String]) -> Result<()> {
    if a.len() < 4 {
        return Err("edges SRC.rgb DEC.rgb W H [TW TH]".into());
    }
    let (w, h): (u32, u32) = (a[2].parse()?, a[3].parse()?);
    let (tw, th): (u32, u32) = (
        a.get(4).map_or(Ok(TW), |s| s.parse())?,
        a.get(5).map_or(Ok(TH), |s| s.parse())?,
    );
    println!(
        "{}",
        edge_json(
            &read_raw(&a[0], w, h)?,
            &read_raw(&a[1], w, h)?,
            w,
            h,
            tw,
            th
        )
    );
    Ok(())
}

/// Re-encode the crop's tiles and compare their decoded SHA-256 with the hashes the
/// published `IMITILE1` index stores for the same cells of the full image.
fn run_deployed(a: &[String]) -> Result<()> {
    if a.len() < 7 {
        return Err("deployed INDEX.bin CROP.rgb W H X0 Y0 Q".into());
    }
    let mut idx = File::open(&a[0])?;
    let mut head = [0u8; 28];
    idx.read_exact(&mut head)?;
    if &head[..8] != b"IMITILE1" {
        return Err("index magic".into());
    }
    let word = |i: usize| u32::from_le_bytes(head[8 + 4 * i..12 + 4 * i].try_into().unwrap());
    let (iw, _ih, tw, th, count) = (word(0), word(1), word(2), word(3), word(4));
    if (tw, th) != (TW, TH) {
        return Err("tile geometry".into());
    }
    let mut hashes = Vec::new();
    for _ in 0..count {
        let mut rec = [0u8; 8 + 4 + 32];
        idx.read_exact(&mut rec)?;
        hashes.push(hex(&rec[12..]));
    }
    let (w, h, x0, y0): (u32, u32, u32, u32) =
        (a[2].parse()?, a[3].parse()?, a[4].parse()?, a[5].parse()?);
    let q: f32 = a[6].parse()?;
    if x0 % TW != 0 || y0 % TH != 0 {
        return Err("crop origin must be tile aligned".into());
    }
    let mut src = File::open(&a[1])?;
    let cols = iw.div_ceil(TW);
    let cfg = Setting::production(q, true).lossy_config();
    let (mut same, mut total) = (0, 0);
    for ty in 0..h.div_ceil(TH) {
        for tx in 0..w.div_ceil(TW) {
            let (x, y) = (x0 + tx * TW, y0 + ty * TH);
            let (cw, chh) = ((w - tx * TW).min(TW), (h - ty * TH).min(TH));
            let mut tile = vec![0u8; cw as usize * chh as usize * 3];
            for yy in 0..chh as u64 {
                let at = (((ty * TH) as u64 + yy) * w as u64 + (tx * TW) as u64) * 3;
                let n = cw as usize * 3;
                src.seek(SeekFrom::Start(at))?;
                src.read_exact(&mut tile[yy as usize * n..(yy as usize + 1) * n])?;
            }
            let enc = cfg.encode_samples(
                imij::ImageRef::new(&tile, cw as usize * 3, &info(cw, chh)),
                &Metadata::default(),
            )?;
            let (_, dec) = imij::decode(&enc)?;
            let key = (y / TH) as usize * cols as usize + (x / TW) as usize;
            let ok = hashes.get(key).is_some_and(|h| *h == sha(&dec));
            same += usize::from(ok);
            total += 1;
            println!(
                "tile key {key} ({x},{y}) {cw}x{chh}: decoded hash {}",
                if ok { "MATCH" } else { "DIFFERS" }
            );
        }
    }
    println!("deployed-hash match {same}/{total} (q{q})");
    if same != total {
        return Err("re-encoded tiles differ from the published archive".into());
    }
    Ok(())
}

fn run_jpeg(a: &[String]) -> Result<()> {
    if a.len() < 5 {
        return Err("jpeg CROP.rgb W H OUTDIR LABEL".into());
    }
    let (w, h): (u32, u32) = (a[1].parse()?, a[2].parse()?);
    let raw = read_raw(&a[0], w, h)?;
    let out = Path::new(&a[3]);
    let label = &a[4];
    std::fs::create_dir_all(out)?;
    let mut jsonl = BufWriter::new(
        OpenOptions::new()
            .append(true)
            .create(true)
            .open(out.join(format!("{label}.rows.jsonl")))?,
    );
    let src_sha = sha(&raw);
    for (sub, subname) in [
        (zenjpeg::encoder::ChromaSubsampling::Quarter, "420"),
        (zenjpeg::encoder::ChromaSubsampling::None, "444"),
    ] {
        for q in [
            30.0f32, 40.0, 50.0, 60.0, 70.0, 75.0, 80.0, 85.0, 90.0, 95.0,
        ] {
            let cfg = zenjpeg::encoder::EncoderConfig::ycbcr(q, sub);
            let mut enc = cfg.encode_from_bytes(w, h, zenjpeg::encoder::PixelLayout::Rgb8Srgb)?;
            enc.push_packed(&raw, enough::Unstoppable)?;
            let jpeg = enc.finish()?;
            let decoded = zenjpeg::decoder::DecodeConfig::new()
                .decode(&jpeg, enough::Unstoppable)?
                .into_pixels_u8()
                .ok_or("expected RGB8 JPEG")?;
            if decoded.len() != raw.len() {
                return Err("decoded extent".into());
            }
            let id = format!("zenjpeg-{subname}-q{q:.0}");
            let name = format!("{label}.{id}");
            fresh(out.join(format!("{name}.jpg")))?.write_all(&jpeg)?;
            fresh(out.join(format!("{name}.rgb")))?.write_all(&decoded)?;
            let edges = edge_json(&raw, &decoded, w, h, TW, TH);
            writeln!(
                jsonl,
                "{{\"label\":\"{label}\",\"id\":\"{id}\",\"tiled\":false,\"lossless\":false,\"quality\":\"zenjpeg{q}\",\
\"scale\":-1,\"effort\":0,\"bytes\":{},\"bpp\":{:.4},\"cells\":1,\"enc_sha\":\"{}\",\"dec_sha\":\"{}\",\
\"src_sha\":\"{src_sha}\",\"bit_exact\":false,\"encode_decode_s\":0,\"w\":{w},\"h\":{h},\"edges\":{edges}}}",
                jpeg.len(),
                jpeg.len() as f64 * 8.0 / (w as f64 * h as f64),
                sha(&jpeg),
                sha(&decoded)
            )?;
            jsonl.flush()?;
            eprintln!("{name}: {} B", jpeg.len());
        }
    }
    Ok(())
}

fn run_decode(a: &[String]) -> Result<()> {
    for path in a {
        let mut hash = Sha256::new();
        let mut cells = 0usize;
        if path.ends_with(".imijset") {
            let mut reader =
                imij_set::Reader::open(imij::planes::SeekReader::new(File::open(path)?))?;
            let mut listing = reader.listing()?;
            while let Some(entry) = listing.next(&mut reader)? {
                let bytes = reader.read_entry_bounded(&entry, 1 << 28)?;
                hash.update(imij::decode(&bytes)?.1);
                cells += 1;
            }
        } else {
            hash.update(imij::decode(&std::fs::read(path)?)?.1);
            cells = 1;
        }
        println!(
            "{} cells={cells} decoded_sha256={}",
            path,
            hex(&hash.finalize())
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("encode") => run_encode(&a[1..]),
        Some("edges") => run_edges(&a[1..]),
        Some("jpeg") => run_jpeg(&a[1..]),
        Some("decode") => run_decode(&a[1..]),
        Some("deployed") => run_deployed(&a[1..]),
        _ => Err("encode | jpeg | edges | decode | deployed".into()),
    }
}
