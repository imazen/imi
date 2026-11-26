//! Multi-pass bounded-memory lossy stripe analysis, pilot/RDO passes, and record emission.

use super::*;

#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) fn component_one_pass_records(
    info: &ImageInfo,
    cfg: &LossyConfig,
) -> Result<usize, Error> {
    Ok(one_pass::configure(plan(info, cfg)?, cfg)?.record_count(false))
}

/// Exact-block selection across bands (`exact::estimate_bits` and
/// `exact::select_mask` of the whole image): the run-level token and
/// residual prices a statistics pass measured, and each lane's luma EOB
/// context, carried from band to band.
/// What the bounded passes need from outside: source rows, the records the
/// nested pass spools for pass 2, and pass 2's output. Async, so one core
/// serves blocking callers (every call completes at once, driven by
/// `parse::payload::ready`) and a sans-IO session (a call waits until the
/// caller answers).
pub(super) trait PassIo {
    fn stop(&self) -> &dyn how_far::Stop {
        &how_far::NoPulse
    }
    /// Source rows of block rows `[by0, by1)` into `band` (resized) for
    /// `pass`; returns their first row.
    async fn rows(
        &mut self,
        pass: u8,
        by0: usize,
        by1: usize,
        band: &mut Vec<u8>,
    ) -> Result<usize, Error>;
    /// Keeps a record the nested pass coded until pass 2 asks for it.
    async fn store<'a>(&'a mut self, kind: Spooled, rec: &'a [u8]) -> Result<(), Error>;
    /// Record `t` of `kind` (in store order) into `buf`.
    async fn load(&mut self, kind: Spooled, t: usize, buf: &mut Vec<u8>) -> Result<(), Error>;
    /// One main record of pass 2, in file order.
    async fn emit(&mut self, id: u64, payload: &[u8]) -> Result<(), Error>;
    /// The completed native choices are available before this band is released.
    /// Legacy writers need only the byte payload; V4 serialization also uses
    /// this view to avoid decoding its just-encoded transform map.
    async fn emit_ac(&mut self, payload: &[u8], _stripe: &StripeCoefs) -> Result<(), Error> {
        self.emit(crate::parse::records::R_AC, payload).await
    }
}

/// [`PassIo`] over blocking callbacks.
#[cfg(all(test, feature = "encode"))]
pub(super) struct Callbacks<'a, F, S, L, E> {
    pub(super) rows: &'a mut F,
    pub(super) store: S,
    pub(super) load: L,
    pub(super) emit: E,
}

#[cfg(all(test, feature = "encode"))]
impl<F, S, L, E> PassIo for Callbacks<'_, F, S, L, E>
where
    F: FnMut(u8, usize, usize, &mut Vec<u8>) -> Result<usize, Error>,
    S: FnMut(Spooled, &[u8]) -> Result<(), Error>,
    L: FnMut(Spooled, usize, &mut Vec<u8>) -> Result<(), Error>,
    E: FnMut(u64, &[u8]) -> Result<(), Error>,
{
    async fn rows(
        &mut self,
        pass: u8,
        by0: usize,
        by1: usize,
        band: &mut Vec<u8>,
    ) -> Result<usize, Error> {
        (self.rows)(pass, by0, by1, band)
    }
    async fn store<'a>(&'a mut self, kind: Spooled, rec: &'a [u8]) -> Result<(), Error> {
        (self.store)(kind, rec)
    }
    async fn load(&mut self, kind: Spooled, t: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        (self.load)(kind, t, buf)
    }
    async fn emit(&mut self, id: u64, payload: &[u8]) -> Result<(), Error> {
        (self.emit)(id, payload)
    }
}

/// A [`Callbacks`] hook a pass does not use.
#[cfg(all(test, feature = "encode"))]
pub(super) fn no_store(_: Spooled, _: &[u8]) -> Result<(), Error> {
    Err(Error::Corrupt(
        "bounded lossy: store outside the nested pass",
    ))
}
#[cfg(all(test, feature = "encode"))]
pub(super) fn no_load(_: Spooled, _: usize, _: &mut Vec<u8>) -> Result<(), Error> {
    Err(Error::Corrupt("bounded lossy: load outside pass 2"))
}
#[cfg(all(test, feature = "encode"))]
pub(super) fn no_emit(_: u64, _: &[u8]) -> Result<(), Error> {
    Err(Error::Corrupt("bounded lossy: record outside pass 2"))
}

pub(super) struct Select {
    pub(super) pol: exact::Policy,
    pub(super) tokens: Vec<[f32; coef::ALPHA]>,
    pub(super) proxy: Vec<[f32; 1024]>,
    pub(super) lanes: Vec<u32>,
}

impl Select {
    /// Measures the prices: one source pass (pass 0).
    pub(super) async fn measure<I: PassIo>(
        p: &Plan,
        pol: exact::Policy,
        threads: usize,
        io: &mut I,
    ) -> Result<Self, Error> {
        let mode = LossyEntropy::RunLevel;
        let mut lanes = vec![0u32; NL.min(p.g.bw)];
        let mut hist = vec![0u64; mode.nraw() * coef::ALPHA];
        let mut proxy = vec![[0u64; 1024]; 3];
        let mut band = Vec::new();
        for (by0, by1) in p.bands() {
            let y0 = io.rows(0, by0, by1, &mut band).await?;
            let stripes = p.band_stripes(&band, y0, by0, by1);
            for (l, prev) in lanes.iter_mut().enumerate() {
                exact::lane_hist(&stripes, l, prev, &mut hist);
            }
            let (_, y1) = p.band_rows(by0, by1);
            exact::proxy_hist(&band, p.rb, p.nci, p.g.w, y1 - y0, threads, &mut proxy);
        }
        let hist: Vec<Vec<u64>> = hist.chunks(coef::ALPHA).map(|h| h.to_vec()).collect();
        lanes.fill(0);
        Ok(Self {
            pol,
            tokens: exact::token_costs(&hist),
            proxy: exact::proxy_costs_of(&proxy),
            lanes,
        })
    }

    /// Restarts the lanes for another pass over the image.
    pub(super) fn begin(&mut self) {
        self.lanes.fill(0);
    }

    /// The exact blocks of band `[by0, by1)` (`stripes`, from `band`, rows
    /// `y0..`), band-local raster order; marks them in `stripes`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn band(
        &mut self,
        p: &Plan,
        stripes: &mut [StripeCoefs],
        band: &[u8],
        y0: usize,
        by0: usize,
        by1: usize,
        threads: usize,
    ) -> Vec<bool> {
        let bw = p.g.bw;
        let mut bits = vec![0.0f32; (by1 - by0) * bw];
        for (l, prev) in self.lanes.iter_mut().enumerate() {
            for (b, v) in exact::lane_bits(stripes, l, prev, &self.tokens) {
                bits[b as usize - by0 * bw] = v;
            }
        }
        let (_, y1) = p.band_rows(by0, by1);
        let mask = exact::select_with(
            band,
            p.rb,
            p.nci,
            p.g.w,
            y1 - y0,
            &bits,
            &self.proxy,
            &self.pol,
            threads,
        );
        exact::mark_from(stripes, &mask, by0);
        mask
    }
}

/// The exact blocks' source pixels packed into the nested image's stripes in
/// list order (unit by unit, raster order inside a unit), as `exact::encode`
/// packs them: B blocks per packed row, partial edge blocks repeating the
/// image edge, the last packed row zero padded.
pub(super) struct Packer {
    pub(super) b: usize,
    /// Blocks per nested stripe.
    pub(super) per: usize,
    pub(super) buf: Vec<u8>,
    pub(super) n: usize,
    /// Exact blocks so far.
    pub(super) total: usize,
}

impl Packer {
    pub(super) fn new(p: &Plan, pol: &exact::Policy) -> Self {
        let b = p.g.bw.min(exact::ROW_BLOCKS);
        Self {
            b,
            per: pol.stripe_rows / 8 * b,
            buf: vec![0u8; 8 * b * pol.stripe_rows * 3],
            n: 0,
            total: 0,
        }
    }

