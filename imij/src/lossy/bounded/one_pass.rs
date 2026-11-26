//! Private lab experiment: fixed model support and DC representation, followed
//! by the existing band pipeline and record writer with a single source pass.
//! Lab-only constructor; ordinary encoder defaults are unchanged.
#![cfg_attr(not(test), allow(dead_code))]
use super::*;
pub(crate) mod rate;

pub(super) fn configure(mut p: Plan, cfg: &LossyConfig) -> Result<Plan, Error> {
    if cfg.effort != 0.0
        || p.trellis.is_some()
        || p.exact.is_some()
        || p.merges.is_some()
        || p.layouts != [LossyChannelLayout::default(); 3]
        || cfg.unit_ordinals
        || cfg.color_transform.is_some()
    {
        return Err(Error::Unsupported(
            "one-pass experiment requires effort0, fixed 8x8 layouts, no exact blocks/merges/ordinals/explicit color transform",
        ));
    }
    // One native AC stripe per source band; its DC group can be published with
    // it instead of gathering DC over later source bands.
    p.work = p.sb as usize;
    p.dc_rows = p.work;
    Ok(p)
}

/// Blend raw band-0 observations with a calibrated normalized prior so flat or
/// dark first bands retain realistic decay across unseen symbols instead of
/// falling back to a flat uniform tail. Every legal symbol keeps weight >= 1.
fn blend_prior(obs: &[u64], prior: &[f64], legal: impl Fn(usize) -> bool, out: &mut [u64]) {
    const OBS_SCALE: u64 = 64;
    let total_obs: u64 = obs.iter().sum();
    // Keep ~8% mixture mass on the calibrated prior (and at least 64 virtual
    // samples when a context had few or zero observations in band 0).
    let prior_mass = (total_obs as f64 * 0.08).max(64.0) * OBS_SCALE as f64;
    for i in 0..out.len() {
        if legal(i) {
            let p = ((prior[i] * prior_mass) as u64).max(1);
            out[i] = obs[i].saturating_mul(OBS_SCALE).saturating_add(p);
        } else {
            out[i] = 0;
        }
    }
}

fn normalize(v: &mut [f64]) {
    let sum: f64 = v.iter().sum();
    if sum > 0.0 {
        for x in v.iter_mut() {
            *x /= sum;
        }
    }
}

fn rl_eob_prior() -> [f64; coef::ALPHA] {
    let mut p = [0.0f64; coef::ALPHA];
    p[0] = 6.0;
    for e in 1..64usize {
        p[e] = libm::pow(1.0 + e as f64 / 5.0, -1.35);
    }
    normalize(&mut p[..64]);
    p
}

fn rl_ac_prior() -> [f64; coef::ALPHA] {
    let mut p = [0.0f64; coef::ALPHA];
    for run in 0..=coef::MAX_RUN {
        let r_decay = libm::pow(1.0 + run as f64, -1.55);
        for class in 0..coef::MAG_CLASSES {
            let c_decay = libm::pow(1.0 + class as f64, -2.3);
            p[(run * 32 + class) as usize] = r_decay * c_decay;
        }
    }
    p[coef::ZRL as usize] = 0.03;
    normalize(&mut p);
    p
}

fn huf_count_prior() -> [f64; huf::CNT_ALPHA] {
    let mut p = [0.0f64; huf::CNT_ALPHA];
    p[0] = 5.0;
    for c in 1..huf::CNT_ALPHA {
        p[c] = libm::pow(1.0 + c as f64 / 2.0, -1.8);
    }
    normalize(&mut p);
    p
}

fn huf_wid_prior() -> [f64; huf::WID_ALPHA] {
    let mut p = [0.0f64; huf::WID_ALPHA];
    p[0] = 8.0;
    for w in 1..huf::WID_ALPHA {
        p[w] = libm::pow(1.0 + w as f64, -2.3);
    }
    normalize(&mut p);
    p
}

