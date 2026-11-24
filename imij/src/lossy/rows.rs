//! Row-at-a-time lossy decoding: `StreamDecoder` and `DecodeSession` rows
//! (records in memory) and `decode_reader` (records from the stream) share
//! one driver. It reads each unit's records in file order (SPEC 2.7: the DC
//! record when its stripe starts a DC stripe, alpha, AC, exact) and
//! reconstructs one stripe at a time, so a forward stream needs no record
//! ahead of the one it reads.

use alloc::vec::Vec;

use super::exact::{self, Exact};
use super::{
    Channels, Error, LossyParsed, NCH, Scratch, dc_values, decode_stripe_bytes_mode, pack_rows,
    resize_fallible,
};
use crate::v1::{OutMap, Parsed};

/// A record the driver asks for: the DC record of DC unit `du`, or unit
/// `u`'s alpha, AC or exact record.
#[derive(Clone, Copy)]
pub(crate) enum Rec {
    Dc(usize),
    Alpha(usize),
    Ac(usize),
    Exact(usize),
}

impl Rec {
    /// The record's kind (SPEC 2.7).
    #[cfg_attr(not(feature = "std"), allow(dead_code))]
    pub(crate) fn kind(self) -> u64 {
        use crate::parse::records::{R_AC, R_ALPHA, R_DC, R_EXACT};
        match self {
            Rec::Dc(_) => R_DC,
            Rec::Alpha(_) => R_ALPHA,
            Rec::Ac(_) => R_AC,
            Rec::Exact(_) => R_EXACT,
        }
    }
}

/// Where the driver's records come from. It asks in file order, so a
/// stream returns its next record and an in-memory payload looks it up.
pub(crate) trait Records {
    fn record(&mut self, r: Rec) -> Result<&[u8], Error>;
    /// The reader's position (lab trace).
    fn pos(&self) -> u64 {
        0
    }
}

/// One retained acquisition future per record. The returned bytes stay
/// borrowed until decoding that record finishes, before another is requested.
pub(crate) trait AsyncRecords {
    async fn record(&mut self, r: Rec) -> Result<&[u8], Error>;
    fn pos(&self) -> u64;
}

impl<R: Records> AsyncRecords for R {
    async fn record(&mut self, r: Rec) -> Result<&[u8], Error> {
        Records::record(self, r)
    }
    fn pos(&self) -> u64 {
        Records::pos(self)
    }
}

/// The records of an in-memory payload, split by the parser.
pub(crate) struct SliceRecords<'a> {
    dc: Vec<&'a [u8]>,
    alpha: Vec<&'a [u8]>,
    ac: Vec<&'a [u8]>,
    exact: Vec<&'a [u8]>,
}

impl Records for SliceRecords<'_> {
    #[inline]
    fn record(&mut self, r: Rec) -> Result<&[u8], Error> {
        Ok(match r {
            Rec::Dc(du) => self.dc[du],
            Rec::Alpha(u) => self.alpha[u],
            Rec::Ac(u) => self.ac[u],
            Rec::Exact(u) => self.exact[u],
        })
    }
}