    /// Packed-image row bytes.
    pub(super) fn stride(&self) -> usize {
        8 * self.b * 3
    }

    /// Adds the `mask`ed blocks of band `[by0, by1)` (`band`, rows `y0..`)
    /// unit by unit, pushing each unit's count to `counts` and handing every
    /// complete stripe (pixels, rows) to `full`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn band(
        &mut self,
        p: &Plan,
        band: &[u8],
        y0: usize,
        by0: usize,
        by1: usize,
        mask: &[bool],
        counts: &mut Vec<usize>,
        full: &mut dyn FnMut(&[u8], usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let (w, h, bw) = (p.g.w, p.g.h, p.g.bw);
        let gb = if p.gcols == 0 { bw } else { p.gcols / 8 };
        let sb = p.sb as usize;
        for s in by0 / sb..by1.div_ceil(sb) {
            for gi in 0..p.ng {
                let before = self.total;
                for by in s * sb..((s + 1) * sb).min(p.g.bh) {
                    for bx in gi * gb..((gi + 1) * gb).min(bw) {
                        if !mask[(by - by0) * bw + bx] {
                            continue;
                        }
                        let (ox, oy) = ((self.n % self.b) * 8, (self.n / self.b) * 8);
                        let stride = self.stride();
                        for dy in 0..8 {
                            let y = (by * 8 + dy).min(h - 1) - y0;
                            for dx in 0..8 {
                                let x = (bx * 8 + dx).min(w - 1);
                                let o = (oy + dy) * stride + (ox + dx) * 3;
                                self.buf[o..o + 3]
                                    .copy_from_slice(&band[y * p.rb + x * p.nci..][..3]);
                            }
                        }
                        self.n += 1;
                        self.total += 1;
                        if self.n == self.per {
                            full(&self.buf, self.per / self.b * 8)?;
                            self.buf.fill(0);
                            self.n = 0;
                        }
                    }
                }
                counts.push(self.total - before);
            }
        }
        Ok(())
    }

    /// Hands the last, partial stripe (its packed rows) to `full`.
    pub(super) fn finish(
        &mut self,
        full: &mut dyn FnMut(&[u8], usize) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.n != 0 {
            full(&self.buf, self.n.div_ceil(self.b) * 8)?;
            self.buf.fill(0);
            self.n = 0;
        }
        Ok(())
    }
}

/// Exact blocks as pass 1 settles them.
pub(super) struct ExactPrep {
    pub(super) sel: Select,
    /// Exact blocks per unit, in unit order.
    pub(super) counts: Vec<usize>,
    /// Blocks per nested stripe.
    pub(super) per: usize,
    /// The exact head (SPEC: B, every unit's count, the nested head, every
    /// nested stripe's size).
    pub(super) head: Vec<u8>,
    /// Prepared numeric head inside the exact metadata; no decoder expansion.
    #[cfg(feature = "std")]
    pub(super) model: core::ops::Range<usize>,
    /// Nested stripes.
    pub(super) stripes: usize,
}

/// The AC tables pass 1 settles.
pub(super) enum Ac {
    Rl(Tables),
    Huf(huf::HufTables),
}

/// What pass 1 settles before the first byte of output.
pub(super) struct Prepared {
    #[cfg(feature = "lab")]
    pub(super) first_band: Option<Vec<u8>>,
    pub(super) entropy: LossyEntropy,
    pub(super) ac: Ac,
    /// The AC table blob as the head carries it.
    pub(super) tb: Vec<u8>,
    pub(super) fits16: bool,
    pub(super) dc_info: ImageInfo,
    pub(super) dc_session: GroupEncodeSession,
    pub(super) dc_head: Vec<u8>,
    /// The nested alpha image's session and head, with alpha.
    pub(super) alpha: Option<(GroupEncodeSession, Vec<u8>)>,
    /// The trellis passes every coded stripe takes (empty without trellis).
    pub(super) passes: Vec<trellis::Pass>,
    /// Exact blocks, when any block qualifies.
    pub(super) exact: Option<ExactPrep>,
    /// Superblock merges or small transforms: the selector, and what it
    /// chose anywhere (then every AC record carries its unit's map).
    pub(super) merges: Option<(superblock::select::Selector, Maps)>,
    /// Compact streaming open-loop trellis-lambda AQ model (`n_bands * bw * 4 B`).
    pub(super) open_lambda: Option<adaptive_rdo::OpenLoopLambdaModel>,
    /// Compact streaming closed-loop seam residual feedback model
    /// (`<= 2 MiB` exact `f64` up to 16 MPx, `1 B/block` quantized `u8` above).
    pub(super) closed_lambda: Option<adaptive_rdo::ClosedLoopLambdaModel>,
}

/// What the selector chose anywhere in the image.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Maps {
    /// A parent merged (16x16, 16x8 or 8x16).
    pub(super) merge: bool,
    /// A plane split into four 4x4 transforms.
    pub(super) split: bool,
}

impl Maps {
    /// The AC records lead with joint maps (head record 23): some plane
    /// splits. As `encode_lossy_frame`'s `MapKind::Joint`.
    pub(super) fn joint(self) -> bool {
        self.split
    }
    /// The AC records lead with merge maps (head record 21).
    pub(super) fn merges_only(self) -> bool {
        self.merge && !self.split
    }
}

#[cfg(all(test, feature = "encode"))]
pub(super) fn fetcher<'a>(
    p: &'a Plan,
    rows: &'a mut dyn FnMut(u8, usize, &mut [u8]) -> Result<(), Error>,
) -> impl FnMut(u8, usize, usize, &mut Vec<u8>) -> Result<usize, Error> + 'a {
    move |pass, by0, by1, band| {
        let (y0, y1) = p.band_rows(by0, by1);
        let n = (y1 - y0).checked_mul(p.rb).ok_or(Error::TooLarge)?;
        band.clear();
        band.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        band.resize(n, 0);
        rows(pass, y0, band)?;
        if p.input.is_some() {
            let mut info = p.info.clone();
            info.height = (y1 - y0) as u32;
            precision::validate_pixels(&info, band, p.rb)?;
        }
        Ok(y0)
    }
}

/// The payload of the one `R_STRIPE` record a group session just produced,
/// with its output consumed.
pub(super) fn take_stripe_record(session: &mut GroupEncodeSession) -> Result<Vec<u8>, Error> {
    let framed = session.output();
    let mut r = ByteReader::new(framed);
    if r.varint()? != crate::v1::R_STRIPE {
        return Err(Error::Corrupt("bounded nested record"));
    }
    let len = r.varint()? as usize;
    let payload = framed[r.pos..r.pos + len].to_vec();
    let n = framed.len();
    session.consume_output(n)?;
    Ok(payload)
}

/// Fills `dc` with the DC image rows of block rows `[by0, by1)` from that
/// band's stripes, as the whole-image encoder lays them out: U16 (value +
/// 32768) or native-endian I32, three channels, or four with block `codes`
/// (exact blocks).
pub(super) fn dc_band(
    p: &Plan,
    stripes: &[StripeCoefs],
    by0: usize,
    by1: usize,
    fits16: bool,
    codes: bool,
    dc: &mut Vec<u8>,
) {
    let dcs = if fits16 { 2 } else { 4 };
    let dch = if codes { 4 } else { NCH };
    let dc_stride = p.g.bw * dch * dcs;
    dc.clear();
    dc.resize((by1 - by0) * dc_stride, 0);
    for sc in stripes {
        for r in 0..sc.nby {
            for bx in 0..p.g.bw {
                let b = r * p.g.bw + bx;
                let o0 = (sc.by0 + r - by0) * dc_stride + bx * dch * dcs;
                let put = |dc: &mut Vec<u8>, c: usize, v: i32| {
                    let o = o0 + c * dcs;
                    if fits16 {
                        dc[o..o + 2].copy_from_slice(&(v as u16).to_ne_bytes());
                    } else {
                        dc[o..o + 4].copy_from_slice(&v.to_ne_bytes());
                    }
                };
                for c in 0..NCH {
                    let v = sc.q[(b * NCH + c) * 64];
                    put(dc, c, if fits16 { v + 32768 } else { v });
                }
                if codes {
                    put(dc, 3, sc.aq[b] as i32);
                }
            }
        }
    }
}

