//! Existing-syntax AQ trials. All records of a source band are staged before
//! publication; global heads and the reserved final tail charge the file cap.
use super::*;
use crate::parse::records::{R_AC, R_ALPHA, R_DC, framed_len};
use crate::stream_encode::GroupRect;

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub frame_bytes: u64,
    pub band_record_bytes: u64,
}

impl Limits {
    pub(super) fn tail(self, p: &Plan, cfg: &LossyConfig) -> Result<u64, Error> {
        // Each span is at most the admitted band/file size. The fixed
        // allowance is >= uIDX tag+length(14)+footer(20), plus cSUM
        // tag+length(14)+three fields(3)+coverage varint(10) = 61 bytes.
        // Covered bytes are a subset of the admitted complete file.
        let index = (p.record_count(false) as u64)
            .checked_mul(crate::varint_len(self.band_record_bytes.min(self.frame_bytes)) as u64)
            .ok_or(Error::TooLarge)?;
        let hashes = if cfg.checksum {
            self.frame_bytes
                .div_ceil(1 << STREAM_PIECE_LOG2)
                .checked_mul(cfg.hash.width() as u64)
                .ok_or(Error::TooLarge)?
        } else {
            0
        };
        index
            .checked_add(hashes)
            .and_then(|n| n.checked_add(64))
            .ok_or(Error::TooLarge)
    }
}

pub(crate) fn admit(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    max_index_bytes: u64,
    max_working_bytes: u64,
    limits: Limits,
) -> Result<(), Error> {
    if limits.frame_bytes == 0 || limits.band_record_bytes == 0 {
        return Err(Error::InvalidInput("rate budgets must be positive"));
    }
    // The common admission checks row and record geometry in u128 before
    // plan() uses usize products. Preserve that ordering on 32-bit targets.
    super::admit(info, meta, cfg, max_index_bytes, max_working_bytes)?;
    let p = configure(prepare_plan(info, cfg)?.plan, cfg)?;
    if !matches!(info.sample, SampleType::U8 | SampleType::U16) {
        return Err(Error::Unsupported(
            "budgeted one-pass requires U8/U16 input",
        ));
    }
    if limits.tail(&p, cfg)? >= limits.frame_bytes {
        return Err(Error::TooLarge);
    }
    // Trial coefficients/units and staged DC/AC records coexist with the
    // source band and alpha. This is allocation admission, not an RSS claim.
    let blocks = p.work as u128 * p.g.bw as u128;
    let extra = blocks * (3 * NCH as u128 * (64 * 4 + 8) + 64)
        + p.work as u128 * 8 * p.g.w as u128 * p.bps as u128;
    let remaining = max_working_bytes
        .checked_sub(u64::try_from(extra).map_err(|_| Error::TooLarge)?)
        .ok_or(Error::TooLarge)?;
    super::admit(info, meta, cfg, max_index_bytes, remaining)
}

fn preview(
    session: &GroupEncodeSession,
    rect: GroupRect,
    bytes: &[u8],
    stride: usize,
) -> Result<Vec<u8>, Error> {
    let (encoded, start) = session.preview_group(rect, bytes, stride)?;
    let framed = &encoded[start..];
    let mut r = ByteReader::new(framed);
    if r.varint()? != crate::v1::R_STRIPE {
        return Err(Error::Corrupt("rate trial nested record"));
    }
    let n = usize::try_from(r.varint()?).map_err(|_| Error::TooLarge)?;
    if framed.len() - r.pos != n {
        return Err(Error::Corrupt("rate trial nested length"));
    }
    Ok(framed[r.pos..].to_vec())
}