/// Calibrated Lomax + zero-run DC prior for `TokenCfg::v1(4, 1, 32)` by raw
/// context class (0..16), keeping full support below the run cap.
fn dc_class_prior(tc: &crate::tokens::TokenCfg, cls: usize) -> Vec<f64> {
    let asz = tc.alphabet() as usize;
    let vt = tc.value_tokens() as usize;
    let legal_end = crate::rans::run_cap_end(tc).min(asz);
    let (scale, power, zero_mass, run_mass) = match cls {
        0 => (2.0, 1.4, 0.36, 0.34),
        1..=3 => (3.5, 1.6, 0.16, 0.04),
        4..=6 => (16.0, 1.5, 0.05, 0.015),
        _ => (256.0, 1.0, 0.015, 0.01),
    };
    let mut p = vec![0.0f64; asz];
    p[0] = zero_mass;
    let val_mass = 1.0 - zero_mass - run_mass;
    let mut val_sum = 0.0f64;
    for (tok, slot) in p.iter_mut().enumerate().take(vt).skip(1) {
        let (base, nb) = crate::tokens::hybrid_base(tok as u32, tc.k, tc.m);
        let lo = base as f64;
        let hi1 = lo + (1u64 << nb) as f64;
        *slot =
            (libm::pow(1.0 + lo / scale, -power) - libm::pow(1.0 + hi1 / scale, -power)).max(0.0);
        val_sum += *slot;
    }
    if val_sum > 0.0 {
        let k = val_mass / val_sum;
        for slot in &mut p[1..vt] {
            *slot *= k;
        }
    }
    let mut run_sum = 0.0f64;
    for (tok, slot) in p.iter_mut().enumerate().take(legal_end).skip(vt) {
        let (base, _) = crate::tokens::hybrid_base((tok - vt) as u32, tc.rk, tc.rm);
        if base + 1 >= 4 {
            let m = libm::pow((base + 1) as f64, -0.45);
            *slot = m;
            run_sum += m;
        }
    }
    if run_sum > 0.0 {
        let k = run_mass / run_sum;
        for slot in &mut p[vt..legal_end] {
            *slot *= k;
        }
    }
    p
}

/// Conservative admission using the existing band's working-storage account,
/// plus metadata copies. Caller-owned scratch/decoder/transport queues are separate.
pub(crate) fn admit(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    max_index_bytes: u64,
    max_working_bytes: u64,
) -> Result<(), Error> {
    // Check row and padded geometry before plan() uses usize pixel products.
    let row = u128::from(info.width) * info.bytes_per_pixel() as u128;
    let padded_width = u128::from(info.width).div_ceil(8) * 8;
    if row > usize::MAX as u128 || padded_width > usize::MAX as u128 {
        return Err(Error::TooLarge);
    }
    let p = configure(prepare_plan(info, cfg)?.plan, cfg)?;
    // Count in a widened domain before the shared writer uses usize counts.
    let units = p.g.stripes() as u128 * p.ng as u128;
    let records = units * if p.alpha.is_some() { 2 } else { 1 }
        + p.g.bh.div_ceil(p.dc_rows) as u128 * p.ng as u128;
    if records > usize::MAX as u128 || records * 10 > u128::from(max_index_bytes) {
        return Err(Error::TooLarge);
    }
    let mut metadata_bytes = 0u128;
    for bytes in [&meta.icc, &meta.exif, &meta.xmp].into_iter().flatten() {
        metadata_bytes += bytes.len() as u128;
    }
    for chunk in &meta.chunks {
        metadata_bytes += chunk.data.len() as u128 + core::mem::size_of::<crate::Chunk>() as u128;
    }
    if let Some(gain) = &meta.gain_map {
        metadata_bytes += gain.image.len() as u128;
        if let Some(icc) = &gain.alt_icc {
            metadata_bytes += icc.len() as u128;
        }
    }
    let total = u128::from(plan_working_bytes(&p)?) + 4 * metadata_bytes + 4096;
    if total > u128::from(max_working_bytes) || total > usize::MAX as u128 {
        return Err(Error::TooLarge);
    }
    Ok(())
}