/// Pass 1: the DC range, the tier's side statistics and both coders'
/// histograms; then the entropy mode, its tables and the nested DC head.
/// Records the nested pass codes ahead of pass 2, which places them by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Spooled {
    /// A nested exact-image stripe.
    Exact,
    /// A DC group record of a DC stripe spanning several work bands.
    Dc,
}

/// Codes one nested exact stripe (`px`, rows `stride` bytes apart) and
/// queues its record in `coded` for storing.
pub(super) fn code_exact(
    session: &mut GroupEncodeSession,
    px: &[u8],
    stride: usize,
    sizes: &mut Vec<usize>,
    coded: &mut Vec<Vec<u8>>,
) -> Result<(), Error> {
    session.push_group(px, stride)?;
    let rec = take_stripe_record(session)?;
    sizes.push(rec.len());
    coded.push(rec);
    Ok(())
}

/// Helper: fills `out` with the current band's open-loop lambda multipliers
/// from `model`/`cursor` (no-op when `model` is `None`).
#[allow(clippy::too_many_arguments)]
pub(super) fn fill_band_open_lambda(
    p: &Plan,
    model: Option<&adaptive_rdo::OpenLoopLambdaModel>,
    cursor: &mut Option<adaptive_rdo::OpenLoopBandCursor>,
    band: &[u8],
    y0: usize,
    by0: usize,
    by1: usize,
    out: &mut Vec<f32>,
) -> Result<(), Error> {
    out.clear();
    if let (Some(m), Some(c)) = (model, cursor.as_mut()) {
        let rows = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
            p.fill_block_row(band, y0, by, planes, extremes)
        };
        m.fill_band(c, by0, by1, &rows, p.pw, &p.steps, out)?;
    }
    Ok(())
}

/// Pass 5 (when `p.merges.is_none()` and `p.needs_open_lambda()`): streams
/// the 3-row sliding-window open-loop trellis-lambda AQ model in `O(width)` RAM.
pub(super) async fn measure_open_lambda<I: PassIo>(
    p: &Plan,
    io: &mut I,
) -> Result<adaptive_rdo::OpenLoopLambdaModel, Error> {
    let mut stream = adaptive_rdo::OpenLoopLambdaStream::new(
        p.g.bw,
        p.g.bh,
        p.work,
        &p.steps,
        p.plan.lambda_aq_open,
    )?;
    let plane_len = NCH
        .checked_mul(8)
        .and_then(|v| v.checked_mul(p.pw))
        .ok_or(Error::TooLarge)?;
    let mut planes = adaptive_rdo::try_vec(plane_len, 0.0f32)?;
    let mut extremes = adaptive_rdo::try_vec_cap(p.g.bw)?;
    let mut band = Vec::new();
    for (by0, by1) in p.bands() {
        let y0 = io.rows(5, by0, by1, &mut band).await?;
        for by in by0..by1 {
            io.stop().check().map_err(|_| Error::Cancelled)?;
            extremes.clear();
            p.fill_block_row(&band, y0, by, &mut planes, &mut extremes);
            stream.push_row(by, &planes, p.pw, &p.steps);
        }
    }
    stream.finish()
}

/// Pass 4: the superblock selector's prices from the units' token counts
/// before any merge (exact blocks marked), as `superblock::select::apply`
/// derives them from the whole image's units. When open-loop lambda AQ is
/// planned, also streams `OpenLoopLambdaStream` in the same pass (zero extra
/// source pass). When `p.plan.two_pass_pilot` is active, runs streaming pilot
/// passes 5 (and 6 when trellis runs) to learn empirical partition-map, 4x4
/// flag, and post-trellis token tables in fixed ~25 KiB RAM.
pub(super) async fn merge_prices<I: PassIo>(
    p: &Plan,
    cfg: &LossyConfig,
    opts: superblock::select::Opts,
    sel: &mut Option<Select>,
    io: &mut I,
) -> Result<
    (
        superblock::select::Selector,
        Option<adaptive_rdo::OpenLoopLambdaModel>,
    ),
    Error,
> {
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let mut base_hist = TokenHistogram::new(opts.split_huf);
    let mut band = Vec::new();
    let mut open_stream = if p.needs_open_lambda() {
        Some(adaptive_rdo::OpenLoopLambdaStream::new(
            p.g.bw,
            p.g.bh,
            p.work,
            &p.steps,
            p.plan.lambda_aq_open,
        )?)
    } else {
        None
    };
    let mut open_planes = if open_stream.is_some() {
        let plane_len = NCH
            .checked_mul(8)
            .and_then(|v| v.checked_mul(p.pw))
            .ok_or(Error::TooLarge)?;
        adaptive_rdo::try_vec(plane_len, 0.0f32)?
    } else {
        Vec::new()
    };
    let mut open_extremes = if open_stream.is_some() {
        adaptive_rdo::try_vec_cap(p.g.bw)?
    } else {
        Vec::new()
    };
    if let Some(sel) = sel {
        sel.begin();
    }
    for (by0, by1) in p.bands() {
        let y0 = io.rows(4, by0, by1, &mut band).await?;
        if let Some(stream) = &mut open_stream {
            for by in by0..by1 {
                io.stop().check().map_err(|_| Error::Cancelled)?;
                open_extremes.clear();
                p.fill_block_row(&band, y0, by, &mut open_planes, &mut open_extremes);
                stream.push_row(by, &open_planes, p.pw, &p.steps);
            }
        }
        let mut stripes = p.band_stripes(&band, y0, by0, by1);
        if let Some(sel) = sel {
            sel.band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
        }
        for sc in stripes {
            for u in p.units(sc) {
                base_hist.add_unit(&u);
            }
        }
    }
    let open_model = match open_stream {
        Some(s) => Some(s.finish()?),
        None => None,
    };
    if !p.plan.two_pass_pilot {
        return Ok((
            superblock::select::Selector::from_counts(
                &base_hist.rl,
                &base_hist.freqs,
                p.pw,
                p.gcols / 8,
                &p.steps,
                p.round,
                cfg,
                effort,
                opts,
            ),
            open_model,
        ));
    }

    // Streaming two-pass pilot (M3): Pass 5 gathers empirical map/flag tables
    // and post-selection histograms; Pass 6 (when trellis runs) gathers
    // post-1-pass-trellis token histograms; both hold only ~25 KiB retained state.
    let mut pilot_opts = opts;
    pilot_opts.rdo.trellis_select = false;
    let pilot_sel = superblock::select::Selector::from_counts(
        &base_hist.rl,
        &base_hist.freqs,
        p.pw,
        p.gcols / 8,
        &p.steps,
        p.round,
        cfg,
        effort,
        pilot_opts,
    );
    let pilot_huf_model = opts.split_huf && trellis::huf_rate(cfg);
    let want_pilot_huf = if p.trellis.is_some() {
        pilot_huf_model
    } else {
        opts.split_huf
    };
    let mut map_stats = adaptive_rdo::MapBitStats::default();
    let mut flag_stats = adaptive_rdo::FlagStats::default();
    let mut pilot_hist = TokenHistogram::new(want_pilot_huf);
    let mut open_cursor = open_model.as_ref().map(|m| m.cursor(p.pw)).transpose()?;
    let mut band_open_lambda = Vec::new();
    if let Some(sel) = sel {
        sel.begin();
    }
    for (by0, by1) in p.bands() {
        let y0 = io.rows(5, by0, by1, &mut band).await?;
        let selected = prepare_pilot_band(
            p,
            cfg,
            &pilot_sel,
            sel,
            open_model.as_ref(),
            &mut open_cursor,
            &band,
            y0,
            by0,
            by1,
            &mut band_open_lambda,
            io.stop(),
        )?;
        for sc in selected {
            map_stats.add_stripe(&sc, p.gcols / 8)?;
            if opts.small.is_some() {
                flag_stats.add_stripe(&sc);
            }
            for u in p.units(sc) {
                pilot_hist.add_unit(&u);
            }
        }
    }
    let learned_map = map_stats.into_table();
    let learned_flags = opts.small.is_some().then(|| flag_stats.into_flags());

    let final_hist = if let Some(mut t_set) = p.trellis {
        t_set.passes = 1;
        let pilot_pass = pilot_hist.trellis_pass(&t_set, cfg, effort, pilot_huf_model);
        let mut learned_hist = TokenHistogram::new(opts.split_huf);
        if let Some(sel) = sel {
            sel.begin();
        }
        for (by0, by1) in p.bands() {
            let y0 = io.rows(6, by0, by1, &mut band).await?;
            let selected = prepare_pilot_band(
                p,
                cfg,
                &pilot_sel,
                sel,
                open_model.as_ref(),
                &mut open_cursor,
                &band,
                y0,
                by0,
                by1,
                &mut band_open_lambda,
                io.stop(),
            )?;
            let rows = |by: usize, planes: &mut [f32], extremes: &mut Vec<(u64, bool)>| {
                p.fill_block_row(&band, y0, by, planes, extremes)
            };
            for sc in selected {
                let mut units = p.units(sc);
                let rewrite_args = trellis::StripeRewrite {
                    rows: &rows,
                    pw: p.pw,
                    image_bw: p.g.bw,
                    group_bw: p.gcols / 8,
                    steps: &p.steps,
                    cfg,
                    set: &t_set,
                    aq: &[],
                    lambda_map: &band_open_lambda,
                    lambda_by0: by0,
                    merged_trellis: p.plan.merged_trellis,
                    bias: None,
                    pass_index: 0,
                };
                trellis::rewrite_stripe_lambda(
                    &pilot_pass,
                    &mut units,
                    &rewrite_args,
                    None,
                    io.stop(),
                )?;
                for u in &units {
                    learned_hist.add_unit(u);
                }
            }
        }
        learned_hist
    } else {
        pilot_hist
    };

    Ok((
        superblock::select::Selector::from_counts_learned(
            &final_hist.rl,
            &final_hist.freqs,
            p.pw,
            p.gcols / 8,
            &p.steps,
            p.round,
            cfg,
            effort,
            opts,
            Some(learned_map),
            learned_flags,
        ),
        open_model,
    ))
}

