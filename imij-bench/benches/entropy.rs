//! Isolated entropy-decode benchmark: same token/context model, different
//! coders and loop orders. Set IMIJ_CORPUS (manifest) and IMIJ_CLASSES.

#[path = "../src/corpus.rs"]
mod corpus;

use std::path::Path;

use imij::lab::{self, Coder, LabCfg, LaneStripe};
use imij::lab_rans;
use imij::{AnalysisConfig as ImiEncoderConfig, analyze};
use zenbench::prelude::*;

struct RCase {
    name: String,
    stripes: Vec<lab_rans::IStripe>,
    enc: lab_rans::RansEncoded,
    flats: Vec<lab_rans::FlatTables<{ lab_rans::NCTX << 12 }>>,
}

fn rans_cases(class: &str, imgs: &[corpus::Img]) -> Vec<RCase> {
    let mut all = Vec::new();
    let mut bits = Vec::new();
    let mut cs = 3;
    let mut px = 0;
    for im in imgs {
        let a = analyze(
            &im.pixels,
            im.info.row_bytes(),
            &im.info,
            &ImiEncoderConfig {
                lanes: 16,
                ..Default::default()
            },
        )
        .unwrap();
        bits = a.slot_bits().to_vec();
        cs = im.info.channels.color_count();
        px += im.pixels_count();
        all.extend(lab::lane_stripes(&a).iter().map(lab_rans::interleave));
    }
    let mut out = Vec::new();
    for lm in [10u32, 12] {
        let enc = lab_rans::encode(&all, &bits, cs, lm, 4, 2, 4);
        let flats: Vec<_> = enc
            .models
            .iter()
            .map(|m| m.flat::<{ lab_rans::NCTX << 12 }>())
            .collect();
        for (si, st) in all.iter().enumerate() {
            let mut o: Vec<Vec<u16>> = st.u.iter().map(|v| vec![0u16; v.len()]).collect();
            lab_rans::decode_scalar(&enc, si, st, &mut o);
            assert!(o == st.u, "{class} rans{lm} scalar mismatch stripe {si}");
            let mut o2: Vec<Vec<u16>> = st.u.iter().map(|v| vec![0u16; v.len()]).collect();
            lab_rans::decode_simd(&enc, si, st, &mut o2, &flats);
            assert!(o2 == st.u, "{class} rans{lm} simd mismatch stripe {si}");
            let mut o3: Vec<Vec<u16>> = st.u.iter().map(|v| vec![0u16; v.len()]).collect();
            lab_rans::decode_fused(&enc, si, st, &mut o3, &flats);
            assert!(o3 == st.u, "{class} rans{lm} fused mismatch stripe {si}");
        }
        eprintln!(
            "{class} rans{lm}x16: {:.4} bpp payload",
            enc.payload_bytes as f64 * 8.0 / px as f64
        );
        let stripes = all
            .iter()
            .map(|s| lab_rans::IStripe {
                rows: s.rows,
                lw: s.lw,
                widths: s.widths,
                u: s.u.clone(),
            })
            .collect();
        out.push(RCase {
            name: format!("rans{lm}x16"),
            stripes,
            enc,
            flats,
        });
    }
    out
}

struct Case {
    name: String,
    pixels: usize,
    stripes: Vec<LaneStripe>,
    enc: lab::LabEncoded,
}

fn cases_for(class: &str, imgs: &[corpus::Img]) -> Vec<Case> {
    let mut out = Vec::new();
    let coders = [
        ("huff12", Coder::Huffman { max_len: 12 }),
        ("tans12", Coder::Tans { log_m: 12 }),
        ("tans10", Coder::Tans { log_m: 10 }),
        ("tans8", Coder::Tans { log_m: 8 }),
    ];
    for lanes in [1u32, 16] {
        // Concatenate stripes of all images in the class.
        let mut all: Vec<LaneStripe> = Vec::new();
        let mut bits = Vec::new();
        let mut color_slots = 3;
        let mut px = 0;
        for im in imgs {
            let a = analyze(
                &im.pixels,
                im.info.row_bytes(),
                &im.info,
                &ImiEncoderConfig {
                    lanes,
                    ..Default::default()
                },
            )
            .unwrap();
            bits = a.slot_bits().to_vec();
            color_slots = im.info.channels.color_count();
            px += im.pixels_count();
            all.extend(lab::lane_stripes(&a));
        }
        for (cn, coder) in coders {
            if lanes == 1 && cn != "huff12" {
                continue;
            }
            let cfg = LabCfg {
                coder,
                k: 4,
                m: 2,
                run_min: 4,
                max_ctx: 4,
                cross: true,
                color_slots,
            };
            let enc = lab::encode(&all, &bits, cfg);
            // Verify both decoders.
            for (si, st) in all.iter().enumerate() {
                let mut o: Vec<Vec<u16>> = st.u.iter().map(|v| vec![0u16; v.len()]).collect();
                lab::decode_lane_seq(&enc, si, st, &mut o);
                assert!(
                    o == st.u,
                    "{class} {cn} lanes{lanes} lane_seq mismatch stripe {si}"
                );
                let mut o2: Vec<Vec<u16>> = st.u.iter().map(|v| vec![0u16; v.len()]).collect();
                fast(&enc, si, st, &mut o2);
                assert!(
                    o2 == st.u,
                    "{class} {cn} lanes{lanes} fast mismatch stripe {si}"
                );
            }
            eprintln!(
                "{class} {cn} lanes{lanes}: {:.4} bpp payload, {} tokens",
                enc.payload_bytes as f64 * 8.0 / px as f64,
                enc.tokens
            );
            out.push(Case {
                name: format!("{cn}_l{lanes}"),
                pixels: px,
                stripes: std::mem::take(&mut all),
                enc,
            });
            // Rebuild stripes for the next coder (moved above).
            for im in imgs {
                let a = analyze(
                    &im.pixels,
                    im.info.row_bytes(),
                    &im.info,
                    &ImiEncoderConfig {
                        lanes,
                        ..Default::default()
                    },
                )
                .unwrap();
                all.extend(lab::lane_stripes(&a));
            }
        }
    }
    out
}