pub(super) async fn prepare<I: PassIo>(
    p: &Plan,
    cfg: &LossyConfig,
    io: &mut I,
    aq: bool,
) -> Result<Prepared, Error> {
    // Train only on the first admitted source band. Every legal future symbol
    // retains positive support; this band is retained for coding, never read
    // from the source again. No later source band is needed before publication.
    let (by0, by1) = p
        .bands()
        .next()
        .ok_or(Error::InvalidInput("empty one-pass image"))?;
    let mut first_band = Vec::new();
    let y0 = io.rows(2, by0, by1, &mut first_band).await?;
    let first = p.band_stripes(&first_band, y0, by0, by1);
    let effort = crate::effort::Effort::new(cfg.effort)?;
    let entropy = if matches!(
        cfg.decode_speed,
        Some(crate::DecodeSpeed::Fast | crate::DecodeSpeed::Fastest)
    ) {
        LossyEntropy::SplitHuf
    } else {
        cfg.entropy
    };
    let ac = match entropy {
        LossyEntropy::RunLevel => {
            let mut obs = vec![vec![0u64; coef::ALPHA]; coef::NRAW];
            for stripe in &first {
                for unit in p.units(stripe.clone()) {
                    stripe_tokens(&unit, unit.bw, Group::ALL, |_, ctx, symbol, _, _| {
                        obs[ctx][symbol as usize] += 1;
                    });
                }
            }
            let eob_prior = rl_eob_prior();
            let ac_prior = rl_ac_prior();
            let mut hist = vec![vec![0u64; coef::ALPHA]; coef::NRAW];
            for ctx in 0..coef::NRAW {
                if ctx < coef::EOB_CTX {
                    blend_prior(&obs[ctx], &eob_prior, |s| s < 64, &mut hist[ctx]);
                } else {
                    blend_prior(
                        &obs[ctx],
                        &ac_prior,
                        |s| {
                            s == coef::ZRL as usize
                                || ((s / 32) <= coef::MAX_RUN as usize
                                    && (s % 32) < coef::MAG_CLASSES as usize)
                        },
                        &mut hist[ctx],
                    );
                }
            }
            Ac::Rl(
                Tables::build_seeded(
                    &hist,
                    4,
                    cfg.max_ac_tables,
                    entropy,
                    effort,
                    None,
                    table_k_mode(cfg),
                )
                .0,
            )
        }
        LossyEntropy::SplitHuf => {
            let mut luma_obs = vec![vec![0u64; huf::CNT_ALPHA]; huf::LUMA_CTX];
            let mut chroma_obs = vec![vec![0u64; huf::CNT_ALPHA]; huf::CHROMA_CTX];
            let mut wid_obs = vec![vec![0u64; huf::WID_ALPHA]; huf::WID_CTX];
            for stripe in &first {
                for unit in p.units(stripe.clone()) {
                    let (counts, widths) = stripe_counts_widths(&unit, unit.bw);
                    let symbols = huf::stripe_syms(&counts, &widths, unit.nby * unit.bw);
                    for (hist, symbols) in [
                        (&mut luma_obs, &symbols.luma),
                        (&mut chroma_obs, &symbols.chroma),
                        (&mut wid_obs, &symbols.wid),
                    ] {
                        for &(ctx, symbol) in symbols {
                            hist[ctx as usize][symbol as usize] += 1;
                        }
                    }
                }
            }
            let cnt_prior = huf_count_prior();
            let wid_prior = huf_wid_prior();
            let mut luma = vec![vec![0u64; huf::CNT_ALPHA]; huf::LUMA_CTX];
            let mut chroma = vec![vec![0u64; huf::CNT_ALPHA]; huf::CHROMA_CTX];
            let mut wid = vec![vec![0u64; huf::WID_ALPHA]; huf::WID_CTX];
            for (o, h) in luma_obs.iter().zip(&mut luma) {
                blend_prior(o, &cnt_prior, |_| true, h);
            }
            for (o, h) in chroma_obs.iter().zip(&mut chroma) {
                blend_prior(o, &cnt_prior, |_| true, h);
            }
            for (o, h) in wid_obs.iter().zip(&mut wid) {
                blend_prior(o, &wid_prior, |_| true, h);
            }
            Ac::Huf(huf::HufTables::build(&luma, &chroma, &wid, effort, None, table_k_mode(cfg)).0)
        }
    };
    let mut bits = BitWriter::new();
    match &ac {
        Ac::Rl(t) => t.write(&mut bits),
        Ac::Huf(t) => t.write(&mut bits),
    }
    // Signed32 DC is an existing native representation. Choosing it up front
    // removes the image-wide "fits16" discovery pass; it adds no precision loss.
    let (dc_info, dc_cfg) = p.dc_layout(false, aq);
    let (dc_session, dc_head) = GroupEncodeSession::nested(dc_info.clone(), dc_cfg)?;
    let alpha = p
        .alpha
        .as_ref()
        .map(|info| GroupEncodeSession::nested(info.clone(), p.alpha_cfg()))
        .transpose()?;
    let mut prepared = Prepared {
        first_band: Some(first_band),
        entropy,
        ac,
        tb: bits.finish(),
        fits16: false,
        dc_info,
        dc_session,
        dc_head,
        alpha,
        passes: Vec::new(),
        exact: None,
        merges: None,
        open_lambda: None,
        closed_lambda: None,
        dc_rdo: false,
    };
    if !aq {
        train_dc_stripes(p, &mut prepared, &first, by0, by1)?;
    }
    Ok(prepared)
}