/// Computes open-loop lambdas, pass-1 stripes, exact-block markings, and pilot
/// partition merges for one band (shared by streaming pilot passes 5 and 6).
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare_pilot_band(
    p: &Plan,
    cfg: &LossyConfig,
    pilot_sel: &superblock::select::Selector,
    sel: &mut Option<Select>,
    open_model: Option<&adaptive_rdo::OpenLoopLambdaModel>,
    open_cursor: &mut Option<adaptive_rdo::OpenLoopBandCursor>,
    band: &[u8],
    y0: usize,
    by0: usize,
    by1: usize,
    band_open_lambda: &mut Vec<f32>,
    stop: &dyn how_far::Stop,
) -> Result<Vec<StripeCoefs>, Error> {
    fill_band_open_lambda(
        p,
        open_model,
        open_cursor,
        band,
        y0,
        by0,
        by1,
        band_open_lambda,
    )?;
    let mut stripes = p.band_stripes(band, y0, by0, by1);
    if let Some(sel) = sel {
        sel.band(p, &mut stripes, band, y0, by0, by1, cfg.threads);
    }
    p.merge_internal(
        pilot_sel,
        stripes,
        band,
        y0,
        band_open_lambda,
        by0,
        cfg.threads,
        stop,
    )
}

/// With exact blocks, pass 0 measures their prices first, and after the
/// trellis passes a pass codes the nested exact stripes, handing each to
/// `store_exact` in order (they are needed again, by index, in pass 2).
/// With superblock merges, pass 4 measures the selector's prices before
/// pass 1, and passes 1 and 2 merge each band's stripes.
pub(super) async fn pass1<I: PassIo>(
    p: &Plan,
    cfg: &LossyConfig,
    io: &mut I,
) -> Result<Prepared, Error> {
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let mut sel = match p.exact {
        Some(pol) => Some(Select::measure(p, pol, cfg.threads, io).await?),
        None => None,
    };
    let (merge_sel, open_lambda) = match p.merges {
        Some(opts) => {
            let (ms, ol) = merge_prices(p, cfg, opts, &mut sel, io).await?;
            (Some(ms), ol)
        }
        None if p.needs_open_lambda() => (None, Some(measure_open_lambda(p, io).await?)),
        None => (None, None),
    };
    let mut open_cursor = open_lambda.as_ref().map(|m| m.cursor(p.pw)).transpose()?;
    let mut band_open_lambda = Vec::new();
    let mut maps = Maps::default();
    if let Some(sel) = &mut sel {
        sel.begin();
    }
    let mut band = Vec::new();
    let mut fits16 = true;
    let mut side = [0u64; 3];
    let mut ac_hist = TokenHistogram::both();
    // The nested DC groups' token counts, for measured DC tables; collected
    // as U16 and discarded if any DC needs I32.
    let (dc16_info, dc_cfg) = p.dc_layout(true, false);
    let masks = dc_masks(cfg)?;
    let (dc_measure, _) = GroupEncodeSession::nested(dc16_info, dc_cfg)?;
    let dc_measure = dc_measure.with_analysis(masks.0, masks.1);
    let mut dc_hists = dc_measure.empty_histograms();
    let mut dc = Vec::new();
    // The current DC stripe's DC images (three channels; four with block
    // codes) and whether its DCs fit U16.
    let (mut dc3_acc, mut dc4_acc, mut stripe_fits) = (Vec::new(), Vec::new(), true);
    // With exact blocks the DC image carries block codes, unless no block
    // qualifies: both layouts' counts are collected.
    let dc4_measure = match &sel {
        Some(_) => Some(
            GroupEncodeSession::nested(p.dc_layout(true, true).0, dc_cfg)?
                .0
                .with_analysis(masks.0, masks.1),
        ),
        None => None,
    };
    let mut dc4_hists = dc4_measure.as_ref().map(|m| m.empty_histograms());
    // The packed exact image's token counts, for its measured tables.
    let mut packer = sel.as_ref().map(|s| Packer::new(p, &s.pol));
    let ex_measure = match (&sel, &packer) {
        (Some(s), Some(pk)) => {
            let w = (8 * pk.b) as u32;
            let rows = s.pol.stripe_rows as u32;
            Some(
                GroupEncodeSession::nested(
                    ImageInfo::new(w, rows, Channels::Rgb, SampleType::U8),
                    GroupEncodeConfig::new(w, rows, usize::MAX),
                )?
                .0
                .with_analysis(masks.0, masks.1),
            )
        }
        _ => None,
    };
    let mut ex_hists = ex_measure.as_ref().map(|m| m.empty_histograms());
    let mut counts = Vec::new();
    let alpha_measure = match &p.alpha {
        Some(ai) => Some(
            GroupEncodeSession::nested(ai.clone(), p.alpha_cfg())?
                .0
                .with_analysis(masks.0, masks.1),
        ),
        None => None,
    };
    let mut alpha_hists = alpha_measure.as_ref().map(|m| m.empty_histograms());
    let mut plane = Vec::new();
    for (by0, by1) in p.bands() {
        let y0 = io.rows(1, by0, by1, &mut band).await?;
        let mut stripes = p.band_stripes(&band, y0, by0, by1);
        if let (Some(sel), Some(pk), Some(m), Some(h)) =
            (&mut sel, &mut packer, &ex_measure, &mut ex_hists)
        {
            let mask = sel.band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
            let stride = pk.stride();
            let w = 8 * pk.b;
            pk.band(
                p,
                &band,
                y0,
                by0,
                by1,
                &mask,
                &mut counts,
                &mut |px, rows| {
                    m.group_histograms(w, rows, px, stride, h);
                    Ok(())
                },
            )?;
        }
        if let Some(ms) = &merge_sel {
            fill_band_open_lambda(
                p,
                open_lambda.as_ref(),
                &mut open_cursor,
                &band,
                y0,
                by0,
                by1,
                &mut band_open_lambda,
            )?;
            stripes = p.merge(
                ms,
                stripes,
                &band,
                y0,
                &band_open_lambda,
                by0,
                cfg.threads,
                io.stop(),
            )?;
            maps.merge |= p.merges.is_some_and(|o| o.merges)
                && stripes.iter().any(|sc| sc.modes.iter().any(|&m| m != 0));
            maps.split |= stripes.iter().any(|sc| sc.splits.iter().any(|&v| v != 0));
        }
        let band_fits = stripes.iter().all(|sc| {
            sc.q.as_chunks::<64>()
                .0
                .iter()
                .all(|b| (-32768..=32767).contains(&b[0]))
        });
        stripe_fits &= band_fits;
        if fits16 && stripe_fits {
            dc_band(p, &stripes, by0, by1, true, false, &mut dc);
            dc3_acc.extend_from_slice(&dc);
            if dc4_measure.is_some() {
                dc_band(p, &stripes, by0, by1, true, true, &mut dc);
                dc4_acc.extend_from_slice(&dc);
            }
        }
        if p.ends_dc_stripe(by1) {
            if fits16 && stripe_fits {
                let cols = dc_cfg.columns as usize;
                for (acc, dch, m, h) in [
                    (&dc3_acc, NCH, Some(&dc_measure), Some(&mut dc_hists)),
                    (&dc4_acc, 4, dc4_measure.as_ref(), dc4_hists.as_mut()),
                ] {
                    let (Some(m), Some(h)) = (m, h) else { continue };
                    let stride = p.g.bw * dch * 2;
                    let rows = acc.len() / stride;
                    for x0 in (0..p.g.bw).step_by(cols) {
                        let w = cols.min(p.g.bw - x0);
                        m.group_histograms(w, rows, &acc[x0 * dch * 2..], stride, h);
                    }
                }
            }
            fits16 &= stripe_fits;
            stripe_fits = true;
            dc3_acc.clear();
            dc4_acc.clear();
        }
        if let (Some(m), Some(h)) = (&alpha_measure, &mut alpha_hists) {
            let cols = p.alpha_cfg().columns as usize;
            for sc in &stripes {
                let rows = p.alpha_plane(&band, y0, sc.by0 / p.sb as usize, &mut plane);
                for x0 in (0..p.g.w).step_by(cols) {
                    m.group_histograms(
                        cols.min(p.g.w - x0),
                        rows,
                        &plane[x0 * p.bps..],
                        p.g.w * p.bps,
                        h,
                    );
                }
            }
        }
        for sc in stripes {
            let st = side_stats(core::slice::from_ref(&sc), p.g.bw, 1);
            for (a, b) in side.iter_mut().zip(st) {
                *a += b;
            }
            for u in p.units(sc) {
                ac_hist.add_unit(&u);
            }
        }
    }
    // Exact blocks only when some block qualifies (as `encode_lossy`).
    let exact_total = match (&mut packer, &ex_measure, &mut ex_hists) {
        (Some(pk), Some(m), Some(h)) => {
            let stride = pk.stride();
            let w = 8 * pk.b;
            pk.finish(&mut |px, rows| {
                m.group_histograms(w, rows, px, stride, h);
                Ok(())
            })?;
            pk.total
        }
        _ => 0,
    };
    if exact_total == 0 {
        sel = None;
    }
    // The tier rule (the whole-image encoder's rule for images above
    // `SMALL_IMAGE`; smaller ones there also compare both codings).
    let entropy = match cfg.decode_speed {
        None => cfg.entropy,
        Some(speed) => {
            let ratio = predicted_huf_ratio((p.g.w * p.g.h) as f64, &side);
            if huf_max_ratio(speed).is_some_and(|max| ratio <= max) {
                LossyEntropy::SplitHuf
            } else {
                LossyEntropy::RunLevel
            }
        }
    };
    // The AC trellis (efforts 9, 9.5, 10, XYB): one source pass per rate-model
    // pass, each recomputing the pass-1 (and merged) units of every band and
    // applying the passes so far, as `trellis::optimize` prices them; the last
    // one also gathers the final tables' counts. Pass 0 also feeds the
    // streaming closed-loop seam feedback collector when enabled.
    let mut passes: Vec<trellis::Pass> = Vec::new();
    let mut closed_lambda: Option<adaptive_rdo::ClosedLoopLambdaModel> = None;
    let huf_model = entropy == LossyEntropy::SplitHuf;
    if let Some(set) = p.trellis.filter(|_| !huf_model || trellis::huf_rate(cfg)) {
        let n = set.passes as usize;
        let mut freqs: trellis::Freqs = ac_hist.freqs.clone();
        let mut band_closed_lambda = Vec::new();
        for k in 0..n {
            passes.push(if huf_model {
                trellis::Pass::split_huf(&freqs, &set, cfg, effort)
            } else {
                trellis::Pass::run_level(&ac_hist.rl, &set, cfg, effort)
            });
            let last = k + 1 == n;
            let mut next = trellis::freqs_new();
            let mut closed_stream = if k == 0 && !last && p.plan.lambda_aq_closed_loop != 0.0 {
                Some(adaptive_rdo::ClosedLoopLambdaStream::new(
                    p.g.bw,
                    p.g.bh,
                    &p.steps,
                    p.plan.lambda_aq_closed_loop,
                )?)
            } else {
                None
            };
            ac_hist.clear_rl();
            if last {
                ac_hist.clear_freqs();
            }
            if let Some(sel) = &mut sel {
                sel.begin();
            }
            for (by0, by1) in p.bands() {
                let y0 = io.rows(10 + k as u8, by0, by1, &mut band).await?;
                fill_band_open_lambda(
                    p,
                    open_lambda.as_ref(),
                    &mut open_cursor,
                    &band,
                    y0,
                    by0,
                    by1,
                    &mut band_open_lambda,
                )?;
                band_closed_lambda.clear();
                if k > 0
                    && let Some(cm) = &closed_lambda
                {
                    cm.fill_band(&band_open_lambda, by0, by1, &mut band_closed_lambda)?;
                }
                let mut stripes = p.band_stripes(&band, y0, by0, by1);
                if let Some(sel) = &mut sel {
                    sel.band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
                }
                if let Some(ms) = &merge_sel {
                    stripes = p.merge(
                        ms,
                        stripes,
                        &band,
                        y0,
                        &band_open_lambda,
                        by0,
                        cfg.threads,
                        io.stop(),
                    )?;
                }
                for sc in stripes {
                    let mut units = p.units(sc);
                    let emit = (huf_model && !last).then_some(&mut next);
                    p.rewrite(
                        cfg,
                        &passes,
                        &mut units,
                        &band,
                        y0,
                        &band_open_lambda,
                        &band_closed_lambda,
                        by0,
                        emit,
                        closed_stream.as_mut(),
                        io.stop(),
                    )?;
                    for u in &units {
                        ac_hist.add_rl(u);
                        if last {
                            ac_hist.add_huf(u);
                        }
                    }
                }
            }
            if let Some(stream) = closed_stream {
                closed_lambda = Some(stream.finish());
            }
            freqs = next;
        }
    }
    let ac = match entropy {
        LossyEntropy::RunLevel => {
            let hist = ac_hist.rl_rows();
            Ac::Rl(
                Tables::build_seeded(
                    &hist,
                    4,
                    cfg.max_ac_tables,
                    LossyEntropy::RunLevel,
                    effort,
                    None,
                    table_k_mode(cfg),
                )
                .0,
            )
        }
        LossyEntropy::SplitHuf => {
            let [luma, chroma, wid] = &ac_hist.freqs;
            Ac::Huf(huf::HufTables::build(luma, chroma, wid, effort, None, table_k_mode(cfg)).0)
        }
    };
    let tb = {
        let mut header = BitWriter::new();
        match &ac {
            Ac::Rl(t) => t.write(&mut header),
            Ac::Huf(t) => t.write(&mut header),
        }
        header.finish()
    };
    // The nested DC image: lossless groups with tables measured in this pass
    // (fixed tables when a DC needs I32, whose counts were not collected).
    let codes = sel.is_some();
    let (dc_info, dc_cfg) = p.dc_layout(fits16, codes);
    let (dc_session, dc_head) = if fits16 {
        let hists = match &dc4_hists {
            Some(h) if codes => h,
            _ => &dc_hists,
        };
        let (session, head) = GroupEncodeSession::nested_measured(dc_info.clone(), dc_cfg, hists)?;
        (session.with_analysis(masks.0, masks.1), head)
    } else {
        GroupEncodeSession::nested(dc_info.clone(), dc_cfg)?
    };
    let alpha = match (&p.alpha, &alpha_hists) {
        (Some(ai), Some(h)) => {
            let (session, head) =
                GroupEncodeSession::nested_measured(ai.clone(), p.alpha_cfg(), h)?;
            Some((session.with_analysis(masks.0, masks.1), head))
        }
        _ => None,
    };
    // The nested pass (pass 3), with exact blocks or DC stripes taller than
    // a work band: it codes the nested exact image (lossless groups, one per
    // nested stripe, with the tables measured above), whose sizes the exact
    // head lists before the first unit record, and the DC records of DC
    // stripes spanning several bands, each once its DC image is complete.
    // Both are stored for pass 2, which places them by index.
    let mut dc_session = dc_session;
    let split = p.split_dc();
    let mut ex = match (sel, ex_hists) {
        (Some(sel), Some(hists)) => {
            let pk = Packer::new(p, &sel.pol);
            let w = (8 * pk.b) as u32;
            let info = ImageInfo::new(
                w,
                u32::try_from(exact_total.div_ceil(pk.b) * 8).map_err(|_| Error::TooLarge)?,
                Channels::Rgb,
                SampleType::U8,
            );
            let (session, nested_head) = GroupEncodeSession::nested_measured(
                info,
                GroupEncodeConfig::new(w, sel.pol.stripe_rows as u32, usize::MAX),
                &hists,
            )?;
            Some((
                sel,
                pk,
                session.with_analysis(masks.0, masks.1),
                nested_head,
            ))
        }
        _ => None,
    };
    let mut sizes = Vec::new();
    if ex.is_some() || split {
        let dch = if codes { 4 } else { NCH };
        let dc_stride = p.g.bw * dch * if fits16 { 2 } else { 4 };
        let mut again = Vec::new();
        let mut coded: Vec<Vec<u8>> = Vec::new();
        let (mut acc, mut dcb) = (Vec::new(), Vec::new());
        if let Some((sel, ..)) = &mut ex {
            sel.begin();
        }
        for (by0, by1) in p.bands() {
            let y0 = io.rows(3, by0, by1, &mut band).await?;
            let mut stripes = p.band_stripes(&band, y0, by0, by1);
            if let Some((sel, pk, session, _)) = &mut ex {
                let mask = sel.band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
                let stride = pk.stride();
                pk.band(p, &band, y0, by0, by1, &mask, &mut again, &mut |px, _| {
                    code_exact(session, px, stride, &mut sizes, &mut coded)
                })?;
                for rec in coded.drain(..) {
                    io.store(Spooled::Exact, &rec).await?;
                }
            }
            if !split {
                continue;
            }
            if let Some(ms) = &merge_sel {
                fill_band_open_lambda(
                    p,
                    open_lambda.as_ref(),
                    &mut open_cursor,
                    &band,
                    y0,
                    by0,
                    by1,
                    &mut band_open_lambda,
                )?;
                stripes = p.merge(
                    ms,
                    stripes,
                    &band,
                    y0,
                    &band_open_lambda,
                    by0,
                    cfg.threads,
                    io.stop(),
                )?;
            }
            dc_band(p, &stripes, by0, by1, fits16, codes, &mut dcb);
            acc.extend_from_slice(&dcb);
            if p.ends_dc_stripe(by1) {
                let first = by0 / p.dc_rows * p.dc_rows;
                for _ in 0..p.ng {
                    let rect = dc_session
                        .next_group()
                        .ok_or(Error::Corrupt("bounded DC group order"))?;
                    if rect.y as usize != first {
                        return Err(Error::Corrupt("bounded DC group order"));
                    }
                    let x = rect.x as usize * dc_stride / p.g.bw;
                    dc_session.push_group(&acc[x..], dc_stride)?;
                    let rec = take_stripe_record(&mut dc_session)?;
                    io.store(Spooled::Dc, &rec).await?;
                }
                acc.clear();
            }
        }
        if let Some((_, pk, session, _)) = &mut ex {
            let stride = pk.stride();
            pk.finish(&mut |px, _| code_exact(session, px, stride, &mut sizes, &mut coded))?;
            for rec in coded.drain(..) {
                io.store(Spooled::Exact, &rec).await?;
            }
            if again != counts || !session.is_finished() {
                return Err(Error::InvalidInput(
                    "source rows changed between bounded lossy passes",
                ));
            }
        }
        if split && !dc_session.is_finished() {
            return Err(Error::Corrupt("bounded nested image incomplete"));
        }
    }
    let exact = match ex {
        Some((sel, pk, _, nested_head)) => {
            let mut head = Vec::new();
            put_varint(&mut head, pk.b as u64);
            for &n in &counts {
                put_varint(&mut head, n as u64);
            }
            #[cfg(feature = "std")]
            let model_start = head.len();
            head.extend_from_slice(&nested_head);
            #[cfg(feature = "std")]
            let model = model_start..head.len();
            for &n in &sizes {
                put_varint(&mut head, n as u64);
            }
            Some(ExactPrep {
                sel,
                counts,
                per: pk.per,
                head,
                #[cfg(feature = "std")]
                model,
                stripes: sizes.len(),
            })
        }
        None => None,
    };
    Ok(Prepared {
        #[cfg(feature = "lab")]
        first_band: None,
        merges: merge_sel.map(|s| (s, maps)),
        open_lambda,
        closed_lambda,
        exact,
        passes,
        alpha,
        entropy,
        ac,
        tb,
        fits16,
        dc_info,
        dc_session,
        dc_head,
    })
}