fn fast(enc: &lab::LabEncoded, si: usize, st: &LaneStripe, out: &mut [Vec<u16>]) {
    let tans = enc.is_tans();
    match (st.u.len(), tans) {
        (1, false) => lab::decode_fast::<1, false>(enc, si, st, out),
        (3, false) => lab::decode_fast::<3, false>(enc, si, st, out),
        (4, false) => lab::decode_fast::<4, false>(enc, si, st, out),
        (1, true) => lab::decode_fast::<1, true>(enc, si, st, out),
        (3, true) => lab::decode_fast::<3, true>(enc, si, st, out),
        (4, true) => lab::decode_fast::<4, true>(enc, si, st, out),
        _ => lab::decode_lane_seq(enc, si, st, out),
    }
}

fn bench(suite: &mut Suite) {
    let manifest = std::env::var("IMIJ_CORPUS")
        .unwrap_or_else(|_| format!("{}/tmp/zqi/corpus_lab.tsv", std::env::var("HOME").unwrap()));
    let classes = std::env::var("IMIJ_CLASSES").unwrap_or_else(|_| "photo,screen,clipart".into());
    for class in classes.split(',') {
        let list = corpus::load_manifest(&manifest, Some(class));
        let imgs: Vec<corpus::Img> = list
            .iter()
            .filter_map(|(c, p)| corpus::load_png(c, Path::new(p)).ok())
            .collect();
        if imgs.is_empty() {
            continue;
        }
        let rcases = std::sync::Arc::new(rans_cases(class, &imgs));
        let cases = std::sync::Arc::new(cases_for(class, &imgs));
        let px = cases[0].pixels;
        suite.group(format!("entropy_{class}"), |g| {
            g.throughput(Throughput::Elements(px as u64));
            g.throughput_unit("px");
            for ci in 0..rcases.len() {
                for (mode, simd) in [("avx2", 1u8), ("avx2fused", 2)] {
                    let rc = rcases.clone();
                    g.bench(format!("{}_{mode}", rc[ci].name), move |b| {
                        let c = &rc[ci];
                        let mut outs: Vec<Vec<Vec<u16>>> = c
                            .stripes
                            .iter()
                            .map(|st| st.u.iter().map(|v| vec![0u16; v.len()]).collect())
                            .collect();
                        b.iter(|| {
                            for (si, st) in c.stripes.iter().enumerate() {
                                match simd {
                                    1 => lab_rans::decode_simd(
                                        &c.enc,
                                        si,
                                        st,
                                        &mut outs[si],
                                        &c.flats,
                                    ),
                                    2 => lab_rans::decode_fused(
                                        &c.enc,
                                        si,
                                        st,
                                        &mut outs[si],
                                        &c.flats,
                                    ),
                                    _ => lab_rans::decode_scalar(&c.enc, si, st, &mut outs[si]),
                                }
                            }
                            outs[0][0][0]
                        })
                    });
                }
            }
            for ci in 0..cases.len().min(if std::env::var("IMIJ_ONLY_RANS").is_ok() {
                1
            } else {
                usize::MAX
            }) {
                for (mode, lockstep) in [("seq", 0u8), ("fast", 2)] {
                    let cases = cases.clone();
                    g.bench(format!("{}_{mode}", cases[ci].name), move |b| {
                        let c = &cases[ci];
                        let mut outs: Vec<Vec<Vec<u16>>> = c
                            .stripes
                            .iter()
                            .map(|st| st.u.iter().map(|v| vec![0u16; v.len()]).collect())
                            .collect();
                        b.iter(|| {
                            for (si, st) in c.stripes.iter().enumerate() {
                                match lockstep {
                                    1 => lab::decode_lockstep(&c.enc, si, st, &mut outs[si]),
                                    2 => fast(&c.enc, si, st, &mut outs[si]),
                                    _ => lab::decode_lane_seq(&c.enc, si, st, &mut outs[si]),
                                }
                            }
                            outs[0][0][0]
                        })
                    });
                }
            }
        });
    }
}

zenbench::main!(bench);