fn ac_record(
    ac: &Ac,
    rl: Option<&EncodeTables<'_>>,
    huffman: Option<&huf::Encoder<'_>>,
    u: &StripeCoefs,
) -> Vec<u8> {
    let mut out = Vec::new();
    match ac {
        Ac::Rl(_) => {
            let mut lanes: [Vec<Sym>; NL] = core::array::from_fn(|_| Vec::new());
            stripe_symbols(u, u.bw, &mut lanes);
            encode_lanes_tables(rl.unwrap(), &lanes, u.bw.min(NL), &mut out);
        }
        Ac::Huf(t) => {
            let encoder = huffman.unwrap();
            let (counts, widths) = stripe_counts_widths(u, u.bw);
            let symbols = huf::stripe_syms(&counts, &widths, u.nby * u.bw);
            let (magnitudes, signs) = stripe_raw_split(u, u.bw, &counts, &widths);
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
    }
    out
}

pub(in crate::lossy::bounded) async fn pass2<X: StreamIo>(
    p: &Plan,
    cfg: &LossyConfig,
    prep: &mut Prepared,
    io: &mut Drive<'_, X>,
    limits: Limits,
) -> Result<(), Error> {
    let reserve = limits.tail(p, cfg)?;
    let rl = match &prep.ac {
        Ac::Rl(t) => Some(EncodeTables::new(&t.freq, &t.ctx_map)),
        _ => None,
    };
    let huffman = match &prep.ac {
        Ac::Huf(t) => Some(t.encoder()),
        _ => None,
    };
    let mut band = Vec::new();
    let mut alpha_plane = Vec::new();
    let columns = if p.gcols == 0 { p.g.bw } else { p.gcols / 8 };
    for (by0, by1) in p.bands() {
        let y0 = if let Some(first) = prep.first_band.take() {
            band = first;
            0
        } else {
            io.rows(2, by0, by1, &mut band).await?
        };
        let available = limits
            .frame_bytes
            .checked_sub(reserve)
            .and_then(|n| n.checked_sub(io.end))
            .ok_or(Error::TooLarge)?
            .div_ceil(p.g.bh.saturating_sub(by0).div_ceil(p.work) as u64)
            .min(limits.band_record_bytes);
        let stripe = by0 / p.sb as usize;
        // Alpha never depends on the AQ trial. Stage it once; a failed
        // encode is terminal, so no advanced alpha state escapes a refusal.
        let mut alpha = Vec::new();
        if let Some((session, _)) = &mut prep.alpha {
            let rows = p.alpha_plane(&band, y0, stripe, &mut alpha_plane);
            for _ in 0..p.ng {
                let rect = session
                    .next_group()
                    .ok_or(Error::Corrupt("rate alpha order"))?;
                if rect.y as usize != y0 || rect.height as usize != rows {
                    return Err(Error::Corrupt("rate alpha geometry"));
                }
                session.push_group(&alpha_plane[rect.x as usize * p.bps..], p.g.w * p.bps)?;
                alpha.push(take_stripe_record(session)?);
            }
        }
        let alpha_bytes = alpha.iter().try_fold(0u64, |n, a| {
            n.checked_add(framed_len(R_ALPHA, a.len()))
                .ok_or(Error::TooLarge)
        })?;
        let mut chosen = None;
        // The first fitting tested scale wins. Entropy sizes need not be
        // monotonic; failure means none of these candidates fit, not that
        // every encoder permitted by the format must fail.
        for code in [64u8, 80, 96, 128, 160, 192, 224, 255] {
            io.stop().check().map_err(|_| Error::Cancelled)?;
            let sc = p.stripe_with_aq(&band, y0, stripe, Some(code));
            let mut dc = Vec::new();
            dc_band(
                p,
                core::slice::from_ref(&sc),
                by0,
                by1,
                false,
                true,
                &mut dc,
            );
            let stride = p.g.bw * 16;
            let mut dc_records = Vec::new();
            let mut ac_records = Vec::new();
            let mut bytes = alpha_bytes;
            for (gi, u) in p.units(sc).into_iter().enumerate() {
                let x = gi * columns;
                let rect = GroupRect {
                    x: x as u32,
                    y: by0 as u32,
                    width: columns.min(p.g.bw - x) as u32,
                    height: (by1 - by0) as u32,
                };
                let d = preview(&prep.dc_session, rect, &dc[x * 16..], stride)?;
                let a = ac_record(&prep.ac, rl.as_ref(), huffman.as_ref(), &u);
                bytes = bytes
                    .checked_add(framed_len(R_DC, d.len()))
                    .and_then(|n| n.checked_add(framed_len(R_AC, a.len())))
                    .ok_or(Error::TooLarge)?;
                dc_records.push(d);
                ac_records.push(a);
            }
            if bytes <= available {
                chosen = Some((dc, dc_records, ac_records, bytes));
                break;
            }
        }
        let (dc, expected_dc, ac, bytes) = chosen.ok_or(Error::TooLarge)?;
        let mut actual_dc = Vec::new();
        for expected in expected_dc {
            let rect = prep
                .dc_session
                .next_group()
                .ok_or(Error::Corrupt("rate DC order"))?;
            prep.dc_session
                .push_group(&dc[rect.x as usize * 16..], p.g.bw * 16)?;
            let actual = take_stripe_record(&mut prep.dc_session)?;
            if actual != expected {
                return Err(Error::Corrupt("rate trial changed DC payload"));
            }
            actual_dc.push(actual);
        }
        let before = io.end;
        for (gi, (dc, ac)) in actual_dc.into_iter().zip(ac).enumerate() {
            io.emit(R_DC, &dc).await?;
            if let Some(alpha) = alpha.get(gi) {
                io.emit(R_ALPHA, alpha).await?;
            }
            io.emit(R_AC, &ac).await?;
        }
        if io.end - before != bytes {
            return Err(Error::Corrupt("rate band byte accounting"));
        }
    }
    if !prep.dc_session.is_finished() || prep.alpha.as_ref().is_some_and(|(s, _)| !s.is_finished())
    {
        return Err(Error::Corrupt("rate nested image incomplete"));
    }
    Ok(())
}
