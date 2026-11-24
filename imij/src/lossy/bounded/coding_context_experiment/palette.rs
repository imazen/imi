//! Private, indexed DC/AC model catalogs. No record allocation or codec API.
//! Models see the complete quantized frame. These are two-pass size/entropy
//! experiments, not evidence of causal one-pass learning or whole-codec speed.
use super::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

#[path = "palette_native.rs"]
mod native;

struct Unit {
    kind: u8,
    ordinal: usize,
    model: usize,
    bytes: Vec<u8>,
}
struct Candidate {
    version: u8,
    common: Vec<u8>,
    dc: Vec<Vec<u8>>,
    ac: Vec<Vec<u8>>,
    units: Vec<Unit>,
}
fn bytes(out: &mut Vec<u8>, data: &[u8]) {
    put_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}
fn take<'a>(r: &mut ByteReader<'a>) -> &'a [u8] {
    let n = r.varint().unwrap() as usize;
    let at = r.pos;
    r.pos += n;
    &r.data[at..at + n]
}

// Common native declaration is retained conservatively, including its original
// global tables/hint. Catalog candidates all pay those identical bytes. A wire
// adoption would remove superseded models and define the new directory grammar.
fn common(file: &[u8]) -> Vec<u8> {
    let mut r = ByteReader::new(file);
    let h = crate::parse::header::read_header_sized(&mut r).unwrap();
    let (_, _, body, features) = crate::v1::lossy_body(file).unwrap().unwrap();
    let mut b = ByteReader::new(body);
    super::super::super::parse_lossy_head_checked(
        h.header.info,
        h.header.lossy_color.unwrap(),
        features,
        &mut b,
        &crate::parse::limits::Unrestricted,
    )
    .unwrap();
    let mut out = Vec::new();
    bytes(&mut out, &file[..r.pos]);
    put_varint(&mut out, features.bits());
    bytes(&mut out, &body[..b.pos]);
    out
}
fn parsed(common: &[u8]) -> super::super::super::LossyParsed<'static> {
    let mut r = ByteReader::new(common);
    let header = take(&mut r);
    let mut h = ByteReader::new(header);
    let h = crate::parse::header::read_header_sized(&mut h)
        .unwrap()
        .header;
    let f = Features::from_bits(r.varint().unwrap()).unwrap();
    let body = take(&mut r);
    let mut b = ByteReader::new(body);
    super::super::super::parse_lossy_head_checked(
        h.info,
        h.lossy_color.unwrap(),
        f,
        &mut b,
        &crate::parse::limits::Unrestricted,
    )
    .unwrap()
    .without_records()
}
fn build(file: &[u8], buckets: usize) -> Candidate {
    use crate::stream_encode::{GroupEncodeConfig, NativeGroupEncodeSession as GroupEncodeSession};
    let p = super::super::super::parse_lossy(file).unwrap();
    assert_eq!(p.dc_k(), 1);
    assert!(!p.aq && p.exact.is_none());
    let count = p.stripes.len();
    let mut lanes = Vec::new();
    for (s, body) in p.stripes.iter().enumerate() {
        let (mut q, _, _, _) = super::super::super::decode_stripe_raw_bytes(
            &p,
            &p.g,
            s,
            body,
            Group::ALL,
            true,
            Vec::new(),
            Vec::new(),
        )
        .unwrap();
        let (a, b) = p.g.rows(s);
        q.truncate((b - a) * p.g.bw * NCH * 64);
        let nz = q
            .as_chunks::<64>()
            .0
            .iter()
            .map(|block| {
                ZIGZAG.iter().enumerate().fold(0u64, |mask, (i, &at)| {
                    mask | ((block[at as usize] != 0) as u64) << i
                })
            })
            .collect();
        let sc = StripeCoefs {
            by0: a,
            nby: b - a,
            bw: p.g.bw,
            q,
            nz,
            aq: Vec::new(),
            modes: Vec::new(),
            splits: Vec::new(),
        };
        let mut symbols = core::array::from_fn(|_| Vec::new());
        stripe_symbols(&sc, p.g.bw, &mut symbols);
        lanes.push(symbols);
    }
    let (di, dp, rows) = super::super::super::dc_image_lab(file).unwrap();
    assert_eq!(rows, 8);
    let cfg = GroupEncodeConfig::new(di.width, rows, usize::MAX);
    let (session, _) = GroupEncodeSession::nested(di.clone(), cfg).unwrap();
    // Every reusable model must cover the complete observed alphabet, even
    // when its bucket did not contain a token. Otherwise an unsupported token
    // can appear spuriously cheap and produce an invalid entropy stream.
    let mut ac_support = vec![vec![0u64; coef::ALPHA]; LossyEntropy::RunLevel.nraw()];
    for symbol in lanes.iter().flatten().flatten() {
        ac_support[symbol.ctx as usize][symbol.tok as usize] = 1;
    }
    let mut dc_support = session.empty_histograms();
    for s in 0..count {
        let y = s * 8;
        session.group_histograms(
            di.width as usize,
            8.min(di.height as usize - y),
            &dp[y * di.row_bytes()..],
            di.row_bytes(),
            &mut dc_support,
        );
    }
    for slot in &mut dc_support {
        for class in slot {
            for value in class {
                *value = u64::from(*value != 0);
            }
        }
    }
    let sets = if buckets == 1 { 1 } else { buckets + 1 };
    let mut dc_heads = Vec::new();
    let mut ac_heads = Vec::new();
    let mut dc_payloads = Vec::new();
    let mut ac_payloads = Vec::new();
    for model in 0..sets {
        let includes = |s: usize| model == 0 || (s * buckets / count).min(buckets - 1) == model - 1;
        let mut hist = vec![vec![0u64; coef::ALPHA]; LossyEntropy::RunLevel.nraw()];
        let mut dh = session.empty_histograms();
        for s in 0..count {
            if includes(s) {
                for symbol in lanes[s].iter().flatten() {
                    hist[symbol.ctx as usize][symbol.tok as usize] += 1;
                }
                let y = s * 8;
                let h = 8.min(di.height as usize - y);
                session.group_histograms(
                    di.width as usize,
                    h,
                    &dp[y * di.row_bytes()..],
                    di.row_bytes(),
                    &mut dh,
                );
            }
        }
        if model != 0 {
            for (hist, support) in hist.iter_mut().flatten().zip(ac_support.iter().flatten()) {
                *hist += support;
            }
            for (hist, support) in dh
                .iter_mut()
                .flatten()
                .flatten()
                .zip(dc_support.iter().flatten().flatten())
            {
                *hist += support;
            }
        }
        let tables = Tables::build_with_effort(
            &hist,
            4,
            16,
            LossyEntropy::RunLevel,
            crate::effort::Effort::new(0.0).unwrap(),
        );
        let mut w = BitWriter::new();
        tables.write(&mut w);
        ac_heads.push(w.finish());
        let encoder = EncodeTables::new(&tables.freq, &tables.ctx_map);
        ac_payloads.push(
            lanes
                .iter()
                .map(|symbols| {
                    let mut out = Vec::new();
                    encode_lanes_tables(&encoder, symbols, p.g.bw.min(NL), &mut out);
                    out
                })
                .collect::<Vec<_>>(),
        );
        let (mut encoder, head) =
            GroupEncodeSession::nested_measured(di.clone(), cfg, &dh).unwrap();
        dc_heads.push(head);
        let mut payloads = Vec::new();
        while let Some(rect) = encoder.next_group() {
            encoder
                .push_group(&dp[rect.y as usize * di.row_bytes()..], di.row_bytes())
                .unwrap();
            let mut r = ByteReader::new(encoder.output());
            assert_eq!(r.varint().unwrap(), crate::v1::R_STRIPE);
            let n = r.varint().unwrap() as usize;
            assert_eq!(r.pos + n, encoder.output().len());
            payloads.push(encoder.output()[r.pos..].to_vec());
            encoder.consume_output(encoder.output().len()).unwrap();
        }
        encoder.finish().unwrap();
        let mut head_reader = ByteReader::new(dc_heads.last().unwrap());
        let decoded_model = crate::v1::parse_head_nested(
            di.clone(),
            &mut head_reader,
            &crate::parse::limits::Unrestricted,
            None,
            Features::ALL,
        )
        .unwrap();
        let om = crate::v1::OutMap::new(&di, &decoded_model.layout);
        for (s, payload) in payloads.iter().enumerate() {
            let mut state = crate::v1::UnitRows::new_with(&decoded_model, s, payload).unwrap();
            let y = s * 8;
            let mut row = vec![0; di.row_bytes()];
            for offset in 0..8.min(di.height as usize - y) {
                state.next_row(&decoded_model, &om, &mut row).unwrap();
                assert_eq!(
                    row,
                    dp[(y + offset) * di.row_bytes()..][..di.row_bytes()],
                    "every DC model candidate must be valid before pricing: model {model}, unit {s}"
                );
            }
        }
        dc_payloads.push(payloads);
    }
    let mut units = Vec::new();
    // Price catalog activation as well as payloads. Finite backward deletion
    // is a heuristic, with the global-only representation as an exact fallback.
    // Four-byte model references and 25-byte index entries cost the same for
    // every selection; each retained model's length prefix is charged here.
    for (kind, heads, payloads) in [
        (0, &mut dc_heads, &dc_payloads),
        (1, &mut ac_heads, &ac_payloads),
    ] {
        let cost = |active: &[usize]| -> usize {
            active
                .iter()
                .map(|&m| heads[m].len() + crate::varint_len(heads[m].len() as u64))
                .sum::<usize>()
                + (0..count)
                    .map(|s| active.iter().map(|&m| payloads[m][s].len()).min().unwrap())
                    .sum::<usize>()
        };
        let mut active: Vec<_> = (0..sets).collect();
        while active.len() > 1 {
            let current = cost(&active);
            let next = (0..active.len())
                .map(|drop| {
                    active
                        .iter()
                        .enumerate()
                        .filter_map(|(i, &m)| (i != drop).then_some(m))
                        .collect::<Vec<_>>()
                })
                .min_by_key(|a| cost(a))
                .unwrap();
            if cost(&next) >= current {
                break;
            }
            active = next;
        }
        if cost(&[0]) <= cost(&active) {
            active = vec![0];
        }
        let chosen: Vec<_> = (0..count)
            .map(|s| {
                *active
                    .iter()
                    .min_by_key(|&&m| payloads[m][s].len())
                    .unwrap()
            })
            .collect();
        let used: Vec<_> = active.into_iter().filter(|m| chosen.contains(m)).collect();
        for (s, m) in chosen.into_iter().enumerate() {
            units.push(Unit {
                kind,
                ordinal: s,
                model: used.iter().position(|&a| a == m).unwrap(),
                bytes: payloads[m][s].clone(),
            });
        }
        *heads = used.iter().map(|&m| heads[m].clone()).collect();
    }
    units.sort_by_key(|u| (u.ordinal, u.kind));
    Candidate {
        version: 1,
        common: common(file),
        dc: dc_heads,
        ac: ac_heads,
        units,
    }
}
impl Candidate {
    fn write(&self) -> Vec<u8> {
        let magic: &[u8; 8] = match self.version {
            1 => b"IMIPAL01",
            2 => b"IMIPAL02",
            3 => b"IMIPAL03",
            _ => panic!("private palette version"),
        };
        let mut out = magic.to_vec();
        bytes(&mut out, &self.common);
        for heads in [&self.dc, &self.ac] {
            put_varint(&mut out, heads.len() as u64);
            for head in heads {
                bytes(&mut out, head);
            }
        }
        let mut entries = Vec::new();
        for u in &self.units {
            let at = out.len();
            if self.version == 2 && u.kind == 0 && self.dc.len() > 1 {
                put_varint(&mut out, u.model as u64);
            }
            out.extend_from_slice(&u.bytes);
            entries.push((u.kind, u.ordinal, u.model, at, out.len() - at));
        }
        let at = out.len();
        for (kind, ordinal, model, offset, len) in entries {
            out.push(kind);
            out.extend_from_slice(&(ordinal as u32).to_le_bytes());
            if self.version == 1 {
                out.extend_from_slice(&(model as u32).to_le_bytes());
            }
            out.extend_from_slice(&(offset as u64).to_le_bytes());
            out.extend_from_slice(&(len as u64).to_le_bytes());
        }
        out.extend_from_slice(&(at as u64).to_le_bytes());
        out.extend_from_slice(&(self.units.len() as u64).to_le_bytes());
        out.extend_from_slice(b"PALIDX01");
        out
    }
}
struct ReadPalette {
    dc: Vec<crate::v1::Parsed<'static>>,
    ac: Vec<super::super::super::LossyParsed<'static>>,
    units: Vec<(u8, usize, usize, usize, usize)>,
    inline_dc: Vec<Option<(usize, usize)>>,
}
impl ReadPalette {
    fn new(file: &[u8]) -> Self {
        let version = match &file[..8] {
            b"IMIPAL01" => 1,
            b"IMIPAL02" => 2,
            b"IMIPAL03" => 3,
            _ => panic!("private palette magic"),
        };
        assert_eq!(&file[file.len() - 8..], b"PALIDX01");
        let mut r = ByteReader::new(file);
        r.pos = 8;
        let common = take(&mut r);
        let p = parsed(common);
        let di = p.dc.as_ref().unwrap().info.clone();
        let mut dc = Vec::new();
        let mut ac = Vec::new();
        let n = r.varint().unwrap();
        assert!(n > 0 && n <= 16, "bounded native catalog");
        for _ in 0..n {
            let model = Self::dc_model(&di, take(&mut r), version >= 2);
            if version >= 2 {
                Self::same_geometry(p.dc.as_ref().unwrap(), &model);
            }
            dc.push(model);
        }
        let n = r.varint().unwrap();
        for _ in 0..n {
            let mut p = parsed(common);
            let head = take(&mut r);
            if !head.is_empty() || version == 1 {
                let tables =
                    Tables::read(&mut BitReader::new(head), LossyEntropy::RunLevel, None).unwrap();
                p.rans = Some(super::super::super::RansTables::new(&tables));
                p.tables = tables;
                p.local_ans_contexts = false;
                p.compact_states = false;
            } else {
                assert!(!p.local_ans_contexts && p.catalog_lab.is_none());
            }
            ac.push(p);
        }
        let footer = file.len() - 24;
        let index = u64::from_le_bytes(file[footer..][..8].try_into().unwrap()) as usize;
        let count = u64::from_le_bytes(file[footer + 8..][..8].try_into().unwrap()) as usize;
        if version == 1 {
            assert_eq!(index + count * 25, footer);
        }
        if version >= 2 {
            let d = p.dc.as_ref().unwrap();
            assert_eq!(
                count,
                d.stripe_count() * d.ngroups() + p.g.stripes() * p.ngroups()
            );
        }
        let mut units = Vec::new();
        let mut inline_dc = Vec::new();
        let mut next = r.pos;
        let mut ix = ByteReader::new(&file[index..footer]);
        let mut ordinals = [0usize; 2];
        let mut definitions = Vec::<(usize, usize)>::new();
        for _ in 0..count {
            let kind = ix.u8().unwrap();
            assert!(kind <= 1);
            let ordinal = u32::from_le_bytes(ix.bytes(4).unwrap().try_into().unwrap()) as usize;
            assert_eq!(ordinal, ordinals[kind as usize]);
            ordinals[kind as usize] += 1;
            let mut model = if version == 1 {
                u32::from_le_bytes(ix.bytes(4).unwrap().try_into().unwrap()) as usize
            } else {
                0
            };
            let mut at = u64::from_le_bytes(ix.bytes(8).unwrap().try_into().unwrap()) as usize;
            let mut len = u64::from_le_bytes(ix.bytes(8).unwrap().try_into().unwrap()) as usize;
            assert_eq!(at, next);
            assert!(at + len <= index);
            next = at + len;
            let mut local = None;
            if kind == 0 && version == 2 && dc.len() > 1 {
                let mut prefix = ByteReader::new(&file[at..at + len]);
                model = prefix.var_usize().unwrap();
                at += prefix.pos;
                len -= prefix.pos;
            } else if kind == 0 && version == 3 {
                let original = at;
                let mut prefix = ByteReader::new(&file[at..at + len]);
                let control = prefix.varint().unwrap();
                let value = (control >> 2) as usize;
                match control & 3 {
                    0 => assert_eq!(value, 0),
                    tag @ (1 | 2) => {
                        assert!(value > 0 && value <= 1024 * 1024);
                        let head_at = original + prefix.pos;
                        let head = prefix.bytes(value).unwrap();
                        if tag == 1 {
                            local = Some((head_at, value));
                        } else {
                            assert!(definitions.len() < 16);
                            model = dc.len();
                            let parsed = Self::dc_model(&di, head, true);
                            Self::same_geometry(&dc[0], &parsed);
                            dc.push(parsed);
                            definitions.push((ordinal, model));
                        }
                    }
                    3 => {
                        assert!(value < ordinal, "references are explicit and backward");
                        model = definitions
                            .iter()
                            .find(|(u, _)| *u == value)
                            .expect("reference must name a reusable definition")
                            .1;
                    }
                    _ => unreachable!(),
                }
                at += prefix.pos;
                len -= prefix.pos;
            }
            assert!(model < if kind == 0 { dc.len() } else { ac.len() });
            units.push((kind, ordinal, model, at, len));
            inline_dc.push(local);
        }
        if version >= 2 {
            let d = p.dc.as_ref().unwrap();
            assert_eq!(
                ordinals,
                [d.stripe_count() * d.ngroups(), p.g.stripes() * p.ngroups()]
            );
        }
        assert_eq!(ix.remaining(), 0);
        assert_eq!(next, index);
        Self {
            dc,
            ac,
            units,
            inline_dc,
        }
    }
    fn same_geometry(a: &crate::v1::Parsed<'_>, b: &crate::v1::Parsed<'_>) {
        assert_eq!(
            (a.hs, a.gcols, a.lanes),
            (b.hs, b.gcols, b.lanes),
            "DC model cannot relabel unit geometry"
        );
    }
    fn dc_model(info: &ImageInfo, bytes: &[u8], native: bool) -> crate::v1::Parsed<'static> {
        let (bank, head) = if native {
            let mut r = ByteReader::new(bytes);
            let bank = r.varint().unwrap();
            let head = &bytes[r.pos..];
            (
                if bank == 0 {
                    None
                } else {
                    Some(crate::seeds::bank(bank).expect("numeric bank"))
                },
                head,
            )
        } else {
            (None, bytes)
        };
        let mut r = ByteReader::new(head);
        let p = crate::v1::parse_head_nested(
            info.clone(),
            &mut r,
            &crate::parse::limits::Unrestricted,
            bank,
            Features::ALL,
        )
        .unwrap();
        if native {
            assert_eq!(r.remaining(), 0);
        }
        p
    }
    fn unit(&self, file: &[u8], entry: usize) -> Vec<u8> {
        let (kind, s, m, at, n) = self.units[entry];
        let payload = &file[at..at + n];
        if kind == 0 {
            let inline = self.inline_dc[entry]
                .map(|(at, n)| Self::dc_model(&self.dc[0].info, &file[at..at + n], true));
            let p = inline.as_ref().unwrap_or(&self.dc[m]);
            Self::same_geometry(&self.dc[0], p);
            let rows = p.hs.min(p.info.height as usize - (s / p.ngroups()) * p.hs);
            let (_, width, _) = p.group(s % p.ngroups());
            let rb = width * p.bpp();
            let mut out = vec![0; rows * rb];
            let om = crate::v1::OutMap::new(&p.info, &p.layout);
            let mut state = crate::v1::UnitRows::new_with(p, s, payload).unwrap();
            for row in out.chunks_mut(rb) {
                state.next_row(p, &om, row).unwrap();
            }
            out
        } else {
            let p = &self.ac[m];
            let g = p.group(s % p.ngroups()).1;
            let stripe = s / p.ngroups();
            let (a, b) = g.rows(stripe);
            let (q, _, _, _) = super::super::super::decode_stripe_raw_bytes(
                p,
                &g,
                stripe,
                payload,
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
        }
    }
    fn decode(&self, file: &[u8], threads: usize) -> Vec<Vec<u8>> {
        crate::par::par_map((0..self.units.len()).collect(), threads, |_, u| {
            self.unit(file, u)
        })
    }
}
fn check(file: &[u8], candidate: &[u8]) -> ReadPalette {
    let r = ReadPalette::new(candidate);
    let p = super::super::super::parse_lossy(file).unwrap();
    let (di, dp, _) = super::super::super::dc_image_lab(file).unwrap();
    for i in 0..r.units.len() {
        let actual = r.unit(candidate, i);
        let (kind, s, _, at, n) = r.units[i];
        let expected = if kind == 0 {
            let y = s * 8;
            dp[y * di.row_bytes()..][..8.min(di.height as usize - y) * di.row_bytes()].to_vec()
        } else {
            let (a, b) = p.g.rows(s);
            let (q, _, _, _) = super::super::super::decode_stripe_raw_bytes(
                &p,
                &p.g,
                s,
                p.stripes[s],
                Group::ALL,
                true,
                Vec::new(),
                Vec::new(),
            )
            .unwrap();
            q[..(b - a) * p.g.bw * NCH * 64]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect()
        };
        assert_eq!(actual, expected, "catalog coefficient unit {kind}/{s}");
        // All unrelated compressed units are absent. The retained prefix,
        // selected catalog and explicit entry suffice; no previous unit state.
        let mut sparse = candidate.to_vec();
        for (j, &(_, _, _, a, b)) in r.units.iter().enumerate() {
            if j != i {
                sparse[a..a + b].fill(0);
            }
        }
        assert_eq!(r.unit(&sparse, i), expected);
        assert!(n > 0 && at < candidate.len());
    }
    let expected = r.decode(candidate, 1);
    for t in [2, 4, 8] {
        assert_eq!(r.decode(candidate, t), expected);
    }
    r
}
#[test]
fn model_palette_oracle() {
    let info = ImageInfo::new(65, 145, Channels::Rgb, SampleType::U8);
    let px: Vec<_> = (0..info.row_bytes() * info.height as usize)
        .map(|v| (v * 37 + v / 127) as u8)
        .collect();
    let cfg = configuration(40.0, LossyColor::Xyb, LossyEntropy::RunLevel, 64);
    let file =
        super::super::super::encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg)
            .unwrap();
    for k in [1, 2, 4, 8] {
        let k = k.min(3);
        check(&file, &build(&file, k).write());
    }
    if let Some(path) = std::env::var_os("IMI_MODEL_PALETTE_MANIFEST") {
        measure(Path::new(&path));
    }
}
fn uniform_map_cost(file: &[u8]) -> Value {
    let p = super::super::super::parse_lossy(file).unwrap();
    let mut lengths = Vec::new();
    for s in 0..p.stripes.len() {
        let (a, b) = p.g.rows(s);
        let rows = b - a;
        let cols = p.g.bw;
        assert!(
            rows.is_multiple_of(2) && cols.is_multiple_of(2),
            "uniform 16x16 witness requires complete 2x2 parent groups"
        );
        let roles = vec![super::super::super::superblock::ROLE_16X16; rows * cols];
        let encoded = super::super::super::superblock::map::encode(&roles, rows, cols);
        let mut decoded = vec![0; roles.len()];
        super::super::super::superblock::map::decode(&encoded, rows, cols, &mut decoded).unwrap();
        assert_eq!(decoded, roles);
        lengths.push(encoded.len() + crate::varint_len(encoded.len() as u64));
    }
    json!({"forced_uniform16_map_and_length_bytes":lengths.iter().sum::<usize>(),"per_unit_bytes":lengths,"fixed8_map_bytes":0,"scope":"map grammar only; no coefficient encoding, BD rate, or proposed declaration bytes"})
}
fn measure(path: &Path) {
    let manifest: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let root = Path::new(manifest["output"].as_str().unwrap());
    std::fs::create_dir(root).unwrap();
    let file = std::fs::read(manifest["native_file"].as_str().unwrap()).unwrap();
    let mut candidates = Vec::new();
    let mut stats = Vec::new();
    for k in [1, 2, 4, 8] {
        let candidate = build(&file, k);
        let bytes = Arc::new(candidate.write());
        let reader = Arc::new(check(&file, &bytes));
        let payload: usize = candidate.units.iter().map(|u| u.bytes.len()).sum();
        let model_bytes: usize = candidate.dc.iter().chain(&candidate.ac).map(Vec::len).sum();
        stats.push(json!({"buckets":k,"bytes":bytes.len(),"payload_bytes":payload,"model_bytes":model_bytes,"other_bytes":bytes.len()-payload-model_bytes,"dc_models":candidate.dc.len(),"ac_models":candidate.ac.len(),"xxh3_128":digest(&bytes)}));
        std::fs::write(root.join(format!("palette-k{k}.private")), &**bytes).unwrap();
        candidates.push((bytes, reader));
    }
    std::fs::write(root.join("correctness.json"),serde_json::to_vec_pretty(&json!({"source_revision":manifest["source_revision"],"binary_xxh3_128":digest(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),"native_xxh3_128":digest(&file),"arms":stats,"uniform_transform_map":uniform_map_cost(&file),"contract":"exact DC pixels and AC coefficients; selected-unit access with unrelated payloads absent","scope":"two-pass ANS coefficient extraction including output allocation and canonical i32 serialization; no inverse/color/raster output, model parsing or encode throughput","catalog_pruning":"backward deletion with exact global-only fallback","model_support":"one count for every globally observed slot/class/token, added to bucket histograms","index_entry_bytes":25,"common_head":"same conservative native declaration in all candidates","maps":false,"nas_writes":0})).unwrap()).unwrap();
    let gate = zenbench::GateConfig::strict()
        .max_heavy_processes(4)
        .min_available_ram_mb(4096)
        .max_cpu_temp_c(Some(85.0))
        .max_wait(Duration::from_secs(30));
    let result = zenbench::run_gated(gate, |suite| {
        suite.plan().workload_id = Some(format!("dc-ac-palette-{}", digest(&file)));
        for threads in [1, 2, 4, 8] {
            suite.group(format!("entropy-decode-{threads}t"), |g| {
                g.config()
                    .min_rounds(24)
                    .max_rounds(24)
                    .auto_rounds(false)
                    .warmup_time(Duration::from_millis(200))
                    .max_wall_time(Duration::from_secs(180));
                for (name, i) in [
                    ("global", 0),
                    ("bucket2", 1),
                    ("bucket4", 2),
                    ("bucket8", 3),
                    ("aa-global", 0),
                ] {
                    let (bytes, reader) = candidates[i].clone();
                    g.bench(name, move |b| b.iter(|| reader.decode(&bytes, threads)));
                }
            });
        }
    });
    result.save(root.join("timing.json")).unwrap();
    let v: Value =
        serde_json::from_slice(&std::fs::read(root.join("timing.json")).unwrap()).unwrap();
    assert_eq!(v["incomplete"], false);
    assert_eq!(v["unreliable"], false);
    for g in v["comparisons"].as_array().unwrap() {
        assert_eq!(g["completed_rounds"], 24);
        assert!(
            g["samples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["gate_clean"] == true)
        );
    }
    std::fs::write(
        root.join("COMPLETE"),
        b"catalog, coefficient and paired entropy gates pass\n",
    )
    .unwrap();
}