/// Offset records into owned input. The raster driver never borrows itself.
pub(crate) struct OwnedRecords {
    source: Vec<u8>,
    dc: Vec<core::ops::Range<usize>>,
    alpha: Vec<core::ops::Range<usize>>,
    ac: Vec<core::ops::Range<usize>>,
    exact: Vec<core::ops::Range<usize>>,
}
impl OwnedRecords {
    pub(crate) fn extra_bytes(p: &LossyParsed<'_>) -> u128 {
        let count = p.stripes.len() as u128
            + p.dc.as_ref().map_or(0, |p| p.stripes.len()) as u128
            + p.alpha.as_ref().map_or(0, |p| p.stripes.len()) as u128
            + p.exact.as_ref().map_or(0, |p| p.records.len()) as u128;
        count * core::mem::size_of::<core::ops::Range<usize>>() as u128
    }
    pub(crate) fn spans(
        source: &[u8],
        p: &LossyParsed<'_>,
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<Self, Error> {
        let dc = crate::owned::spans(source, p.dc.as_ref().map_or(&[][..], |p| &p.stripes), check)?;
        let alpha = crate::owned::spans(
            source,
            p.alpha.as_ref().map_or(&[][..], |p| &p.stripes),
            check,
        )?;
        let ac = crate::owned::spans(source, &p.stripes, check)?;
        let exact = crate::owned::spans(
            source,
            p.exact.as_ref().map_or(&[][..], |p| &p.records),
            check,
        )?;
        Ok(Self {
            source: Vec::new(),
            dc,
            alpha,
            ac,
            exact,
        })
    }
}
impl Records for OwnedRecords {
    fn record(&mut self, r: Rec) -> Result<&[u8], Error> {
        let span = match r {
            Rec::Dc(u) => &self.dc[u],
            Rec::Alpha(u) => &self.alpha[u],
            Rec::Ac(u) => &self.ac[u],
            Rec::Exact(u) => &self.exact[u],
        };
        Ok(&self.source[span.clone()])
    }
}
impl LossyStream<'static, OwnedRecords> {
    pub(crate) fn from_owned(
        source: Vec<u8>,
        p: LossyParsed<'static>,
        mut records: OwnedRecords,
    ) -> Result<Self, Error> {
        records.source = source;
        Ok(Self {
            d: LossyRows::new(alloc::boxed::Box::new(p))?,
            src: records,
            next_in_buf: 0,
            stripe: 0,
            y: 0,
        })
    }
}

/// The reader's records, one at a time in a reused buffer.
#[cfg(feature = "std")]
pub(crate) struct StreamRecords<'s, R> {
    pub(crate) src: &'s mut crate::read::Src<R>,
    pub(crate) buf: Vec<u8>,
}

#[cfg(feature = "std")]
impl<R: crate::read::input::Input> AsyncRecords for StreamRecords<'_, R> {
    async fn record(&mut self, r: Rec) -> Result<&[u8], Error> {
        self.src.record_async(&mut self.buf, r.kind()).await?;
        Ok(&self.buf)
    }
    fn pos(&self) -> u64 {
        self.src.pos
    }
}

/// Exact blocks in file order: each unit's record carries the nested
/// stripes that start in it; they decode on arrival and are kept while
/// later units need them.
struct ExactRows {
    om: OutMap,
    /// Units read so far.
    u: usize,
    /// Nested stripes decoded so far.
    t: usize,
    held: Vec<(usize, Vec<u8>)>,
    st: exact::ItemState,
}

/// The row driver: one stripe of the image at a time.
pub(crate) struct LossyRows<'a> {
    p: alloc::boxed::Box<LossyParsed<'a>>,
    dc: Parsed<'a>,
    dc_om: OutMap,
    alpha: Option<(Parsed<'a>, OutMap)>,
    exact: Option<ExactRows>,
    /// Per column group: DC values and block scales of its current DC
    /// stripe. A group's entry is pushed when its first DC record arrives.
    dcg: Vec<(Vec<i32>, Vec<f32>)>,
    dc_row: Vec<u8>,
    dc_px: Vec<u8>,
    /// The decoded stripe: RGB rows, and alpha rows with alpha.
    rgb: Vec<u8>,
    alpha_px: Vec<u8>,
    sc: Scratch,
    /// Rows of the decoded stripe.
    pub(crate) rows: usize,
}

impl<'a> LossyRows<'a> {
    /// The driver of `p`, whose nested DC and alpha heads it takes.
    pub(crate) fn new(p: alloc::boxed::Box<LossyParsed<'a>>) -> Result<Self, Error> {
        Self::with_reference(p, false)
    }

