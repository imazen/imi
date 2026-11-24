//! Native-learner DC controls in the retained private palette carrier.
//! The ordinary AC payloads are identical in every arm. Numeric analysis,
//! maps, row predictors, tokenization and coding use the native implementation.
use super::*;
use crate::v1::model_study::{Definition, NativeStudy, Training};

#[path = "palette_native_measure.rs"]
mod measurement;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Native,
    Root,
    Inline,
    Catalog,
    References,
}

fn definition(d: &Definition) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, d.bank);
    out.extend_from_slice(&d.head);
    out
}
fn available(d: &Definition, u: usize) -> bool {
    match d.training {
        Training::Inline(at) => at == u,
        _ => true,
    }
}
fn materialize(
    common: &[u8],
    ac: &[&[u8]],
    study: &NativeStudy,
    mode: Mode,
    active: &[usize],
    chosen: &[usize],
) -> Candidate {
    let version = if matches!(mode, Mode::Inline | Mode::References) {
        3
    } else {
        2
    };
    let mut units = Vec::new();
    let mut introduced = Vec::<(usize, usize)>::new();
    for (u, &model) in chosen.iter().enumerate() {
        let d = &study.definitions[model];
        let mut body = Vec::new();
        if version == 3 {
            if model == active[0] {
                put_varint(&mut body, 0);
            } else if mode == Mode::References && !matches!(d.training, Training::Inline(_)) {
                if let Some((_, at)) = introduced.iter().find(|(m, _)| *m == model) {
                    put_varint(&mut body, (*at as u64) * 4 + 3);
                } else {
                    let head = definition(d);
                    put_varint(&mut body, (head.len() as u64) * 4 + 2);
                    body.extend_from_slice(&head);
                    introduced.push((model, u));
                }
            } else {
                let head = definition(d);
                put_varint(&mut body, (head.len() as u64) * 4 + 1);
                body.extend_from_slice(&head);
            }
        }
        body.extend_from_slice(&d.records[u]);
        let id = if version == 2 {
            active.iter().position(|&m| m == model).unwrap()
        } else {
            0
        };
        units.push(Unit {
            kind: 0,
            ordinal: u,
            model: id,
            bytes: body,
        });
    }
    units.extend(ac.iter().enumerate().map(|(u, b)| Unit {
        kind: 1,
        ordinal: u,
        model: 0,
        bytes: b.to_vec(),
    }));
    units.sort_by_key(|u| (u.ordinal, u.kind));
    Candidate {
        version,
        common: common.to_vec(),
        dc: if version == 3 {
            vec![definition(&study.definitions[active[0]])]
        } else {
            active
                .iter()
                .map(|&m| definition(&study.definitions[m]))
                .collect()
        },
        ac: vec![Vec::new()],
        units,
    }
}

fn select(common: &[u8], ac: &[&[u8]], study: &NativeStudy, mode: Mode) -> Candidate {
    let pool: Vec<_> = study
        .definitions
        .iter()
        .enumerate()
        .filter_map(|(i, d)| (!matches!(d.training, Training::Inline(_))).then_some(i))
        .collect();
    let globals: Vec<_> = pool
        .iter()
        .copied()
        .filter(|&i| matches!(study.definitions[i].training, Training::Global))
        .collect();
    let count = study.definitions[0].records.len();
    let singleton = |m| materialize(common, ac, study, Mode::Root, &[m], &vec![m; count]);
    let roots = if mode == Mode::Native {
        &globals
    } else {
        &pool
    };
    let mut best = roots
        .iter()
        .map(|&m| singleton(m))
        .min_by_key(|c| c.write().len())
        .unwrap();
    if matches!(mode, Mode::Native | Mode::Root) {
        return best;
    }
    for &root in roots {
        let mut active = vec![root];
        if mode != Mode::Inline {
            active.extend(pool.iter().copied().filter(|&m| m != root));
        }
        let plan = |active: &[usize]| {
            let chosen: Vec<_> = (0..count)
                .map(|u| {
                    study
                        .definitions
                        .iter()
                        .enumerate()
                        .filter(|(m, d)| {
                            available(d, u)
                                && (active.contains(m)
                                    || (mode != Mode::Catalog
                                        && matches!(d.training, Training::Inline(_))))
                        })
                        .min_by_key(|(m, d)| {
                            let extra = if *m == root {
                                usize::from(mode != Mode::Catalog || active.len() > 1)
                            } else if mode == Mode::Inline
                                || matches!(d.training, Training::Inline(_))
                            {
                                let n = d.head.len() + 1;
                                n + crate::varint_len(n as u64 * 4 + 1)
                            } else {
                                1
                            };
                            d.records[u].len() + extra
                        })
                        .map(|(m, _)| m)
                        .unwrap()
                })
                .collect();
            materialize(common, ac, study, mode, active, &chosen)
        };
        loop {
            let now = plan(&active).write().len();
            let next = (1..active.len())
                .map(|at| {
                    let mut trial = active.clone();
                    trial.remove(at);
                    let n = plan(&trial).write().len();
                    (n, trial)
                })
                .min_by_key(|(n, _)| *n);
            match next {
                Some((n, a)) if n <= now => active = a,
                _ => break,
            }
        }
        let candidate = plan(&active);
        if candidate.write().len() < best.write().len() {
            best = candidate;
        }
    }
    best
}

