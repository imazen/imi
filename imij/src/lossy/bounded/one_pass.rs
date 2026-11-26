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
    #[cfg(feature = "rayon")]
    {
        p.one_pass_threads = cfg.threads.max(1);
    }
    p.work = p.sb as usize;
    p.dc_rows = p.work;
    Ok(p)
}

/// Keep cache-sized column partitions while reusing each task's scratch
/// across every block row of the admitted source band. Jobs own disjoint
/// output slices and never exceed the requested worker count.
#[cfg(feature = "rayon")]
pub(super) fn parallel_stripe(
    p: &Plan,
    band: &[u8],
    y0: usize,
    stripe: usize,
    threads: usize,
    aq_code: Option<u8>,
) -> StripeCoefs {
    struct Work<'a> {
        bx: usize,
        bw: usize,
        rows: Vec<(usize, &'a mut [i32], &'a mut [u64])>,
    }
    let (by0, by1) = p.g.rows(stripe);
    let nby = by1 - by0;
    let bw = p.g.bw;
    let workers = threads.min(bw.div_ceil(32)).max(1);
    let chunk = bw.div_ceil(workers).div_ceil(8) * 8;
    let mut q = vec![0i32; nby * bw * NCH * 64];
    let mut nz = vec![0u64; nby * bw * NCH];
    let mut jobs: Vec<_> = (0..bw.div_ceil(chunk))
        .map(|i| Work {
            bx: i * chunk,
            bw: chunk.min(bw - i * chunk),
            rows: Vec::with_capacity(nby),
        })
        .collect();
    for (r, (q, nz)) in q
        .chunks_mut(bw * NCH * 64)
        .zip(nz.chunks_mut(bw * NCH))
        .enumerate()
    {
        for (i, (q, nz)) in q
            .chunks_mut(chunk * NCH * 64)
            .zip(nz.chunks_mut(chunk * NCH))
            .enumerate()
        {
            jobs[i].rows.push((by0 + r, q, nz));
        }
    }
    debug_assert!(jobs.len() <= workers);
    let process = |work: Work<'_>| {
        let Work {
            bx,
            bw: blocks,
            rows,
        } = work;
        let pw = blocks * 8;
        let mut planes = vec![0.0f32; NCH * 8 * pw];
        let mut extremes = Vec::with_capacity(blocks);
        let mut coefs = vec![0.0f32; blocks * 64];
        let aq: Vec<f32> = aq_code.map_or_else(Vec::new, |a| vec![a as f32 / 64.0; blocks]);
        for (by, q, nz) in rows {
            p.fill_block_row_columns(band, y0, by, bx..bx + blocks, &mut planes, &mut extremes);
            for ch in 0..NCH {
                enc_simd::fdct_row(
                    &planes[ch * 8 * pw..][..8 * pw],
                    pw,
                    blocks,
                    &p.cm,
                    &mut coefs,
                );
                enc_simd::quant_row(&coefs, blocks, ch, &p.inv_steps[ch], &aq, p.round, q, nz);
                if p.dc_fix {
                    let ext = (ch == 0 && p.white_code.is_some()).then_some(&extremes[..]);
                    dc_row(&coefs, blocks, ch, p.inv_steps[ch][0], ext, q, nz);
                }
            }
        }
    };
    let mut jobs = jobs.into_iter();
    let first = jobs.next();
    rayon::in_place_scope(|scope| {
        for job in jobs {
            let process = &process;
            scope.spawn(move |_| process(job));
        }
        if let Some(job) = first {
            process(job);
        }
    });
    StripeCoefs {
        by0,
        nby,
        bw,
        q,
        nz,
        aq: aq_code.map_or_else(Vec::new, |a| vec![a; nby * bw]),
        modes: Vec::new(),
        splits: Vec::new(),
    }
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
    const OBSERVATION_WEIGHT: u64 = 64;
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
            let mut hist = vec![vec![0u64; coef::ALPHA]; coef::NRAW];
            for (ctx, row) in hist.iter_mut().enumerate() {
                if ctx < coef::EOB_CTX {
                    row[..64].fill(1);
                } else {
                    for run in 0..=coef::MAX_RUN {
                        for class in 0..coef::MAG_CLASSES {
                            row[(run * 32 + class) as usize] = 1;
                        }
                    }
                    row[coef::ZRL as usize] = 1;
                }
            }
            for stripe in &first {
                for unit in p.units(stripe.clone()) {
                    stripe_tokens(&unit, unit.bw, Group::ALL, |_, ctx, symbol, _, _| {
                        hist[ctx][symbol as usize] += OBSERVATION_WEIGHT;
                    });
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
            let mut luma = vec![vec![1u64; huf::CNT_ALPHA]; huf::LUMA_CTX];
            let mut chroma = vec![vec![1u64; huf::CNT_ALPHA]; huf::CHROMA_CTX];
            let mut wid = vec![vec![1u64; huf::WID_ALPHA]; huf::WID_CTX];
            for stripe in &first {
                for unit in p.units(stripe.clone()) {
                    let (counts, widths) = stripe_counts_widths(&unit, unit.bw);
                    let symbols = huf::stripe_syms(&counts, &widths, unit.nby * unit.bw);
                    for (hist, symbols) in [
                        (&mut luma, &symbols.luma),
                        (&mut chroma, &symbols.chroma),
                        (&mut wid, &symbols.wid),
                    ] {
                        for &(ctx, symbol) in symbols {
                            hist[ctx as usize][symbol as usize] += OBSERVATION_WEIGHT;
                        }
                    }
                }
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
    if cfg.local_ans_contexts && !aq {
        train_first_band_dc(p, &mut prepared)?;
    }
    Ok(prepared)
}

/// Learn DC models from the retained first band for local-context one-pass
/// coding and the compatible study control. Smoothing preserves support for
/// every legal later DC/run token without reading future source rows.
pub(super) fn train_first_band_dc(p: &Plan, prepared: &mut Prepared) -> Result<(), Error> {
    let band = prepared.first_band.take().ok_or(Error::InvalidInput(
        "DC learning requires the retained first band",
    ))?;
    let result = train_dc_band(p, prepared, &band);
    prepared.first_band = Some(band);
    result
}

fn train_dc_band(p: &Plan, prepared: &mut Prepared, band: &[u8]) -> Result<(), Error> {
    assert!(!prepared.fits16);
    let (by0, by1) = p
        .bands()
        .next()
        .ok_or(Error::InvalidInput("empty source band"))?;
    let stripes = p.band_stripes(band, 0, by0, by1);
    let mut dc = Vec::new();
    dc_band(p, &stripes, by0, by1, false, false, &mut dc);
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
    for slot in &mut histograms {
        for context in slot {
            for (symbol, weight) in context.iter_mut().enumerate() {
                *weight = if symbol < legal_end {
                    *weight * 64 + 1
                } else {
                    0
                };
            }
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