    fn with_reference(
        mut p: alloc::boxed::Box<LossyParsed<'a>>,
        reference: bool,
    ) -> Result<Self, Error> {
        if p.skip && !reference {
            return Err(Error::Corrupt(
                "keep-reference blocks outside an inter frame",
            ));
        }
        let dc = p.dc.take().unwrap();
        let dc_om = OutMap::new(&dc.info, &dc.layout);
        let alpha = p.alpha.take().map(|a| {
            let om = OutMap::new(&a.info, &a.layout);
            (a, om)
        });
        let exact = p.exact.as_ref().map(|e| ExactRows {
            om: OutMap::new(&e.img.info, &e.img.layout),
            u: 0,
            t: 0,
            held: Vec::new(),
            st: exact::ItemState::default(),
        });
        Ok(Self {
            p,
            dc,
            dc_om,
            alpha,
            exact,
            dcg: Vec::new(),
            dc_row: Vec::new(),
            dc_px: Vec::new(),
            rgb: Vec::new(),
            alpha_px: Vec::new(),
            sc: Scratch::default(),
            rows: 0,
        })
    }

    #[cfg(any(feature = "v3", test))]
    pub(crate) fn info(&self) -> &crate::ImageInfo {
        &self.p.info
    }

    /// Bytes of one decoded RGB row.
    fn rgb_bytes(&self) -> usize {
        self.p.g.w * 3 * self.p.info.sample.bytes()
    }

    /// Decodes stripe `s` from its units' records; `check` runs before
    /// every unit and nested row (cancellation).
    pub(crate) fn stripe<R: Records, const BOUNDED: bool>(
        &mut self,
        s: usize,
        src: &mut R,
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        crate::parse::payload::ready(self.stripe_async::<_, BOUNDED>(
            s,
            src,
            check,
            &crate::parse::limits::Unrestricted,
        ))
    }