fn prepare(file: &[u8], effort: f32, bank: u64) -> (ImageInfo, Vec<u8>, NativeStudy) {
    let p = super::super::super::super::parse_lossy(file).unwrap();
    assert!(!p.local_ans_contexts && p.catalog_lab.is_none());
    let dc = p.dc.as_ref().unwrap();
    let (info, pixels, _) = super::super::super::super::dc_image_lab(file).unwrap();
    let cfg = crate::EncodeConfig {
        effort,
        stripe_rows: Some(dc.hs as u32),
        lanes: Some(dc.lanes as u32),
        column_groups: Some(dc.gcols as u32),
        float_shift: false,
        ..Default::default()
    };
    let study = crate::seeds::with_role(crate::seeds::Role::Dc, || {
        crate::v1::model_study::prepare(
            &pixels,
            info.row_bytes(),
            &info,
            &cfg,
            crate::seeds::bank(bank),
            4,
            true,
        )
    })
    .unwrap();
    if super::super::super::super::lab_entropy_seed(file)
        .unwrap()
        .1
        .scope
        & crate::seeds::SCOPE_DC
        == 0
    {
        assert_eq!(
            study.definitions[0].records,
            dc.stripes.iter().map(|b| b.to_vec()).collect::<Vec<_>>(),
            "native global DC payloads must match, before changing model placement"
        );
    }
    (info, pixels, study)
}

