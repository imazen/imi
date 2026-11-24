//! Encoder candidate learning shared with the retained placement study.
//! Production framing lives in `model_refs`; the sentinel carrier below is
//! compiled only for the retained study's decoder tests. Candidate budgets
//! are encoder policies, not limits on the native model-reference grammar.
#[cfg(all(feature = "lab", test))]
use super::local_ans::{DecodedModel, Unit as Selection};
use super::*;
use alloc::boxed::Box;

pub(super) const HEAD: u64 = 0x3fff_ffff;
const MAX_MODELS: usize = 16;
#[cfg(all(feature = "lab", test))]
const MAX_BYTES: usize = 1024 * 1024;
const MAX_MODEL_BYTES: usize = 65536;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Global,
    RootCandidates,
    Inline,
    Catalog,
    Mixed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    pub mode: Mode,
    pub buckets: usize,
    pub fast_training: bool,
    /// Force legal alternatives only for positive decoder witnesses.
    pub witness: bool,
}

impl Settings {
    pub(super) fn uses_envelope(self) -> bool {
        self.mode != Mode::RootCandidates
    }
}

#[cfg(all(feature = "lab", test))]
pub(super) struct Catalog {
    pub models: Vec<DecodedModel>,
    mode: LossyEntropy,
    bank: Option<&'static crate::seeds::Bank>,
}

#[cfg(all(feature = "lab", test))]
impl Catalog {
    pub fn read(
        bytes: &[u8],
        mode: LossyEntropy,
        bank: Option<&'static crate::seeds::Bank>,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::TooLarge);
        }
        let mut r = ByteReader::new(bytes);
        if r.varint()? != 1 {
            return Err(Error::Unsupported("lab model catalog version"));
        }
        let n = r.var_usize()?;
        if n > MAX_MODELS {
            return Err(Error::TooLarge);
        }
        // Charge all resident expanded tables plus two concurrent inline models
        // before constructing any of them, independently of encoded lengths.
        admission.ac_models((n + 2) as u128)?;
        let mut models = Vec::new();
        models.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        for _ in 0..n {
            if r.varint()? != 1 {
                return Err(Error::Unsupported("lab catalog model role"));
            }
            let len = r.var_usize()?;
            if len > MAX_MODEL_BYTES {
                return Err(Error::TooLarge);
            }
            models.push(DecodedModel::read(r.bytes(len)?, mode, bank)?);
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("catalog tail"));
        }
        Ok(Self { models, mode, bank })
    }

    pub fn select<'a>(&'a self, bytes: &'a [u8]) -> Result<Selection<'a>, Error> {
        let mut r = ByteReader::new(bytes);
        let control = r.varint()?;
        let value = usize::try_from(control >> 2).map_err(|_| Error::TooLarge)?;
        let compact = control & 1 != 0;
        if compact && self.mode != LossyEntropy::RunLevel {
            return Err(Error::Corrupt("compact Huffman catalog unit"));
        }
        let (shared, inline) = if control & 2 == 0 {
            (
                if value == 0 {
                    None
                } else {
                    Some(
                        self.models
                            .get(value - 1)
                            .ok_or(Error::Corrupt("catalog model id"))?,
                    )
                },
                None,
            )
        } else {
            if value == 0 || value > MAX_MODEL_BYTES {
                return Err(Error::TooLarge);
            }
            (
                None,
                Some(Box::new(DecodedModel::read(
                    r.bytes(value)?,
                    self.mode,
                    self.bank,
                )?)),
            )
        };
        Ok(Selection {
            shared,
            inline,
            body: r.bytes(r.remaining())?,
            compact,
        })
    }
}

struct EncodingModel {
    tables: Tables,
    huf: Option<huf::HufTables>,
    bytes: Vec<u8>,
}
enum Symbols {
    Ans(Box<[Vec<Sym>; NL]>),
    Huffman(HuffmanStripe),
}