impl Plan {
    /// Main records of the payload: every unit's AC record plus each column
    /// group's DC record per DC stripe (SPEC 2.7).
    pub(super) fn record_count(&self, exact: bool) -> usize {
        let units = self.g.stripes() * self.ng;
        let alpha = if self.alpha.is_some() { units } else { 0 };
        let exact = if exact { units } else { 0 };
        units + alpha + exact + self.g.bh.div_ceil(self.dc_rows) * self.ng
    }
}

/// Pass 2: recomputes each band and hands every main record, in decode
/// order (SPEC 2.7), to `io.emit(id, payload)`.
pub(super) async fn pass2<I: PassIo>(
    p: &Plan,
    cfg: &LossyConfig,
    prep: &mut Prepared,
    io: &mut I,
) -> Result<(), Error> {
    use crate::parse::records::{R_DC, R_EXACT};
    let split = p.split_dc();
    let k = p.dc_rows / p.sb as usize;
    let dcs = if prep.fits16 { 2 } else { 4 };
    let codes = prep.exact.is_some();
    let dch = if codes { 4 } else { NCH };
    if let Some(e) = &mut prep.exact {
        e.sel.begin();
    }
    // Exact records: unit `unit` carries the nested stripes whose first
    // block is among its own (list entries from `list`).
    let (mut unit, mut list, mut next_t) = (0usize, 0usize, 0usize);
    let mut stripe_bytes = Vec::new();
    let mut band = Vec::new();
    let mut dc = Vec::new();
    let mut plane = Vec::new();
    let encoder_rl = match &prep.ac {
        Ac::Rl(t) => Some(EncodeTables::new(&t.freq, &t.ctx_map)),
        Ac::Huf(_) => None,
    };
    // Canonical Huffman codes are immutable across every unit in this image,
    // as in the whole-image encoder. Compile them once, not per unit.
    let encoder_huf = match &prep.ac {
        Ac::Huf(t) => Some(t.encoder()),
        Ac::Rl(_) => None,
    };
    let mut open_cursor = prep
        .open_lambda
        .as_ref()
        .map(|m| m.cursor(p.pw))
        .transpose()?;
    let mut band_open_lambda = Vec::new();
    let mut band_closed_lambda = Vec::new();
    for (by0, by1) in p.bands() {
        #[cfg(feature = "lab")]
        let prefetched = prep.first_band.take();
        #[cfg(not(feature = "lab"))]
        let prefetched: Option<Vec<u8>> = None;
        let y0 = if let Some(first) = prefetched {
            debug_assert_eq!(by0, 0);
            band = first;
            0
        } else {
            io.rows(2, by0, by1, &mut band).await?
        };
        fill_band_open_lambda(
            p,
            prep.open_lambda.as_ref(),
            &mut open_cursor,
            &band,
            y0,
            by0,
            by1,
            &mut band_open_lambda,
        )?;
        band_closed_lambda.clear();
        if let Some(cm) = &prep.closed_lambda {
            cm.fill_band(&band_open_lambda, by0, by1, &mut band_closed_lambda)?;
        }
        let mut stripes = p.band_stripes(&band, y0, by0, by1);
        if let Some(e) = &mut prep.exact {
            e.sel
                .band(p, &mut stripes, &band, y0, by0, by1, cfg.threads);
        }
        if let Some((ms, _)) = &prep.merges {
            stripes = p.merge(
                ms,
                stripes,
                &band,
                y0,
                &band_open_lambda,
                by0,
                cfg.threads,
                io.stop(),
            )?;
        }
        // DC records: coded here when the band is the DC stripe, else stored
        // by the nested pass.
        let mut dc_records = Vec::with_capacity(p.ng);
        let dc_stride = p.g.bw * dch * dcs;
        if !split {
            dc_band(p, &stripes, by0, by1, prep.fits16, codes, &mut dc);
        }
        for _ in 0..if split { 0 } else { p.ng } {
            let rect = prep
                .dc_session
                .next_group()
                .ok_or(Error::Corrupt("bounded DC group order"))?;
            if rect.y as usize != by0 {
                return Err(Error::Corrupt("bounded DC group order"));
            }
            let x = rect.x as usize * dch * dcs;
            prep.dc_session.push_group(&dc[x..], dc_stride)?;
            dc_records.push(take_stripe_record(&mut prep.dc_session)?);
        }
        for sc in stripes {
            let si = sc.by0 / p.sb as usize;
            if split && si.is_multiple_of(k) {
                dc_records.clear();
                for gi in 0..p.ng {
                    let mut rec = Vec::new();
                    io.load(Spooled::Dc, si / k * p.ng + gi, &mut rec).await?;
                    dc_records.push(rec);
                }
            }
            let mut alpha_records = Vec::new();
            if let Some((session, _)) = &mut prep.alpha {
                let rows = p.alpha_plane(&band, y0, si, &mut plane);
                for _ in 0..p.ng {
                    let rect = session
                        .next_group()
                        .ok_or(Error::Corrupt("bounded alpha group order"))?;
                    if rect.y as usize != si * p.sb as usize * 8 || rect.height as usize != rows {
                        return Err(Error::Corrupt("bounded alpha group order"));
                    }
                    session.push_group(&plane[rect.x as usize * p.bps..], p.g.w * p.bps)?;
                    alpha_records.push(take_stripe_record(session)?);
                }
            }
            let mut units = p.units(sc);
            p.rewrite(
                cfg,
                &prep.passes,
                &mut units,
                &band,
                y0,
                &band_open_lambda,
                &band_closed_lambda,
                by0,
                None,
                None,
                io.stop(),
            )?;
            for (gi, u) in units.into_iter().enumerate() {
                if si.is_multiple_of(k) {
                    if cfg.model_references {
                        let mut record = vec![0, 0];
                        record.extend_from_slice(&dc_records[gi]);
                        io.emit(R_DC, &record).await?;
                    } else {
                        io.emit(R_DC, &dc_records[gi]).await?;
                    }
                }
                if let Some(a) = alpha_records.get(gi) {
                    io.emit(crate::parse::records::R_ALPHA, a).await?;
                }
                let mut out = Vec::new();
                // With any merge or split, every unit's record leads with its
                // map, as `encode_lossy_frame` writes them.
                let maps = prep.merges.as_ref().map(|m| m.1).unwrap_or_default();
                if cfg.coding_defaults {
                    let settings = coding_defaults::encode_inline(&u)?;
                    put_varint(&mut out, settings.len() as u64);
                    out.extend_from_slice(&settings);
                } else if maps.merges_only() {
                    let map = superblock::map::encode(&u.modes, u.nby, u.bw);
                    put_varint(&mut out, map.len() as u64);
                    out.extend_from_slice(&map);
                } else if maps.joint() {
                    // Exact parents (wire code 1) carry no flags.
                    let bypass = |b: usize| codes && u.aq.get(b).copied() == Some(1);
                    let n = u.nby * u.bw;
                    let modes = if u.modes.is_empty() {
                        vec![0u8; n]
                    } else {
                        u.modes.clone()
                    };
                    // Flags only where the final coefficients have AC.
                    let ac = superblock::ac_bits(&u.q, n);
                    let splits: Vec<u8> = (0..n)
                        .map(|b| u.splits.get(b).copied().unwrap_or(0) & ac[b])
                        .collect();
                    let map =
                        superblock::map::encode_joint(&modes, &splits, &ac, (u.nby, u.bw), &bypass);
                    put_varint(&mut out, map.len() as u64);
                    out.extend_from_slice(&map);
                } else if p.adaptive_mask != 0 {
                    // Adaptive layouts: the unit's transform choices.
                    layout::write_modes(&mut out, &u.modes);
                }
                match (&prep.ac, &encoder_rl) {
                    (Ac::Rl(_), Some(encoder)) => {
                        let mut lanes: [Vec<Sym>; NL] = core::array::from_fn(|_| Vec::new());
                        stripe_symbols(&u, u.bw, &mut lanes);
                        let map_prefix = out.len();
                        encode_lanes_tables(encoder, &lanes, u.bw.min(NL), &mut out);
                        if cfg.model_references {
                            let body = model_refs::encode_causal_ans(
                                &lanes,
                                u.bw.min(NL),
                                cfg,
                                crate::effort::Effort::new(cfg.effort)?,
                                &out[map_prefix..],
                            )?;
                            out.truncate(map_prefix);
                            out.extend_from_slice(&body);
                        } else if cfg.local_ans_contexts {
                            let body = local_ans::encode(
                                &lanes,
                                u.bw.min(NL),
                                cfg,
                                crate::effort::Effort::new(cfg.effort)?,
                                &out[map_prefix..],
                            )?;
                            out.truncate(map_prefix);
                            out.extend_from_slice(&body);
                        }
                    }
                    (Ac::Huf(t), _) => {
                        if cfg.model_references {
                            put_varint(&mut out, 0);
                        }
                        let encoder = encoder_huf.as_ref().unwrap();
                        let (counts, widths) = stripe_counts_widths(&u, u.bw);
                        let symbols = huf::stripe_syms(&counts, &widths, u.nby * u.bw);
                        let (magnitudes, signs) = stripe_raw_split(&u, u.bw, &counts, &widths);
                        for (symbols, map) in [
                            (&symbols.luma, &t.luma_map[..]),
                            (&symbols.chroma, &t.chroma_map[..]),
                            (&symbols.wid, &t.wid_map[..]),
                        ] {
                            let mut section = Vec::new();
                            encoder.encode_section(symbols, map, &mut section);
                            put_varint(&mut out, section.len() as u64);
                            out.extend_from_slice(&section);
                        }
                        put_varint(&mut out, magnitudes.len() as u64);
                        out.extend_from_slice(&magnitudes);
                        out.extend_from_slice(&signs);
                    }
                    _ => unreachable!(),
                }
                io.emit_ac(&out, &u).await?;
                if let Some(e) = &prep.exact {
                    let n = e.counts[unit];
                    let mut rec = Vec::new();
                    while next_t < e.stripes && next_t * e.per < list + n {
                        io.load(Spooled::Exact, next_t, &mut stripe_bytes).await?;
                        rec.extend_from_slice(&stripe_bytes);
                        next_t += 1;
                    }
                    list += n;
                    io.emit(R_EXACT, &rec).await?;
                }
                unit += 1;
            }
        }
    }
    if !prep.dc_session.is_finished() || prep.alpha.as_ref().is_some_and(|(s, _)| !s.is_finished())
    {
        return Err(Error::Corrupt("bounded nested image incomplete"));
    }
    Ok(())
}