fn check_native(file: &[u8], encoded: &[u8]) -> ReadPalette {
    let reader = ReadPalette::new(encoded);
    let p = super::super::super::super::parse_lossy(file).unwrap();
    let (di, dc, _) = super::super::super::super::dc_image_lab(file).unwrap();
    for (entry, &(kind, u, _, _, _)) in reader.units.iter().enumerate() {
        let expected = if kind == 0 {
            let d = p.dc.as_ref().unwrap();
            let y = u / d.ngroups() * d.hs;
            let (x, w, _) = d.group(u % d.ngroups());
            let rows = d.hs.min(di.height as usize - y);
            let bpp = di.bytes_per_pixel();
            (y..y + rows)
                .flat_map(|row| {
                    dc[row * di.row_bytes() + x * bpp..row * di.row_bytes() + (x + w) * bpp]
                        .iter()
                        .copied()
                })
                .collect::<Vec<_>>()
        } else {
            let g = p.group(u % p.ngroups()).1;
            let stripe = u / p.ngroups();
            let (a, b) = g.rows(stripe);
            let (q, _, _, _) = super::super::super::super::decode_stripe_raw_bytes(
                &p,
                &g,
                stripe,
                p.stripes[u],
                Group::ALL,
                true,
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
            q[..(b - a) * g.bw * NCH * 64]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect()
        };
        assert_eq!(reader.unit(encoded, entry), expected);
        // Preserve only required metadata; every unrelated coded payload is
        // absent. Reopen so no previous expansion/cache supplies hidden state.
        let mut sparse = encoded.to_vec();
        for (other, &(_, _, _, at, n)) in reader.units.iter().enumerate() {
            if other != entry {
                sparse[at..at + n].fill(0);
            }
        }
        assert_eq!(ReadPalette::new(&sparse).unit(&sparse, entry), expected);
    }
    let expected = reader.decode(encoded, 1);
    for threads in [2, 4, 8] {
        assert_eq!(reader.decode(encoded, threads), expected);
    }
    reader
}

#[test]
fn native_dc_models_and_indexed_definition_references() {
    let info = ImageInfo::new(129, 145, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize)
        .map(|i| ((i * 41 + i / 97) ^ (i / 177)) as u8)
        .collect();
    for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
        let cfg = configuration(40.0, LossyColor::Xyb, entropy, 64);
        let file = super::super::super::super::encode_lossy(
            &pixels,
            info.row_bytes(),
            &info,
            &Metadata::default(),
            &cfg,
        )
        .unwrap();
        for bank in [0, 1] {
            let (_, _, study) = prepare(&file, 0.0, bank);
            let p = super::super::super::super::parse_lossy(&file).unwrap();
            let common = common(&file);
            for mode in [
                Mode::Native,
                Mode::Root,
                Mode::Inline,
                Mode::Catalog,
                Mode::References,
            ] {
                check_native(&file, &select(&common, &p.stripes, &study, mode).write());
            }
            let count = study.definitions[0].records.len();
            let alternate = study
                .definitions
                .iter()
                .position(|d| matches!(d.training, Training::Bucket(_)))
                .unwrap();
            let forced = vec![alternate; count];
            let shared = materialize(
                &common,
                &p.stripes,
                &study,
                Mode::References,
                &[0, alternate],
                &forced,
            );
            check_native(&file, &shared.write());
            let inline = materialize(&common, &p.stripes, &study, Mode::Inline, &[0], &forced);
            check_native(&file, &inline.write());
        }
    }
    if let Some(path) = std::env::var_os("IMI_NATIVE_DC_MODEL_MANIFEST") {
        measurement::measure(Path::new(&path));
    }
}

#[test]
fn indexed_numeric_definitions_reject_invalid_dependencies() {
    let info = ImageInfo::new(65, 145, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize)
        .map(|i| (i * 37) as u8)
        .collect();
    let cfg = configuration(40.0, LossyColor::Xyb, LossyEntropy::RunLevel, 64);
    let file = super::super::super::super::encode_lossy(
        &pixels,
        info.row_bytes(),
        &info,
        &Metadata::default(),
        &cfg,
    )
    .unwrap();
    let (di, dp, study) = prepare(&file, 0.0, 0);
    let p = super::super::super::super::parse_lossy(&file).unwrap();
    let common = common(&file);
    let base = || {
        materialize(
            &common,
            &p.stripes,
            &study,
            Mode::References,
            &[0],
            &vec![0; study.definitions[0].records.len()],
        )
    };
    let reject = |candidate: Candidate| {
        assert!(std::panic::catch_unwind(|| ReadPalette::new(&candidate.write())).is_err());
    };
    for (ordinal, control) in [
        (0, 3),
        (0, 7),
        (1, 3),
        (0, ((1024 * 1024 + 1) as u64) * 4 + 1),
    ] {
        let mut c = base();
        let unit = c
            .units
            .iter_mut()
            .find(|u| u.kind == 0 && u.ordinal == ordinal)
            .unwrap();
        unit.bytes.clear();
        put_varint(&mut unit.bytes, control);
        unit.bytes
            .extend_from_slice(&study.definitions[0].records[ordinal]);
        reject(c);
    }
    let mut c = base();
    c.dc[0][0] = 255;
    reject(c);
    let mut c = base();
    c.units[0].kind = 2;
    reject(c);
    let different = crate::EncodeConfig {
        effort: 0.0,
        stripe_rows: Some(1),
        column_groups: Some(0),
        float_shift: false,
        ..Default::default()
    };
    let (head, _, _) = crate::v1::encode_parts(&dp, di.row_bytes(), &di, &different).unwrap();
    let mut c = base();
    c.dc[0] = [vec![0], head].concat();
    reject(c);
}