    pub(crate) async fn stripe_async<R: AsyncRecords, const BOUNDED: bool>(
        &mut self,
        s: usize,
        src: &mut R,
        check: &impl Fn() -> Result<(), Error>,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<(), Error> {
        let g = self.p.g;
        let (by0, by1) = g.rows(s);
        let rows = (by1 * 8).min(g.h) - by0 * 8;
        let bps = self.p.info.sample.bytes();
        let crb = self.rgb_bytes();
        let arb = g.w * bps;
        let ng = self.p.ngroups();
        // Lossy stripes per DC stripe (`p.dc_k()` needs the DC head, taken here).
        #[cfg(all(feature = "lab", feature = "std"))]
        let k = self.dc.hs / g.sb;
        crate::trace::tspan!(
            "stripe",
            "LOSSY",
            alloc::format!("stripe {s} (rows {}-{})", by0 * 8, (by1 * 8).min(g.h) - 1),
            src.pos()
        );
        crate::trace::region(0, (by0 * 8) as u64, g.w as u64, rows as u64);
        // The stripe's buffers grow when its records arrive: a head alone
        // cannot reserve them.
        let rgb_len = rows.checked_mul(crb).ok_or(Error::TooLarge)?;
        let alpha_len = rows.checked_mul(arb).ok_or(Error::TooLarge)?;
        self.rows = 0;
        for gi in 0..ng {
            check()?;
            let (x0, gg) = self.p.group(gi);
            let u = s * ng + gi;
            #[cfg(all(feature = "lab", feature = "std"))]
            super::trace_lossy_group(
                &g,
                s,
                k,
                gi,
                x0,
                gg.w,
                self.alpha.is_some(),
                self.p.color,
                self.dc.info.channels.count() == 4,
            );
            let span = self.p.dc_span(s, gi);
            if s == span.start {
                let rec = src.record(Rec::Dc(span.unit)).await?;
                let ordinal = self.p.unit_map().first(u) as u64;
                if let Some(models) = &mut self.p.model_refs {
                    models.admit(ordinal, 0, rec, &self.dc, admission)?;
                }
                self.dc_unit(span.unit, gi, rec, check)?;
            }
            if let Some((a, om)) = &self.alpha {
                let rec = src.record(Rec::Alpha(u)).await?;
                resize_fallible(&mut self.alpha_px, alpha_len, 0)?;
                let mut sink = crate::v1::Strided {
                    out: &mut self.alpha_px[x0 * bps..],
                    stride: arb,
                    n: gg.w * bps,
                };
                crate::v1::decode_unit_rows(a, om, u, rec, &mut sink, check)?;
            }
            let off = (s - span.start) * g.sb * gg.bw;
            let (dv, av) = self.dcg.get(gi).ok_or(Error::Corrupt("missing DC group"))?;
            if dv.len() < (off + (by1 - by0) * gg.bw) * NCH {
                return Err(Error::Corrupt("DC image too short"));
            }
            let aq = if av.is_empty() { &av[..] } else { &av[off..] };
            let rec = src.record(Rec::Ac(u)).await?;
            let body = self.p.record(&gg, s, rec)?.1;
            let ordinal = (self.p.unit_map().first(u)
                + usize::from(s == span.start)
                + usize::from(self.alpha.is_some())) as u64;
            if let Some(models) = &mut self.p.model_refs {
                models.admit(ordinal, 1, body, &self.dc, admission)?;
            }
            resize_fallible(&mut self.rgb, rgb_len, 0)?;
            decode_stripe_bytes_mode::<BOUNDED>(
                &self.p,
                &gg,
                s,
                rec,
                &dv[off * NCH..],
                aq,
                &mut self.rgb[x0 * 3 * bps..],
                crb,
                true,
                &mut self.sc,
            )?;
            if let (Some(e), Some(ex)) = (&self.p.exact, &mut self.exact) {
                let rec = src.record(Rec::Exact(u)).await?;
                let mut rows_of = exact::Strided {
                    buf: &mut self.rgb,
                    stride: crb,
                    off: x0 * 3,
                    rows,
                };
                ex.unit(e, rec, s, &gg, aq, &mut rows_of, check)?;
            }
        }
        crate::trace::exit(src.pos());
        self.rows = rows;
        Ok(())
    }

    /// Decodes DC unit `du` (column group `gi`) from `rec` into the group's
    /// DC values.
    fn dc_unit(
        &mut self,
        du: usize,
        gi: usize,
        rec: &[u8],
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let (model, rec) = if let Some(models) = &self.p.model_refs {
            models.dc(rec, &self.dc)?
        } else {
            (None, rec)
        };
        let dc = model.as_deref().unwrap_or(&self.dc);
        let alternate = model
            .as_ref()
            .map(|p| crate::v1::OutMap::new(&p.info, &p.layout));
        let om = alternate.as_ref().unwrap_or(&self.dc_om);
        self.dc_px.clear();
        resize_fallible(&mut self.dc_row, dc.info.row_bytes(), 0)?;
        let dc_px = &mut self.dc_px;
        crate::v1::unit_rows(
            dc,
            om,
            du,
            rec,
            &mut self.dc_row,
            |r| {
                dc_px.try_reserve(r.len()).map_err(|_| Error::TooLarge)?;
                dc_px.extend_from_slice(r);
                Ok(())
            },
            check,
        )?;
        let ng = self.p.ngroups();
        while self.dcg.len() <= gi {
            if self.dcg.len() == self.dcg.capacity() {
                let grow = self.dcg.capacity().max(1).min(ng - self.dcg.len());
                self.dcg.try_reserve(grow).map_err(|_| Error::TooLarge)?;
            }
            self.dcg.push((Vec::new(), Vec::new()));
        }
        let (dv, av) = &mut self.dcg[gi];
        dv.clear();
        av.clear();
        dc_values(
            self.dc.info.sample,
            self.p.aq,
            self.p.exact.is_some(),
            &self.dc_px,
            dv,
            av,
        )
    }

    /// Row `j` of the decoded stripe as RGB samples (images without alpha).
    pub(crate) fn rgb_row(&self, j: usize) -> &[u8] {
        let crb = self.rgb_bytes();
        &self.rgb[j * crb..][..crb]
    }

    /// Writes row `j` of the decoded stripe, in the image's channel order,
    /// into `out` (its row bytes).
    pub(crate) fn pack_row(&self, j: usize, out: &mut [u8]) {
        let src = self.rgb_row(j);
        if self.p.info.channels == Channels::Rgb {
            out.copy_from_slice(src);
        } else {
            let arb = self.p.g.w * self.p.info.sample.bytes();
            let a = self
                .alpha
                .as_ref()
                .map(|_| &self.alpha_px[j * arb..][..arb]);
            pack_rows(&self.p.info, self.p.g.w, src, a, &mut [out]);
        }
    }

    /// After the last stripe: every exact block and nested stripe arrived.
    #[cfg(feature = "std")]
    pub(crate) fn finish(&self) -> Result<(), Error> {
        if let (Some(e), Some(ex)) = (&self.p.exact, &self.exact)
            && (ex.u != e.ranges.len() || ex.t != e.nested_stripes())
        {
            return Err(Error::Corrupt("exact blocks missing"));
        }
        Ok(())
    }
}

impl ExactRows {
    /// Applies a unit's exact record `rec` (stripe `s` of a column group
    /// with geometry `gg`, block scales `aq`) to its RGB8 rows.
    #[allow(clippy::too_many_arguments)]
    fn unit(
        &mut self,
        e: &Exact<'_>,
        rec: &[u8],
        s: usize,
        gg: &super::Geom,
        aq: &[f32],
        rows: &mut exact::Strided<'_>,
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let u = self.u;
        let mut parts = Vec::new();
        e.split_record(rec, u, &mut parts)?;
        for (t, part) in parts {
            let px = e.decode_stripe_bytes(&self.om, t, part, check)?;
            self.held.try_reserve(1).map_err(|_| Error::TooLarge)?;
            self.held.push((t, px));
            self.t += 1;
        }
        let nested = exact::Nested::Held(&self.held);
        let range = e.range(u)?;
        exact::apply_unit(e, nested, &mut self.st, range, s, gg, aq, rows, check)?;
        self.u += 1;
        // Later units only need list entries from the next unit's on.
        let per = e.per_stripe();
        let next = range.0 + range.1;
        self.held.retain(|(t, _)| (t + 1) * per > next);
        Ok(())
    }
}

/// Row-at-a-time decoding of an in-memory payload (`StreamDecoder`,
/// `DecodeSession` rows): the driver over its split records.
pub(crate) struct LossyStream<'a, R = SliceRecords<'a>> {
    d: LossyRows<'a>,
    src: R,
    next_in_buf: usize,
    stripe: usize,
    y: usize,
}

impl<'a> LossyStream<'a> {
    #[cfg(any(feature = "v3", test))]
    pub(crate) fn new(
        info: crate::ImageInfo,
        version: u8,
        color: crate::LossyColor,
        lossy: crate::parse::header::Features,
        body: &'a [u8],
    ) -> Result<Self, Error> {
        Self::from_parsed(super::parse_lossy_as(info, version, color, lossy, body)?)
    }