/// The `DATA` payload head (everything before the first main record), with
/// `records` as its head record list, and where that list starts in it.
pub(super) fn data_head(p: &Plan, prep: &Prepared, records: &[u8]) -> (Vec<u8>, usize) {
    let mut data = Vec::new();
    data.push(p.sb as u8);
    put_varint(&mut data, p.gcols as u64);
    envelope::write(
        &mut data,
        p.color,
        p.kind,
        &envelope::Params {
            scale: p.scale,
            qp: p.qp,
            entropy: prep.entropy,
            compact: false,
            seed: crate::seeds::Selection::NONE,
            records: !records.is_empty(),
        },
    );
    put_varint(&mut data, prep.tb.len() as u64);
    data.extend_from_slice(&prep.tb);
    let mut at = data.len();
    if !records.is_empty() {
        put_varint(&mut data, records.len() as u64);
        at = data.len();
        data.extend_from_slice(records);
    }
    put_varint(&mut data, prep.dc_head.len() as u64);
    data.extend_from_slice(&prep.dc_head);
    if let Some((_, head)) = &prep.alpha {
        put_varint(&mut data, head.len() as u64);
        data.extend_from_slice(head);
    }
    if let Some(e) = &prep.exact {
        put_varint(&mut data, e.head.len() as u64);
        data.extend_from_slice(&e.head);
    }
    (data, at)
}