impl Symbols {
    fn new(sc: &StripeCoefs, mode: LossyEntropy) -> Self {
        if mode == LossyEntropy::RunLevel {
            let mut lanes = core::array::from_fn(|_| Vec::new());
            stripe_symbols(sc, sc.bw, &mut lanes);
            Self::Ans(Box::new(lanes))
        } else {
            let (counts, widths) = stripe_counts_widths(sc, sc.bw);
            let symbols = huf::stripe_syms(&counts, &widths, sc.nby * sc.bw);
            let (magnitudes, signs) = stripe_raw_split(sc, sc.bw, &counts, &widths);
            Self::Huffman(HuffmanStripe {
                symbols,
                magnitudes,
                signs,
            })
        }
    }
    fn histogram(&self, hist: &mut [Vec<Vec<u64>>], support: bool) {
        match self {
            Self::Ans(lanes) => {
                for s in lanes.iter().flatten() {
                    if s.ctx != crate::rans::RAW_CTX {
                        hist[0][s.ctx as usize][s.tok as usize] += 1;
                    }
                }
            }
            Self::Huffman(s) => {
                for (h, syms) in
                    hist.iter_mut()
                        .zip([&s.symbols.luma, &s.symbols.chroma, &s.symbols.wid])
                {
                    for &(ctx, tok) in syms {
                        h[ctx as usize][tok as usize] += 1;
                    }
                }
            }
        }
        if support {
            for n in hist.iter_mut().flatten().flatten() {
                *n = u64::from(*n != 0);
            }
        }
    }
}

fn empty_hist(mode: LossyEntropy) -> Vec<Vec<Vec<u64>>> {
    if mode == LossyEntropy::RunLevel {
        vec![vec![vec![0; coef::ALPHA]; mode.nraw()]]
    } else {
        vec![
            vec![vec![0; huf::CNT_ALPHA]; huf::LUMA_CTX],
            vec![vec![0; huf::CNT_ALPHA]; huf::CHROMA_CTX],
            vec![vec![0; huf::WID_ALPHA]; huf::WID_CTX],
        ]
    }
}

fn train(
    hist: &[Vec<Vec<u64>>],
    mode: LossyEntropy,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    bank: Option<&'static crate::seeds::Bank>,
    local: bool,
) -> EncodingModel {
    let mut writer = BitWriter::new();
    if mode == LossyEntropy::RunLevel {
        let mut tables = if local {
            Tables::build_local(&hist[0], cfg.max_ac_tables, effort, bank, table_k_mode(cfg))
        } else {
            Tables::build_seeded(
                &hist[0],
                4,
                cfg.max_ac_tables,
                mode,
                effort,
                bank,
                table_k_mode(cfg),
            )
            .0
        };
        if let Some(bank) = bank
            && tables.seeds.is_none()
        {
            tables.seeds = Some((bank, vec![None; tables.freq.len()]));
        }
        tables.write(&mut writer);
        EncodingModel {
            tables,
            huf: None,
            bytes: writer.finish(),
        }
    } else {
        let mut ht = huf::HufTables::build_budget(
            &hist[0],
            &hist[1],
            &hist[2],
            effort,
            bank,
            table_k_mode(cfg),
            local,
        )
        .0;
        if let Some(bank) = bank
            && ht.seeds.is_none()
        {
            ht.seeds = Some((bank, vec![None; ht.lens.len()]));
        }
        ht.write(&mut writer);
        EncodingModel {
            tables: Tables::placeholder(mode),
            huf: Some(ht),
            bytes: writer.finish(),
        }
    }
}

enum Encoder<'a> {
    Ans(EncodeTables<'a>),
    Huffman(huf::Encoder<'a>),
}
impl EncodingModel {
    fn encoder(&self) -> Encoder<'_> {
        match &self.huf {
            Some(h) => Encoder::Huffman(h.encoder()),
            None => Encoder::Ans(EncodeTables::new(&self.tables.freq, &self.tables.ctx_map)),
        }
    }
}