    pub(crate) fn from_parsed(mut p: LossyParsed<'a>) -> Result<Self, Error> {
        let take = |n: &mut Option<Parsed<'a>>| n.as_mut().map(|n| core::mem::take(&mut n.stripes));
        let src = SliceRecords {
            dc: take(&mut p.dc).unwrap_or_default(),
            alpha: take(&mut p.alpha).unwrap_or_default(),
            ac: core::mem::take(&mut p.stripes),
            exact: p
                .exact
                .as_mut()
                .map(|e| core::mem::take(&mut e.records))
                .unwrap_or_default(),
        };
        Ok(Self {
            d: LossyRows::new(alloc::boxed::Box::new(p))?,
            src,
            next_in_buf: 0,
            stripe: 0,
            y: 0,
        })
    }

    /// Per stripe: its first pixel row and its last record in file order:
    /// the last group's exact record, or without exact blocks its lossy
    /// record (after the DC and alpha records it needs).
    #[cfg(feature = "v3")]
    pub(crate) fn stripe_ends(&self) -> Vec<(u32, &'a [u8])> {
        let p = &self.d.p;
        let ng = p.ngroups();
        (0..p.g.stripes())
            .map(|s| {
                let u = s * ng + ng - 1;
                let last = match p.exact {
                    Some(_) => self.src.exact[u],
                    None => self.src.ac[u],
                };
                ((p.g.rows(s).0 * 8) as u32, last)
            })
            .collect()
    }
}

impl<R: Records> LossyStream<'_, R> {
    #[cfg(any(feature = "v3", test))]
    pub(crate) fn info(&self) -> &crate::ImageInfo {
        self.d.info()
    }