/// The file header for this payload (everything before `DATA`).
pub(super) fn file_header(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    p: &Plan,
    prep: &Prepared,
) -> Result<Vec<u8>, Error> {
    use crate::parse::header::Feature;
    let mut features = crate::v1::head_features(&prep.dc_info, &prep.dc_head);
    if let (Some(ai), Some((_, head))) = (&p.alpha, &prep.alpha) {
        features = features.union(crate::v1::head_features(ai, head));
    }
    if !prep.fits16 {
        features = features.with(Feature::Samples32);
    }
    if cfg.local_ans_contexts {
        features = features.with(Feature::LocalAnsContexts);
    }
    if cfg.coding_defaults {
        features = features.with(Feature::CodingDefaults);
    }
    if cfg.model_references {
        features = features.with(Feature::ModelReferences);
    }
    if prep.exact.is_some() {
        features = features.with(Feature::ExactBlocks);
    }
    let maps = prep.merges.as_ref().map(|m| m.1).unwrap_or_default();
    if maps.merge {
        features = features.with(Feature::Superblocks);
    }
    if maps.joint() {
        features = features.with(Feature::SmallTransforms);
    }
    if p.layouts != [LossyChannelLayout::default(); 3] {
        features = features.with(Feature::ChannelLayouts);
    }
    if (3..=MATRIX_LAST_PRESET).contains(&p.qp.matrix) {
        features = features.with(Feature::DequantPresets);
    }
    let mut c = cfg.clone();
    c.color = p.color;
    let mut out = Vec::new();
    crate::container::write_head_features(
        &mut out,
        info,
        meta,
        crate::Coding::Lossy,
        Some(p.color),
        c.decode_speed(),
        features,
    )?;
    Ok(out)
}