impl Encoder<'_> {
    fn price(&self, symbols: &Symbols, active: usize, compact: bool) -> Option<(usize, bool)> {
        match (self, symbols) {
            (Self::Ans(e), Symbols::Ans(s)) => {
                let (states, raw) = e.encoded_size(s, active)?;
                let initial: Vec<u8> = states[..active]
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect();
                let packed = if compact {
                    raw - 4 * active + compact::packed_len(&initial, active).ok()?
                } else {
                    usize::MAX
                };
                Some(if packed < raw {
                    (packed, true)
                } else {
                    (raw, false)
                })
            }
            (Self::Huffman(e), Symbols::Huffman(s)) => {
                let tables = e.tables_for_lab();
                let mut len = 0;
                for (syms, map) in [
                    (&s.symbols.luma, &tables.luma_map[..]),
                    (&s.symbols.chroma, &tables.chroma_map[..]),
                    (&s.symbols.wid, &tables.wid_map[..]),
                ] {
                    let n = e.section_len(syms, map)?;
                    len += crate::varint_len(n as u64) + n;
                }
                Some((
                    len + crate::varint_len(s.magnitudes.len() as u64)
                        + s.magnitudes.len()
                        + s.signs.len(),
                    false,
                ))
            }
            _ => None,
        }
    }
    fn emit(&self, symbols: &Symbols, active: usize, compact: bool) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        match (self, symbols) {
            (Self::Ans(e), Symbols::Ans(s)) => {
                encode_lanes_tables(e, s, active, &mut out);
                if compact {
                    out = compact::pack(&out, active)?;
                }
            }
            (Self::Huffman(e), Symbols::Huffman(s)) => {
                let t = e.tables_for_lab();
                for (syms, map) in [
                    (&s.symbols.luma, &t.luma_map[..]),
                    (&s.symbols.chroma, &t.chroma_map[..]),
                    (&s.symbols.wid, &t.wid_map[..]),
                ] {
                    let mut b = Vec::new();
                    e.encode_section(syms, map, &mut b);
                    put_varint(&mut out, b.len() as u64);
                    out.extend_from_slice(&b);
                }
                put_varint(&mut out, s.magnitudes.len() as u64);
                out.extend_from_slice(&s.magnitudes);
                out.extend_from_slice(&s.signs);
            }
            _ => return Err(Error::Corrupt("lab model coder mismatch")),
        }
        Ok(out)
    }
}

#[derive(Clone, Copy)]
struct Choice {
    model: usize,
    inline: bool,
    packed: bool,
    length: usize,
}
fn control(reference: usize, inline: bool, packed: bool) -> u64 {
    (reference as u64) * 4 + u64::from(inline) * 2 + u64::from(packed)
}