    #[cfg(feature = "v3")]
    pub(crate) fn rows_decoded(&self) -> u32 {
        self.y as u32
    }

    /// Test-only: groups whose DC values are held, and the capacity of the
    /// nested images' decoded state (DC values and rows, alpha rows).
    #[cfg(all(test, feature = "encode"))]
    pub(crate) fn retained_nested_capacity(&self) -> (usize, usize) {
        let d = &self.d;
        let dc = d.dcg.capacity() * size_of::<(Vec<i32>, Vec<f32>)>()
            + d.dcg
                .iter()
                .map(|(v, a)| v.capacity() * size_of::<i32>() + a.capacity() * size_of::<f32>())
                .sum::<usize>();
        (
            d.dcg.len(),
            dc + d.dc_row.capacity() + d.dc_px.capacity() + d.alpha_px.capacity(),
        )
    }

    #[cfg(all(all(test, feature = "encode"), feature = "lab", feature = "std"))]
    pub(crate) fn reconstruction_capacity_bytes(&self) -> usize {
        (self.d.sc.planes.capacity() + self.d.sc.lin.capacity()) * core::mem::size_of::<f32>()
    }

    #[cfg(any(feature = "v3", test))]
    pub(crate) fn next_row(&mut self, out: &mut [u8]) -> Result<bool, Error> {
        self.next_row_mode::<false>(out, &|| Ok(()))
    }

    pub(crate) fn next_row_checked(
        &mut self,
        out: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<bool, Error> {
        self.next_row_mode::<true>(out, check)
    }

    fn next_row_mode<const BOUNDED: bool>(
        &mut self,
        out: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<bool, Error> {
        check()?;
        if self.y >= self.d.p.info.height as usize {
            return Ok(false);
        }
        let rb = self.d.p.info.row_bytes();
        if out.len() < rb {
            return Err(Error::InvalidInput("row buffer too small"));
        }
        if self.next_in_buf == self.d.rows {
            self.d
                .stripe::<_, BOUNDED>(self.stripe, &mut self.src, check)?;
            self.next_in_buf = 0;
            self.stripe += 1;
        }
        check()?;
        self.d.pack_row(self.next_in_buf, &mut out[..rb]);
        self.next_in_buf += 1;
        self.y += 1;
        check()?;
        Ok(true)
    }
}

/// Reader decoding of a lossy `DATA` payload after its head `p` (read by
/// `parse::payload::read_head`): the driver over the stream's records, one
/// record held at a time.
#[cfg(feature = "std")]
pub(crate) async fn read_lossy<R: crate::read::input::Input, A: crate::parse::limits::Admission>(
    src: &mut crate::read::Src<R>,
    p: alloc::boxed::Box<LossyParsed<'static>>,
    admission: &A,
    on_row: &mut (impl crate::read::input::Rows + ?Sized),
) -> Result<(), Error> {
    let mut p = p;
    let info = &p.info.clone();
    crate::trace::exit(src.pos);
    if let Some(room) = src.remaining() {
        crate::parse::records::check_count(p.ngroups() * p.g.stripes(), room)?;
    }
    src.begin_records_with(p.hint.take(), p.unit_ordinals)?;
    let stripes = p.g.stripes();
    let mut d = LossyRows::with_reference(p, on_row.has_reference())?;
    let mut src = StreamRecords {
        src,
        buf: Vec::new(),
    };
    let mut out_row = Vec::new();
    let mut y = 0u32;
    for s in 0..stripes {
        admission.check()?;
        if d.p.skip {
            // Keep-reference blocks are defined only for RGB. Load just this
            // stripe; reconstruction overwrites every non-keep block.
            let (by0, by1) = d.p.g.rows(s);
            let rows = (by1 * 8).min(d.p.g.h) - by0 * 8;
            let rb = info.row_bytes();
            resize_fallible(&mut d.rgb, rows.checked_mul(rb).ok_or(Error::TooLarge)?, 0)?;
            for (j, row) in d.rgb.chunks_exact_mut(rb).enumerate() {
                on_row.reference(info, y + j as u32, row).await?;
            }
        }
        d.stripe_async::<_, false>(s, &mut src, &|| Ok(()), admission)
            .await?;
        if info.channels != Channels::Rgb {
            resize_fallible(&mut out_row, info.row_bytes(), 0)?;
        }
        for j in 0..d.rows {
            if info.channels == Channels::Rgb {
                on_row.row(info, y, d.rgb_row(j)).await?;
            } else {
                d.pack_row(j, &mut out_row);
                on_row.row(info, y, &out_row).await?;
            }
            y += 1;
        }
    }
    d.finish()
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, ImageInfo, LossyColor, LossyConfig, Metadata, SampleType};

    fn key(r: Rec) -> (u8, usize) {
        match r {
            Rec::Dc(i) => (0, i),
            Rec::Alpha(u) => (1, u),
            Rec::Ac(u) => (2, u),
            Rec::Exact(u) => (3, u),
        }
    }

    /// Records from a payload, logged; optionally failing or cut short at
    /// request `n`.
    struct Log<'a> {
        inner: SliceRecords<'a>,
        seen: Vec<(u8, usize)>,
        fail_at: Option<usize>,
        cut_at: Option<usize>,
    }