fn train_dc_stripes(
    p: &Plan,
    prepared: &mut Prepared,
    stripes: &[StripeCoefs],
    by0: usize,
    by1: usize,
) -> Result<(), Error> {
    assert!(!prepared.fits16);
    let mut dc = Vec::new();
    dc_band(p, stripes, by0, by1, false, false, &mut dc);
    let mut histograms = prepared.dc_session.empty_histograms();
    let columns = if p.gcols == 0 { p.g.bw } else { p.gcols / 8 };
    for x in (0..p.g.bw).step_by(columns) {
        prepared.dc_session.group_histograms(
            columns.min(p.g.bw - x),
            by1 - by0,
            &dc[x * NCH * 4..],
            p.g.bw * NCH * 4,
            &mut histograms,
        );
    }
    let tc = crate::tokens::TokenCfg::v1(4, 1, 32);
    let legal_end = crate::rans::run_cap_end(&tc);
    let priors: Vec<Vec<f64>> = (0..crate::coding_layout::CTX_CLASSES)
        .map(|cls| dc_class_prior(&tc, cls))
        .collect();
    for slot in &mut histograms {
        for (cls, context) in slot.iter_mut().enumerate() {
            let obs = context.clone();
            blend_prior(&obs, &priors[cls], |s| s < legal_end, context);
        }
    }
    let (info, cfg) = p.dc_layout(false, false);
    (prepared.dc_session, prepared.dc_head) =
        GroupEncodeSession::nested_measured(info, cfg, &histograms)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn encode<X: StreamIo>(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    x.stop().check().map_err(|_| Error::Cancelled)?;
    encode_lossy_stream_io(
        info,
        meta,
        cfg,
        x,
        StreamIndex::Trailing,
        index_start,
        checksum_start,
        max_index_bytes,
        Preparation::OnePass,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{DecodeConfig, FeedDecoder, FeedStatus};
    use crate::stream_encode::StreamEncodeError;
    use zencodec::decode::DecoderConfig as _;

    struct Live {
        info: ImageInfo,
        source: Vec<u8>,
        stride: usize,
        calls: Vec<(usize, usize)>,
        output: Vec<u8>,
        index: Vec<u8>,
        checksum: Vec<u8>,
        decoder: FeedDecoder,
        decoded: Vec<u8>,
        rows: usize,
        require_each_group: bool,
    }
    impl Live {
        fn feed(&mut self, bytes: &[u8]) {
            let mut at = 0;
            loop {
                match self.decoder.advance().unwrap() {
                    FeedStatus::NeedInput if at == bytes.len() => break,
                    FeedStatus::NeedInput => {
                        let n = self.decoder.push(&bytes[at..]).unwrap();
                        assert!(n > 0);
                        at += n;
                    }
                    FeedStatus::Row { y } => {
                        assert_eq!(y as usize, self.rows);
                        self.rows += 1;
                        self.decoded
                            .extend_from_slice(self.decoder.row().unwrap().unwrap().row(0));
                    }
                    FeedStatus::Complete => {
                        assert_eq!(at, bytes.len());
                        break;
                    }
                }
            }
        }
    }
    impl StreamIo for Live {
        async fn source(
            &mut self,
            pass: u8,
            y0: usize,
            out: &mut Vec<u8>,
        ) -> Result<(), StreamEncodeError> {
            assert_eq!(pass, 2, "a statistics/source replay pass is forbidden");
            let expected = self.calls.last().map_or(0, |(y, n)| y + n);
            assert_eq!(
                y0, expected,
                "each source row must be consumed exactly once"
            );
            if y0 > 0 {
                if self.require_each_group {
                    assert_eq!(
                        self.rows, y0,
                        "each local-context band must decode before the next input band"
                    );
                }
                assert!(
                    self.rows > 0,
                    "no later source band before the first decoded row"
                );
            }
            let rb = self.info.row_bytes();
            let rows = out.len() / rb;
            for y in 0..rows {
                out[y * rb..(y + 1) * rb]
                    .copy_from_slice(&self.source[(y0 + y) * self.stride..][..rb]);
            }
            self.calls.push((y0, rows));
            Ok(())
        }
        async fn write(
            &mut self,
            stream: Stream,
            at: u64,
            bytes: &[u8],
        ) -> Result<(), StreamEncodeError> {
            match stream {
                Stream::Output => {
                    assert_eq!(at, self.output.len() as u64, "forward-only publication");
                    self.output.extend_from_slice(bytes);
                    self.feed(bytes);
                }
                Stream::Index => {
                    assert_eq!(at, self.index.len() as u64);
                    self.index.extend_from_slice(bytes);
                }
                Stream::Checksum => {
                    assert_eq!(at, self.checksum.len() as u64);
                    self.checksum.extend_from_slice(bytes);
                }
            };
            Ok(())
        }
        async fn read(
            &mut self,
            stream: Stream,
            at: u64,
            buf: &mut [u8],
        ) -> Result<(), StreamEncodeError> {
            let data = match stream {
                Stream::Index => &self.index,
                Stream::Checksum => &self.checksum,
                Stream::Output => panic!("published output must not be read back"),
            };
            buf.copy_from_slice(&data[at as usize..][..buf.len()]);
            Ok(())
        }
    }

    fn witness(info: &ImageInfo, cfg: &LossyConfig, flat_first: bool) -> Vec<u8> {
        let rb = info.row_bytes();
        let stride = rb + 13;
        let bps = info.sample.bytes();
        let bits = info.significant_bits[0];
        let mut pixels = vec![0xa5; stride * (info.height as usize - 1) + rb];
        let mut tight = Vec::new();
        for y in 0..info.height as usize {
            for x in 0..info.width as usize * info.channels.count() {
                let pixel_x = x / info.channels.count();
                let mut value = if flat_first && y < 8 {
                    0
                } else if (pixel_x / 8 + y / 8) % 13 == 3
                    || (pixel_x.is_multiple_of(17) && y % 11 == 3)
                {
                    ((1usize << bits) - 1) as u16
                } else {
                    ((x * 37 + y * 19 + (x ^ y) * 11) & ((1usize << bits) - 1)) as u16
                };
                if cfg.local_ans_contexts && y >= cfg.stripe_block_rows as usize * 8 {
                    value = (((1usize << bits) - 1) / 4) as u16 + value / 16;
                }
                let at = y * stride + x * bps;
                if bps == 1 {
                    pixels[at] = value as u8;
                } else {
                    pixels[at..at + 2].copy_from_slice(&value.to_ne_bytes());
                }
            }
            tight.extend_from_slice(&pixels[y * stride..][..rb]);
        }
        let original = pixels.clone();
        let mut live = Live {
            info: info.clone(),
            source: pixels,
            stride,
            calls: Vec::new(),
            output: Vec::new(),
            index: Vec::new(),
            checksum: Vec::new(),
            decoder: DecodeConfig::default().job().feed_decoder(8192).unwrap(),
            decoded: Vec::new(),
            rows: 0,
            require_each_group: cfg.local_ans_contexts,
        };
        let n = crate::parse::payload::ready(encode(
            info,
            &Metadata::default(),
            cfg,
            &mut live,
            0,
            cfg.checksum.then_some(0),
            1 << 20,
        ))
        .unwrap();
        assert_eq!(n, live.output.len() as u64);
        assert_eq!(live.source, original, "source and padding are immutable");
        assert_eq!(
            live.calls.iter().map(|x| x.1).sum::<usize>(),
            info.height as usize
        );
        if info.height > cfg.stripe_block_rows * 8 {
            assert!(live.calls.len() > 1);
        }
        live.decoder.finish_input().unwrap();
        live.feed(&[]);
        assert_eq!(live.rows, info.height as usize);
        let reference = encode_lossy_bounded(info, &Metadata::default(), cfg, &mut |_, y, out| {
            out.copy_from_slice(&tight[y * rb..][..out.len()]);
            Ok(())
        })
        .unwrap();
        let expected = crate::decode(&reference).unwrap();
        let whole = encode_lossy(&original, stride, info, &Metadata::default(), cfg).unwrap();
        assert_eq!(
            crate::decode(&whole).unwrap(),
            expected,
            "ordinary encoder oracle"
        );
        assert_eq!(live.decoded, expected.1, "incremental decoder");
        assert_eq!(
            crate::decode(&live.output).unwrap(),
            expected,
            "full decoder"
        );
        for threads in [1, 2, 4, 8] {
            let mut out = vec![0x6d; stride * (info.height as usize - 1) + rb];
            let decoded_info =
                crate::decode_into_threads(&live.output, &mut out, stride, threads).unwrap();
            assert_eq!(decoded_info, expected.0);
            for y in 0..info.height as usize {
                assert_eq!(&out[y * stride..][..rb], &expected.1[y * rb..][..rb]);
                if y + 1 < info.height as usize {
                    assert!(
                        out[y * stride + rb..(y + 1) * stride]
                            .iter()
                            .all(|&b| b == 0x6d)
                    );
                }
            }
        }
        if info.channels.has_alpha() {
            let pixel_bytes = info.channels.count() * bps;
            for (src, dst) in tight
                .chunks_exact(pixel_bytes)
                .zip(expected.1.chunks_exact(pixel_bytes))
            {
                assert_eq!(
                    &src[pixel_bytes - bps..],
                    &dst[pixel_bytes - bps..],
                    "exact alpha"
                );
            }
        }
        live.output
    }

    fn info(w: u32, h: u32, channels: Channels, bits: u8) -> ImageInfo {
        ImageInfo::new(
            w,
            h,
            channels,
            if bits == 8 {
                SampleType::U8
            } else {
                SampleType::U16
            },
        )
        .with_significant_bits(&vec![bits; channels.count()])
        .unwrap()
    }

    #[test]
    fn scoped_defaults_and_causal_model_grammar_keep_one_source_pass() {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let mut cfg = LossyConfig::new(40.0).with_effort(0.0);
            cfg.entropy = entropy;
            cfg.stripe_block_rows = 1;
            cfg.coding_defaults = true;
            cfg.model_references = true;
            witness(&info(65, 65, Channels::Rgba, 12), &cfg, false);
        }
    }

    #[test]
    fn local_ans_rows_publish_before_the_next_band_with_exact_pixels() {
        for bits in [8, 10, 12, 16] {
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                for rows in [1, 8] {
                    let mut cfg = LossyConfig::new(40.0).with_effort(0.0);
                    cfg.local_ans_contexts = true;
                    cfg.color = color;
                    cfg.stripe_block_rows = rows;
                    cfg.column_groups = Some(512);
                    cfg.checksum = true;
                    let encoded = witness(&info(1025, 137, Channels::Rgba, bits), &cfg, true);
                    assert!(parse_lossy(&encoded).unwrap().local_ans_contexts);
                }
            }
        }
    }

    #[test]
    fn native_rows_arrive_before_later_source_bands_and_match_two_pass_pixels() {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for bits in [8, 10, 12] {
                let mut cfg = LossyConfig::new(80.0).with_effort(0.0);
                cfg.entropy = entropy;
                cfg.stripe_block_rows = 1;
                witness(&info(65, 129, Channels::Rgb, bits), &cfg, false);
            }
        }
    }

    #[test]
    fn native_profile_preserves_whole_image_pixels_alpha_padding_and_thread_parity() {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for bits in [8, 10, 12, 16] {
                    for channels in [
                        Channels::Gray,
                        Channels::GrayAlpha,
                        Channels::Rgb,
                        Channels::Rgba,
                    ] {
                        for (w, h, quality, groups, stripe, checksum, flat) in [
                            (1, 1, 5.0, None, 1, false, false),
                            (17, 25, 30.0, Some(8), 1, true, true),
                            (65, 129, 60.0, Some(32), 2, false, true),
                            (67, 33, 95.0, None, 1, true, false),
                        ] {
                            let mut cfg = LossyConfig::new(quality).with_effort(0.0);
                            cfg.color = color;
                            cfg.entropy = entropy;
                            cfg.stripe_block_rows = stripe;
                            cfg.column_groups = groups;
                            cfg.checksum = checksum;
                            witness(&info(w, h, channels, bits), &cfg, flat);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn wide_native_worker_budgets_preserve_serial_wire_and_ordinary_pixels() {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for color in [LossyColor::Xyb, LossyColor::YCoCg] {
                for bits in [8, 10, 12, 16] {
                    for (w, h, channels) in [
                        (257, 9, Channels::Rgb),
                        (513, 73, Channels::Rgba),
                        (2049, 9, Channels::Rgb),
                        (3841, 65, Channels::Rgba),
                        (8201, 73, Channels::Rgb),
                    ] {
                        let info = info(w, h, channels, bits);
                        for stripe in [1, 2, 4, 8] {
                            let mut cfg = LossyConfig::new(80.0).with_effort(0.0);
                            cfg.entropy = entropy;
                            cfg.color = color;
                            cfg.stripe_block_rows = stripe;
                            let serial = witness(&info, &cfg, true);
                            for workers in [2, 4, 8] {
                                cfg.threads = workers;
                                assert_eq!(
                                    witness(&info, &cfg, true),
                                    serial,
                                    "{w}x{h} {bits}bit {channels:?} {color:?} {entropy:?} {workers} workers"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[derive(Default)]
    struct Faults {
        reads: usize,
        writes: usize,
        source_error: bool,
        output_error: bool,
        cancel: bool,
    }
    impl how_far::Stop for Faults {
        fn check(&self) -> Result<(), how_far::StopReason> {
            if self.cancel {
                Err(how_far::StopReason::Cancelled)
            } else {
                Ok(())
            }
        }
    }
    impl StreamIo for Faults {
        fn stop(&self) -> &dyn how_far::Stop {
            self
        }
        async fn source(
            &mut self,
            _: u8,
            _: usize,
            out: &mut Vec<u8>,
        ) -> Result<(), StreamEncodeError> {
            self.reads += 1;
            if self.source_error {
                return Err(StreamEncodeError::Read(std::io::Error::other(
                    "source witness",
                )));
            }
            out.fill(0);
            Ok(())
        }
        async fn write(&mut self, _: Stream, _: u64, _: &[u8]) -> Result<(), StreamEncodeError> {
            self.writes += 1;
            if self.output_error {
                Err(StreamEncodeError::Write(std::io::Error::other(
                    "destination witness",
                )))
            } else {
                Ok(())
            }
        }
        async fn read(&mut self, _: Stream, _: u64, _: &mut [u8]) -> Result<(), StreamEncodeError> {
            panic!("no replay after an early failure")
        }
    }

    #[test]
    fn native_profile_refuses_unsupported_work_before_io_and_preserves_failures() {
        let info = info(65, 33, Channels::Rgb, 10);
        let cfg = LossyConfig::new(80.0).with_effort(0.0);
        let run = |cfg: &LossyConfig, x: &mut Faults, limit| {
            crate::parse::payload::ready(encode(
                &info,
                &Metadata::default(),
                cfg,
                x,
                0,
                None,
                limit,
            ))
        };
        let huge = ImageInfo::new(u32::MAX, u32::MAX, Channels::Rgba, SampleType::U16);
        assert!(matches!(
            admit(&huge, &Metadata::default(), &cfg, 1 << 20, 16 << 20),
            Err(Error::TooLarge)
        ));
        let metadata = Metadata {
            xmp: Some(vec![0; 3 << 20]),
            ..Metadata::default()
        };
        assert!(matches!(
            admit(&info, &metadata, &cfg, 1 << 20, 16 << 20),
            Err(Error::TooLarge)
        ));
        let mut unsupported = cfg.clone();
        unsupported.effort = 7.5;
        let mut x = Faults::default();
        assert!(matches!(
            run(&unsupported, &mut x, 1 << 20),
            Err(StreamEncodeError::Codec(Error::Unsupported(_)))
        ));
        assert_eq!((x.reads, x.writes), (0, 0));
        let mut transformed = cfg.clone();
        transformed.color = LossyColor::YCoCg;
        transformed.color_transform = Some(ColorTransform::Identity);
        assert!(matches!(
            run(&transformed, &mut x, 1 << 20),
            Err(StreamEncodeError::Codec(Error::Unsupported(_)))
        ));
        assert_eq!((x.reads, x.writes), (0, 0));
        assert!(matches!(
            run(&cfg, &mut x, 0),
            Err(StreamEncodeError::Codec(Error::TooLarge))
        ));
        assert_eq!((x.reads, x.writes), (0, 0));
        x.cancel = true;
        assert!(matches!(
            run(&cfg, &mut x, 1 << 20),
            Err(StreamEncodeError::Codec(Error::Cancelled))
        ));
        assert_eq!((x.reads, x.writes), (0, 0));
        let mut x = Faults {
            source_error: true,
            ..Faults::default()
        };
        match run(&cfg, &mut x, 1 << 20).unwrap_err() {
            StreamEncodeError::Read(e) => assert_eq!(e.to_string(), "source witness"),
            e => panic!("wrong source error: {e}"),
        }
        assert_eq!(x.reads, 1);
        let mut x = Faults {
            output_error: true,
            ..Faults::default()
        };
        match run(&cfg, &mut x, 1 << 20).unwrap_err() {
            StreamEncodeError::Write(e) => assert_eq!(e.to_string(), "destination witness"),
            e => panic!("wrong output error: {e}"),
        }
        assert_eq!((x.reads, x.writes), (1, 1));
    }
}