fn code(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    mode: LossyEntropy,
    bank: Option<&'static crate::seeds::Bank>,
    pulse: &dyn Pulse,
) -> Result<Coded, Error> {
    let settings = cfg.catalog_lab.expect("lab configuration");
    let symbols = crate::par::par_map(stripes.iter().collect(), cfg.threads, |_, s| {
        Symbols::new(s, mode)
    });
    let mut hist = empty_hist(mode);
    for s in &symbols {
        s.histogram(&mut hist, false);
    }
    let global = train(&hist, mode, cfg, effort, bank, false);
    let mut support = hist.clone();
    for n in support.iter_mut().flatten().flatten() {
        *n = u64::from(*n != 0);
    }
    let buckets = settings
        .buckets
        .clamp(1, MAX_MODELS)
        .min(symbols.len().max(1));
    let mut models = vec![global];
    if matches!(
        settings.mode,
        Mode::RootCandidates | Mode::Catalog | Mode::Mixed
    ) {
        for k in 0..buckets {
            let mut h = support.clone();
            for (u, s) in symbols.iter().enumerate() {
                if (u * buckets / symbols.len()).min(buckets - 1) == k {
                    s.histogram(&mut h, false);
                }
            }
            models.push(train(&h, mode, cfg, effort, bank, settings.fast_training));
        }
    }
    let inline = if matches!(settings.mode, Mode::Inline | Mode::Mixed) {
        crate::par::par_map(symbols.iter().collect(), cfg.threads, |_, s| {
            let mut h = empty_hist(mode);
            s.histogram(&mut h, false);
            train(&h, mode, cfg, effort, bank, settings.fast_training)
        })
    } else {
        Vec::new()
    };
    let encoders: Vec<_> = models.iter().map(EncodingModel::encoder).collect();
    let inline_encoders: Vec<_> = inline.iter().map(EncodingModel::encoder).collect();
    let prices: Vec<Vec<Choice>> = crate::par::par_map(
        symbols.iter().zip(stripes).enumerate().collect(),
        cfg.threads,
        |_, (u, (s, sc))| {
            let mut choices = Vec::new();
            for (model, e) in encoders.iter().enumerate() {
                if let Some((n, packed)) = e.price(
                    s,
                    sc.bw.min(NL),
                    cfg.compact_streams && settings.uses_envelope(),
                ) {
                    choices.push(Choice {
                        model,
                        inline: false,
                        packed,
                        length: n + if settings.uses_envelope() {
                            crate::varint_len(control(model, false, packed))
                        } else {
                            0
                        },
                    });
                }
            }
            if let Some(m) = inline.get(u)
                && m.bytes.len() <= MAX_MODEL_BYTES
                && let Some((n, packed)) =
                    inline_encoders[u].price(s, sc.bw.min(NL), cfg.compact_streams)
            {
                choices.push(Choice {
                    model: 0,
                    inline: true,
                    packed,
                    length: n
                        + m.bytes.len()
                        + crate::varint_len(control(m.bytes.len(), true, packed)),
                });
            }
            choices
        },
    );
    let cost = |active: &[usize]| -> usize {
        let root = models[active[0]].bytes.len();
        let catalog = 1
            + crate::varint_len(active.len().saturating_sub(1) as u64)
            + active[1..]
                .iter()
                .map(|&m| {
                    1 + crate::varint_len(models[m].bytes.len() as u64) + models[m].bytes.len()
                })
                .sum::<usize>();
        root + crate::varint_len(root as u64)
            + if settings.uses_envelope() {
                crate::varint_len(HEAD) + crate::varint_len(catalog as u64) + catalog
            } else {
                0
            }
            + prices
                .iter()
                .map(|c| {
                    let n = c
                        .iter()
                        .filter(|c| c.inline || active.contains(&c.model))
                        .map(|c| c.length)
                        .min()
                        .expect("global model covers symbols");
                    n + crate::varint_len(n as u64)
                })
                .sum::<usize>()
    };
    // Scope does not prescribe training: any candidate may supply the image's
    // root model. Retaining the whole-image learner unconditionally would give
    // it free activation while charging every alternative, biasing the study.
    let mut active = vec![0];
    for root in 0..models.len() {
        let mut trial = vec![root];
        if settings.uses_envelope() {
            trial.extend((0..models.len()).filter(|&m| m != root));
        }
        loop {
            let now = cost(&trial);
            let mut next = None;
            for at in 1..trial.len() {
                let mut candidate = trial.clone();
                candidate.remove(at);
                let n = cost(&candidate);
                if n <= now && next.as_ref().is_none_or(|(best, _)| n < *best) {
                    next = Some((n, candidate));
                }
            }
            match next {
                Some((_, a)) => trial = a,
                None => break,
            }
        }
        if cost(&trial) < cost(&active) {
            active = trial;
        }
    }
    if settings.witness && matches!(settings.mode, Mode::Catalog | Mode::Mixed) {
        active = (0..models.len()).collect();
    }
    if !settings.uses_envelope() {
        let root = active[0];
        let records = crate::par::par_map(
            symbols.iter().zip(stripes).collect(),
            cfg.threads,
            |_, (s, sc)| {
                pulse.check().map_err(|_| Error::Cancelled)?;
                // Ordinary assembly prices one common compact-state mode from
                // these raw payloads. This arm adds no private selector/head.
                encoders[root].emit(s, sc.bw.min(NL), false)
            },
        )
        .into_iter()
        .collect::<Result<Vec<_>, Error>>()?;
        return Ok((models[root].bytes.clone(), records));
    }
    // Encoder-internal bundle only. Assembly writes the ordinary model blob
    // and the catalog as separate, length-bounded native head records.
    let mut tb = Vec::new();
    put_varint(&mut tb, models[active[0]].bytes.len() as u64);
    tb.extend_from_slice(&models[active[0]].bytes);
    put_varint(&mut tb, 1);
    put_varint(&mut tb, (active.len() - 1) as u64);
    for &model in &active[1..] {
        put_varint(&mut tb, 1);
        put_varint(&mut tb, models[model].bytes.len() as u64);
        tb.extend_from_slice(&models[model].bytes);
    }
    let records = crate::par::par_map(
        symbols.iter().zip(stripes).enumerate().collect(),
        cfg.threads,
        |_, (u, (s, sc))| -> Result<Vec<u8>, Error> {
            pulse.check().map_err(|_| Error::Cancelled)?;
            let forced_inline = settings.witness
                && matches!(settings.mode, Mode::Inline | Mode::Mixed)
                && (settings.mode == Mode::Inline || u.is_multiple_of(3));
            let forced_model = active[u % active.len()];
            let choice = *prices[u]
                .iter()
                .filter(|c| {
                    !settings.witness
                        || if forced_inline {
                            c.inline
                        } else {
                            !c.inline && c.model == forced_model
                        }
                })
                .filter(|c| c.inline || active.contains(&c.model))
                .min_by_key(|c| (c.length, c.inline, c.model))
                .expect("priced choice");
            let encoder = if choice.inline {
                &inline_encoders[u]
            } else {
                &encoders[choice.model]
            };
            let body = encoder.emit(s, sc.bw.min(NL), choice.packed)?;
            let mut out = Vec::new();
            let reference = if choice.inline {
                inline[u].bytes.len()
            } else {
                active.iter().position(|&m| m == choice.model).unwrap()
            };
            put_varint(&mut out, control(reference, choice.inline, choice.packed));
            if choice.inline {
                out.extend_from_slice(&inline[u].bytes);
            }
            out.extend_from_slice(&body);
            debug_assert_eq!(out.len(), choice.length);
            Ok(out)
        },
    )
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    Ok((tb, records))
}