    impl Records for Log<'_> {
        fn record(&mut self, r: Rec) -> Result<&[u8], Error> {
            let n = self.seen.len();
            self.seen.push(key(r));
            if Some(n) == self.fail_at {
                return Err(Error::Read);
            }
            let b = Records::record(&mut self.inner, r)?;
            Ok(if Some(n) == self.cut_at {
                &b[..b.len() / 2]
            } else {
                b
            })
        }
    }

    fn files() -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for (w, h, ch, cfg) in [
            (40, 37, Channels::Rgb, LossyConfig::new(85.0)),
            (33, 150, Channels::Rgba, LossyConfig::new(60.0)),
            (
                70,
                300,
                Channels::Rgb,
                LossyConfig::new(30.0).with_color(LossyColor::YCoCg),
            ),
            (24, 9, Channels::Gray, LossyConfig::new(95.0)),
            // Tall and narrow: a DC stripe spans several lossy stripes.
            (
                16,
                2400,
                Channels::Rgb,
                LossyConfig::new(70.0).with_stripe_block_rows(1),
            ),
            (
                40,
                300,
                Channels::Rgba,
                LossyConfig::new(80.0).with_stripe_block_rows(2),
            ),
        ] {
            let info = ImageInfo::new(w, h, ch, SampleType::U8);
            let n = info.row_bytes() * h as usize;
            let px: Vec<u8> = (0..n)
                .map(|i| ((i * 31) % 253) as u8 ^ ((i / 97) as u8))
                .collect();
            out.push(
                crate::encode_lossy(&px, info.row_bytes(), &info, &Metadata::default(), &cfg)
                    .unwrap(),
            );
        }
        out
    }

    fn driver(file: &[u8]) -> (LossyRows<'_>, SliceRecords<'_>) {
        let f = crate::container::parse(file, false, &crate::v1::payload_len).unwrap();
        let color = f.header.lossy_color.unwrap();
        let s = LossyStream::new(
            f.header.info.clone(),
            f.header.version,
            color,
            f.declared,
            f.data,
        )
        .unwrap();
        (s.d, s.src)
    }

    /// The order SPEC 2.7 gives a lossy payload's records.
    fn spec_order(
        stripes: usize,
        ng: usize,
        k: usize,
        alpha: bool,
        exact: bool,
    ) -> Vec<(u8, usize)> {
        let mut v = Vec::new();
        for s in 0..stripes {
            for g in 0..ng {
                let u = s * ng + g;
                if s % k == 0 {
                    v.push((0, s / k * ng + g));
                }
                if alpha {
                    v.push((1, u));
                }
                v.push((2, u));
                if exact {
                    v.push((3, u));
                }
            }
        }
        v
    }

    #[test]
    fn row_driver_asks_for_records_in_file_order() {
        let mut saw_dc_span = false;
        let mut saw_alpha = false;
        let mut spans = Vec::new();
        for file in files() {
            let whole = crate::decode(&file).unwrap().1;
            let (mut d, src) = driver(&file);
            let mut log = Log {
                inner: src,
                seen: Vec::new(),
                fail_at: None,
                cut_at: None,
            };
            let (ng, stripes) = (d.p.ngroups(), d.p.g.stripes());
            let k = d.dc.hs / d.p.g.sb;
            let (alpha, exact) = (d.alpha.is_some(), d.p.exact.is_some());
            saw_dc_span |= k > 1 && stripes > k;
            spans.push((k, stripes));
            saw_alpha |= alpha;
            let rb = d.info().row_bytes();
            let mut rows = Vec::new();
            for s in 0..stripes {
                d.stripe::<_, false>(s, &mut log, &|| Ok(())).unwrap();
                for j in 0..d.rows {
                    let mut row = vec![0u8; rb];
                    d.pack_row(j, &mut row);
                    rows.extend_from_slice(&row);
                }
            }
            assert!(rows == whole, "rows differ from the whole-image decode");
            let want = spec_order(stripes, ng, k, alpha, exact);
            assert_eq!(log.seen, want);
            // The closed form the readers use agrees with the order.
            let map = crate::parse::records::UnitMap::lossy(ng, k, alpha, exact);
            assert_eq!(map.count(stripes), Ok(want.len()));
            for u in 0..stripes * ng {
                let first = want
                    .iter()
                    .position(|&(t, i)| t != 0 && i == u)
                    .map(|p| {
                        if p > 0 && want[p - 1].0 == 0 {
                            p - 1
                        } else {
                            p
                        }
                    })
                    .unwrap();
                assert_eq!(map.first(u), first, "unit {u}");
            }
        }
        assert!(
            saw_dc_span,
            "no file whose DC stripes span several stripes: {spans:?}"
        );
        assert!(saw_alpha);
    }

    /// A record source that fails, or returns a record cut short, at any
    /// request: the driver returns an error there (no panic, no rows past
    /// it).
    #[test]
    fn row_driver_errors_at_any_record() {
        for file in files() {
            let (d, _) = driver(&file);
            let (ng, stripes) = (d.p.ngroups(), d.p.g.stripes());
            let k = d.dc.hs / d.p.g.sb;
            let total = spec_order(stripes, ng, k, d.alpha.is_some(), d.p.exact.is_some()).len();
            for n in 0..total {
                for cut in [false, true] {
                    let (mut d, src) = driver(&file);
                    let mut log = Log {
                        inner: src,
                        seen: Vec::new(),
                        fail_at: (!cut).then_some(n),
                        cut_at: cut.then_some(n),
                    };
                    let r = (0..stripes)
                        .try_for_each(|s| d.stripe::<_, false>(s, &mut log, &|| Ok(())));
                    if cut {
                        // Half a record: the decoder notices, or (rarely) a
                        // record whose first half is a valid record decodes.
                        assert!(log.seen.len() > n);
                    } else {
                        assert_eq!(r, Err(Error::Read), "request {n}");
                        assert_eq!(log.seen.len(), n + 1, "requests after the failure");
                    }
                }
            }
        }
    }
}