#[cfg(all(test, feature = "encode"))]
pub(crate) fn encode_lossy_bounded(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    rows: &mut dyn FnMut(u8, usize, &mut [u8]) -> Result<(), Error>,
) -> Result<Vec<u8>, Error> {
    use crate::parse::records::{framed_len, put_record};
    let p = plan(info, cfg)?;
    let mut fetch = fetcher(&p, rows);
    let mut exact_stripes: Vec<Vec<u8>> = Vec::new();
    let mut dc_records: Vec<Vec<u8>> = Vec::new();
    let mut prep = crate::parse::payload::ready(pass1(
        &p,
        cfg,
        &mut Callbacks {
            rows: &mut fetch,
            store: |kind: Spooled, rec: &[u8]| {
                match kind {
                    Spooled::Exact => exact_stripes.push(rec.to_vec()),
                    Spooled::Dc => dc_records.push(rec.to_vec()),
                }
                Ok(())
            },
            load: no_load,
            emit: no_emit,
        },
    ))?;
    let mut body = Vec::new();
    let mut spans = Vec::new();
    crate::parse::payload::ready(pass2(
        &p,
        cfg,
        &mut prep,
        &mut Callbacks {
            rows: &mut fetch,
            store: no_store,
            load: |kind: Spooled, t: usize, out: &mut Vec<u8>| {
                out.clear();
                out.extend_from_slice(match kind {
                    Spooled::Exact => &exact_stripes[t][..],
                    Spooled::Dc => &dc_records[t][..],
                });
                Ok(())
            },
            emit: |id: u64, payload: &[u8]| {
                spans.push(framed_len(id, payload.len()));
                put_record(&mut body, id, payload);
                Ok(())
            },
        },
    ))?;
    let nunits = p.g.stripes() * p.ng;
    // Head records in increasing id order (SPEC 2.8), as `finish_lossy_frame`
    // writes them for this configuration.
    let mut records = Vec::new();
    if prep.exact.is_some() {
        put_record(&mut records, H_EXACT, &[]);
    }
    let hint_form = matches!(crate::v1::unit_table_form(), crate::v1::UnitTableForm::Hint);
    if hint_form {
        crate::v1::put_unit_table(&mut records, nunits, &spans);
    }
    if !prep.fits16 {
        put_record(&mut records, H_DC32, &[]);
    }
    if !hint_form {
        crate::v1::put_unit_table(&mut records, nunits, &spans);
    }
    match if cfg.coding_defaults {
        None
    } else {
        prep.merges.as_ref().map(|m| m.1)
    } {
        Some(m) if m.joint() => put_record(&mut records, small::H_PARTITIONS, &[]),
        Some(m) if m.merges_only() => put_record(&mut records, superblock::H_SUPERBLOCKS, &[]),
        _ => {}
    }
    if p.layouts != [LossyChannelLayout::default(); 3] {
        put_record(&mut records, H_LAYOUTS, &p.layouts.map(|l| l.byte()));
    }
    if cfg.local_ans_contexts {
        put_record(&mut records, local_ans::HEAD, &[]);
    }
    if cfg.coding_defaults {
        put_record(&mut records, coding_defaults::HEAD, &[1, 0, 0]);
    }
    if cfg.model_references {
        put_record(&mut records, model_refs::HEAD, &[1, 0]);
    }
    let (mut data, _) = data_head(&p, &prep, &records);
    data.extend_from_slice(&body);
    let mut out = file_header(info, meta, cfg, &p, &prep)?;
    crate::container::put_data(&mut out, &data);
    let out = crate::container::finish_unit_table(out)?;
    let out = crate::container::locate(out)?;
    if cfg.checksum {
        crate::checksum::finish_file(out, Some(cfg.hash))
    } else {
        Ok(out)
    }
}