pub(super) fn encode(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    mode: LossyEntropy,
    pulse: &dyn Pulse,
) -> Result<(Vec<u8>, Vec<Vec<u8>>, Option<Coded>), Error> {
    let bank = seed_bank(cfg);
    let (tb, records) = code(stripes, cfg, effort, mode, bank, pulse)?;
    let explicit = bank
        .map(|_| code(stripes, cfg, effort, mode, None, pulse))
        .transpose()?;
    Ok((tb, records, explicit))
}

#[cfg(all(test, feature = "lab"))]
mod tests {
    use super::*;

    #[test]
    fn catalog_admits_expansion_before_models_and_rejects_bad_selectors() {
        struct Deny;
        impl crate::parse::limits::Admission for Deny {
            fn ac_models(&self, n: u128) -> Result<(), Error> {
                assert_eq!(n, 18);
                Err(Error::TooLarge)
            }
        }
        // No model bytes are present: the admission error must win before
        // model parsing/allocation, even for the largest admitted catalog.
        assert!(matches!(
            Catalog::read(&[1, 16], LossyEntropy::RunLevel, None, &Deny),
            Err(Error::TooLarge)
        ));
        let admission = crate::parse::limits::Unrestricted;
        assert!(matches!(
            Catalog::read(&[1, 17], LossyEntropy::RunLevel, None, &admission),
            Err(Error::TooLarge)
        ));
        assert!(matches!(
            Catalog::read(&[2, 0], LossyEntropy::RunLevel, None, &admission),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            Catalog::read(&[1, 1, 7], LossyEntropy::RunLevel, None, &admission),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            Catalog::read(&[1, 0, 0], LossyEntropy::RunLevel, None, &admission),
            Err(Error::Corrupt(_))
        ));
        for mode in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let catalog = Catalog::read(&[1, 0], mode, None, &admission).unwrap();
            assert!(catalog.select(&[4]).is_err()); // undefined catalog id
            assert!(catalog.select(&[2]).is_err()); // empty inline model
            let mut oversized = Vec::new();
            put_varint(&mut oversized, control(MAX_MODEL_BYTES + 1, true, false));
            assert!(matches!(catalog.select(&oversized), Err(Error::TooLarge)));
            assert!(catalog.select(&[0]).unwrap().model().is_none());
            if mode == LossyEntropy::SplitHuf {
                assert!(matches!(catalog.select(&[1]), Err(Error::Corrupt(_))));
            }
        }
    }

    #[test]
    fn model_prices_equal_serialized_lengths() {
        for bw in [1usize, 7, 16] {
            for sparse in [false, true] {
                let blocks = bw * 3;
                let mut q = vec![0i32; blocks * NCH * 64];
                let mut state = 0x91e1_0da5u32;
                for (i, v) in q.iter_mut().enumerate() {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    if !i.is_multiple_of(64) && (!sparse || state.is_multiple_of(17)) {
                        let magnitude = (1u32 << (state % 22)) + (state >> 13);
                        *v = (magnitude.min((1 << 22) - 1) as i32)
                            * if state & 64 == 0 { 1 } else { -1 };
                    }
                }
                let nz = q
                    .as_chunks::<64>()
                    .0
                    .iter()
                    .map(|b| {
                        ZIGZAG
                            .iter()
                            .enumerate()
                            .fold(0, |m, (i, &p)| m | u64::from(b[p as usize] != 0) << i)
                    })
                    .collect();
                let sc = StripeCoefs {
                    by0: 0,
                    nby: 3,
                    bw,
                    q,
                    nz,
                    aq: Vec::new(),
                    modes: Vec::new(),
                    splits: Vec::new(),
                };
                for mode in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                    let symbols = Symbols::new(&sc, mode);
                    let mut hist = empty_hist(mode);
                    symbols.histogram(&mut hist, false);
                    for local in [false, true] {
                        for bank in [None, crate::seeds::bank(1)] {
                            let model = train(
                                &hist,
                                mode,
                                &LossyConfig::default(),
                                crate::effort::Effort::new(0.0).unwrap(),
                                bank,
                                local,
                            );
                            let decoded = DecodedModel::read(&model.bytes, mode, bank)
                                .unwrap_or_else(|e| {
                                    panic!(
                                        "{mode:?} local={local} seed={} model read: {e:?}",
                                        bank.is_some()
                                    )
                                });
                            assert_eq!(decoded.tables.ctx_map, model.tables.ctx_map);
                            assert_eq!(decoded.tables.freq, model.tables.freq);
                            let encoder = model.encoder();
                            for compact in [false, true] {
                                let (length, packed) =
                                    encoder.price(&symbols, bw.min(NL), compact).unwrap();
                                let bytes = encoder.emit(&symbols, bw.min(NL), packed).unwrap();
                                assert_eq!(
                                    length,
                                    bytes.len(),
                                    "{mode:?} bw={bw} sparse={sparse} local={local} seed={} compact={compact}",
                                    bank.is_some()
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn native_catalog_preserves_positive_exact_overlays() {
        let info = ImageInfo::new(129, 65, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize)
            .map(|i| ((i * 37 + i / 23) ^ ((i / 991) * 173)) as u8)
            .collect();
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for seed_bank in [0, 1] {
                let mut cfg = LossyConfig {
                    quality: 90.0,
                    effort: 0.0,
                    entropy,
                    seed_bank,
                    stripe_block_rows: 4,
                    aq_gamma: 0.0,
                    exact_blocks: Some(32.0),
                    checksum: false,
                    unit_ordinals: false,
                    ..Default::default()
                };
                let baseline =
                    encode_lossy(&pixels, info.row_bytes(), &info, &Metadata::default(), &cfg)
                        .unwrap();
                assert!(
                    parse_lossy(&baseline).unwrap().exact.is_some(),
                    "actual exact overlay, not only requested tuning"
                );
                let expected = crate::decode(&baseline).unwrap();
                for mode in [Mode::Inline, Mode::Catalog, Mode::Mixed] {
                    cfg.catalog_lab = Some(Settings {
                        mode,
                        buckets: 2,
                        fast_training: true,
                        witness: true,
                    });
                    let file =
                        encode_lossy(&pixels, info.row_bytes(), &info, &Metadata::default(), &cfg)
                            .unwrap();
                    assert!(parse_lossy(&file).unwrap().exact.is_some());
                    for threads in [1, 2, 4, 8] {
                        let mut out = vec![0; info.row_bytes() * info.height as usize];
                        crate::decode_into_threads(&file, &mut out, info.row_bytes(), threads)
                            .unwrap();
                        assert_eq!(out, expected.1);
                    }
                }
            }
        }
    }

    #[test]
    fn native_catalog_preserves_u16_precision_alpha_and_padded_rows() {
        for bits in [10, 12, 16] {
            let info = ImageInfo::new(129, 65, Channels::Rgba, SampleType::U16)
                .with_significant_bits(&[bits; 4])
                .unwrap();
            let stride = info.row_bytes() + 16;
            let mut source = vec![0x71; stride * info.height as usize];
            let mask = (1u32 << bits) - 1;
            for y in 0..info.height as usize {
                for x in 0..info.width as usize * 4 {
                    let value = ((x * 977 + y * 173 + x * y * 13) as u32 & mask) as u16;
                    source[y * stride + x * 2..y * stride + x * 2 + 2]
                        .copy_from_slice(&value.to_ne_bytes());
                }
            }
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                let mut cfg = LossyConfig {
                    quality: 40.0,
                    effort: 0.0,
                    color: LossyColor::YCoCg,
                    entropy,
                    stripe_block_rows: 4,
                    aq_gamma: 0.0,
                    exact_blocks: Some(0.0),
                    checksum: false,
                    unit_ordinals: false,
                    ..Default::default()
                };
                let baseline =
                    encode_lossy(&source, stride, &info, &Metadata::default(), &cfg).unwrap();
                let expected = crate::decode(&baseline).unwrap();
                for mode in [Mode::Inline, Mode::Catalog, Mode::Mixed] {
                    cfg.catalog_lab = Some(Settings {
                        mode,
                        buckets: 2,
                        fast_training: true,
                        witness: true,
                    });
                    let file =
                        encode_lossy(&source, stride, &info, &Metadata::default(), &cfg).unwrap();
                    for threads in [1, 2, 4, 8] {
                        let mut actual = vec![0x35; stride * info.height as usize];
                        crate::decode_into_threads(&file, &mut actual, stride, threads).unwrap();
                        for y in 0..info.height as usize {
                            assert_eq!(
                                &actual[y * stride..y * stride + info.row_bytes()],
                                &expected.1[y * info.row_bytes()..(y + 1) * info.row_bytes()]
                            );
                            assert!(
                                actual[y * stride + info.row_bytes()..(y + 1) * stride]
                                    .iter()
                                    .all(|&v| v == 0x35)
                            );
                            // Alpha is coded exactly, at its declared depth.
                            for x in 0..info.width as usize {
                                assert_eq!(
                                    &actual[y * stride + x * 8 + 6..y * stride + x * 8 + 8],
                                    &source[y * stride + x * 8 + 6..y * stride + x * 8 + 8]
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn native_catalog_modes_retain_coefficients_and_pixels() {
        let info = ImageInfo::new(129, 145, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize)
            .map(|i| ((i * 37 + i / 23) ^ ((i / 991) * 173)) as u8)
            .collect();
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for seed in [0, 1] {
                let mut cfg = LossyConfig {
                    quality: 40.0,
                    effort: 0.0,
                    entropy,
                    seed_bank: seed,
                    stripe_block_rows: 4,
                    checksum: false,
                    unit_ordinals: false,
                    aq_gamma: 0.0,
                    exact_blocks: Some(0.0),
                    ..Default::default()
                };
                let baseline =
                    encode_lossy(&pixels, info.row_bytes(), &info, &Metadata::default(), &cfg)
                        .unwrap();
                let expected = crate::decode(&baseline).unwrap();
                let coefficients = quantized_lab_any(&baseline).unwrap();
                for mode in [Mode::Global, Mode::Inline, Mode::Catalog, Mode::Mixed] {
                    cfg.catalog_lab = Some(Settings {
                        mode,
                        buckets: 3,
                        fast_training: true,
                        witness: true,
                    });
                    let encoded =
                        encode_lossy(&pixels, info.row_bytes(), &info, &Metadata::default(), &cfg)
                            .unwrap();
                    assert_eq!(
                        quantized_lab_any(&encoded)
                            .unwrap_or_else(|e| panic!("{entropy:?} seed={seed} {mode:?}: {e:?}")),
                        coefficients,
                        "{entropy:?} seed={seed} {mode:?}"
                    );
                    assert_eq!(
                        crate::decode(&encoded).unwrap(),
                        expected,
                        "{entropy:?} seed={seed} {mode:?}"
                    );
                    let p = parse_lossy(&encoded).unwrap();
                    let catalog = p.catalog_lab.as_ref().unwrap();
                    if matches!(mode, Mode::Catalog | Mode::Mixed) {
                        assert!(catalog.models.len() >= 2);
                    }
                    if mode == Mode::Inline {
                        assert!(
                            p.stripes
                                .iter()
                                .all(|b| ByteReader::new(b).varint().unwrap() & 2 != 0)
                        );
                    }
                    let first = p.stripes[0];
                    let at = first.as_ptr() as usize - encoded.as_ptr() as usize;
                    let mut bad = encoded.clone();
                    bad[at] = ((MAX_MODELS + 1) * 4) as u8;
                    assert!(matches!(
                        crate::decode(&bad),
                        Err(Error::Corrupt("catalog model id"))
                    ));
                    for (s, bytes) in p.stripes.iter().enumerate() {
                        p.catalog_lab.as_ref().unwrap().select(bytes).unwrap();
                        // Scalar and every available native SIMD route consume
                        // exactly the same selected model and coefficient body.
                        let a = decode_stripe_raw_bytes(
                            &p,
                            &p.g,
                            s,
                            bytes,
                            Group::ALL,
                            false,
                            Vec::new(),
                            Vec::new(),
                        )
                        .unwrap();
                        let b = decode_stripe_raw_bytes(
                            &p,
                            &p.g,
                            s,
                            bytes,
                            Group::ALL,
                            true,
                            Vec::new(),
                            Vec::new(),
                        )
                        .unwrap();
                        let (start, end) = p.g.rows(s);
                        let blocks = (end - start) * p.g.bw * NCH;
                        // The raw entropy API deliberately exposes 64 spare
                        // coefficients for vector stores. Only its logical
                        // blocks/EOBs are samples; native pixels remain exact.
                        assert_eq!(
                            &a.0[..blocks * 64],
                            &b.0[..blocks * 64],
                            "{entropy:?} seed={seed} {mode:?} unit={s}"
                        );
                        assert_eq!(&a.1[..blocks], &b.1[..blocks]);
                        assert_eq!((a.2, a.3), (b.2, b.3));
                    }
                }
            }
        }
    }
}

pub(super) fn encode_native(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    mode: LossyEntropy,
    pulse: &dyn Pulse,
) -> Result<(Vec<u8>, Vec<Vec<u8>>, Option<Coded>), Error> {
    let mut cfg = cfg.clone();
    cfg.catalog_lab.get_or_insert(Settings {
        mode: Mode::Mixed,
        buckets: 4,
        fast_training: true,
        witness: false,
    });
    encode(stripes, &cfg, effort, mode, pulse)
}
