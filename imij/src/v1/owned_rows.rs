//! Owned compressed storage over the same resumable stripe arithmetic.
use super::*;
use core::ops::Range;

// A native image has at most four channels, each with at most HI/LO slots.
const MAX_SLOTS: usize = 8;
struct OwnedStripe<S: crate::px::Px> {
    core: StripeCore<S>,
    words: Vec<Range<usize>>,
}
impl<S: crate::px::Px> OwnedStripe<S> {
    fn new(p: &Parsed<'_>, u: usize, bytes: &[u8], source: &[u8]) -> Result<Self, Error> {
        let state = StripeState::<S>::new_with(p, u, bytes)?;
        if state.words.len() > MAX_SLOTS {
            return Err(Error::Corrupt("too many coded slots"));
        }
        let mut words = Vec::new();
        words
            .try_reserve_exact(state.words.len())
            .map_err(|_| Error::TooLarge)?;
        for word in &state.words {
            words.push(word.span(source)?);
        }
        Ok(Self {
            core: state.core,
            words,
        })
    }
    fn next(
        &mut self,
        source: &[u8],
        p: &Parsed<'_>,
        om: &OutMap,
        out: &mut [u8],
    ) -> Result<(), Error> {
        // Only stack views change between rows. Entropy state and predictor
        // buffers persist; neither records nor preceding rows are decoded again.
        // Avoid initializing eight padded tails and then overwriting the used
        // entries on every row. Slot count is fixed by the admitted head.
        macro_rules! views {
            ($n:literal) => {{
                let words: [Words<'_>; $n] = core::array::from_fn(|i| {
                    // These immutable spans came from successfully parsed Words
                    // in new(); their lengths and source bounds cannot change.
                    Words::from_body(&source[self.words[i].clone()])
                        .expect("validated immutable word stream")
                });
                self.core.next_row(&words, p, om, out)
            }};
        }
        match self.words.len() {
            0 => views!(0),
            1 => views!(1),
            2 => views!(2),
            3 => views!(3),
            4 => views!(4),
            5 => views!(5),
            6 => views!(6),
            7 => views!(7),
            8 => views!(8),
            _ => Err(Error::Corrupt("too many coded slots")),
        }
    }
}
#[cfg_attr(
    not(any(feature = "extended", feature = "encode")),
    allow(
        clippy::large_enum_variant,
        reason = "keep stripe state inline after wide variant removal"
    )
)]
enum OwnedUnit {
    Quick(alloc::boxed::Box<crate::quick::State>, Range<usize>),
    S16(OwnedStripe<u16>),
    #[cfg(any(feature = "extended", feature = "encode"))]
    S32(OwnedStripe<u32>),
}
impl OwnedUnit {
    fn new(p: &Parsed<'_>, u: usize, bytes: &[u8], source: &[u8]) -> Result<Self, Error> {
        if p.quick {
            return Ok(Self::Quick(
                alloc::boxed::Box::new(crate::quick::State::new(p, u, bytes)?),
                crate::owned::span(source, bytes)?,
            ));
        }
        #[cfg(any(feature = "extended", feature = "encode"))]
        if !p.narrow() {
            return Ok(Self::S32(OwnedStripe::new(p, u, bytes, source)?));
        }
        Ok(Self::S16(OwnedStripe::new(p, u, bytes, source)?))
    }
    fn next(
        &mut self,
        source: &[u8],
        p: &Parsed<'_>,
        om: &OutMap,
        out: &mut [u8],
    ) -> Result<(), Error> {
        match self {
            Self::Quick(s, span) => s.next(&source[span.clone()], out),
            Self::S16(s) => s.next(source, p, om, out),
            #[cfg(any(feature = "extended", feature = "encode"))]
            Self::S32(s) => s.next(source, p, om, out),
        }
    }
    fn done(&self) -> bool {
        match self {
            Self::Quick(s, _) => s.done(),
            Self::S16(s) => s.core.row == s.core.rows,
            #[cfg(any(feature = "extended", feature = "encode"))]
            Self::S32(s) => s.core.row == s.core.rows,
        }
    }
}

/// One received stripe, independent of the lifetime/address of its input.
/// Entropy and predictor state survive between calls. The owner can wait for
/// more compressed input after draining this stripe, then reuse its buffers.
#[cfg(feature = "std")]
pub(super) struct RecordStripe {
    records: Vec<Vec<u8>>,
    units: Vec<OwnedUnit>,
    failed: Option<Error>,
}

#[cfg(feature = "std")]
impl RecordStripe {
    pub(super) fn new(p: &Parsed<'_>, stripe: usize, records: Vec<Vec<u8>>) -> Result<Self, Error> {
        let ng = p.ngroups();
        if stripe >= p.stripe_count() || records.len() != ng {
            return Err(Error::Corrupt("column group records"));
        }
        let first = stripe.checked_mul(ng).ok_or(Error::TooLarge)?;
        // Keep the existing admission order: validate the first record before
        // allocating a group-state table. Each offset is local to its record.
        let initial = OwnedUnit::new(p, first, &records[0], &records[0])?;
        let mut units = Vec::new();
        units.try_reserve_exact(ng).map_err(|_| Error::TooLarge)?;
        units.push(initial);
        for (gi, record) in records.iter().enumerate().skip(1) {
            units.push(OwnedUnit::new(p, first + gi, record, record)?);
        }
        Ok(Self {
            records,
            units,
            failed: None,
        })
    }

    pub(super) fn next_row(
        &mut self,
        p: &Parsed<'_>,
        om: &OutMap,
        row: &mut [u8],
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<bool, Error> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = (|| {
            check()?;
            if self.units.first().is_none_or(OwnedUnit::done) {
                return Ok(false);
            }
            if row.len() < p.info.row_bytes() {
                return Err(Error::InvalidInput("short output row"));
            }
            let bpp = p.bpp();
            for (gi, (unit, record)) in self.units.iter_mut().zip(&self.records).enumerate() {
                check()?;
                let (x, width, _) = p.group(gi);
                unit.next(record, p, om, &mut row[x * bpp..][..width * bpp])?;
            }
            check()?;
            Ok(true)
        })();
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }

    pub(super) fn into_records(self) -> Vec<Vec<u8>> {
        self.records
    }
}

pub(crate) struct OwnedLossless {
    source: Vec<u8>,
    p: Parsed<'static>,
    spans: Vec<Range<usize>>,
    om: OutMap,
    stripe: usize,
    cur: Vec<OwnedUnit>,
    y: usize,
}
impl OwnedLossless {
    pub(crate) fn new(source: Vec<u8>, p: Parsed<'static>, spans: Vec<Range<usize>>) -> Self {
        let om = OutMap::new(&p.info, &p.layout);
        Self {
            source,
            p,
            spans,
            om,
            stripe: 0,
            cur: Vec::new(),
            y: 0,
        }
    }
    pub(crate) fn next_row_checked(
        &mut self,
        out: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<bool, Error> {
        check()?;
        if self.y >= self.p.info.height as usize {
            return Ok(false);
        }
        if self.cur.is_empty() {
            let ng = self.p.ngroups();
            self.cur
                .try_reserve_exact(ng)
                .map_err(|_| Error::TooLarge)?;
            for gi in 0..ng {
                check()?;
                let u = self.stripe * ng + gi;
                self.cur.push(OwnedUnit::new(
                    &self.p,
                    u,
                    &self.source[self.spans[u].clone()],
                    &self.source,
                )?);
            }
        }
        for (gi, unit) in self.cur.iter_mut().enumerate() {
            check()?;
            let (x, w, _) = self.p.group(gi);
            let bpp = self.p.bpp();
            unit.next(
                &self.source,
                &self.p,
                &self.om,
                &mut out[x * bpp..][..w * bpp],
            )?;
        }
        if self.cur.first().is_none_or(OwnedUnit::done) {
            self.cur.clear();
            self.stripe += 1;
        }
        self.y += 1;
        check()?;
        Ok(true)
    }
}

/// Additional peak storage during borrowed-to-offset conversion of one unit.
/// The main row plan already includes the larger borrowed Words descriptors.
pub(crate) fn owned_extra_bytes(p: &Parsed<'_>) -> u128 {
    (p.stripes.len() as u128) * size_of::<Range<usize>>() as u128
        + MAX_SLOTS as u128 * size_of::<Range<usize>>() as u128
        + size_of::<OwnedLossless>() as u128
}

#[cfg(all(test, feature = "std", feature = "encode"))]
mod record_tests {
    use super::*;
    use core::cell::Cell;

    #[test]
    fn retained_record_rows_match_samples_across_groups_and_stripes() {
        for sample in [
            SampleType::U8,
            SampleType::U16,
            SampleType::F16,
            SampleType::F32,
            #[cfg(feature = "extended")]
            SampleType::U32,
            #[cfg(feature = "extended")]
            SampleType::U64,
            #[cfg(feature = "extended")]
            SampleType::I8,
            #[cfg(feature = "extended")]
            SampleType::I16,
            #[cfg(feature = "extended")]
            SampleType::I32,
            #[cfg(feature = "extended")]
            SampleType::I64,
            #[cfg(feature = "extended")]
            SampleType::F64,
        ] {
            for channels in [
                crate::Channels::Gray,
                crate::Channels::GrayAlpha,
                crate::Channels::Rgb,
                crate::Channels::Rgba,
            ] {
                for palette in [false, true] {
                    let info = ImageInfo::new(39, 11, channels, sample);
                    let raw: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                        .map(|i| {
                            if palette {
                                (i % info.bytes_per_pixel()) as u8
                            } else {
                                i.wrapping_mul(71).wrapping_add(i / 13) as u8
                            }
                        })
                        .collect();
                    let cfg = EncodeConfig {
                        stripe_rows: Some(3),
                        column_groups: Some(17),
                        palette,
                        effort: 0.0,
                        ..Default::default()
                    };
                    let file =
                        encode(&raw, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
                    let p = parse(&file).unwrap();
                    assert!(p.ngroups() > 1);
                    assert!(p.stripe_count() > 1);
                    let om = OutMap::new(&p.info, &p.layout);
                    let mut y = 0;
                    for si in 0..p.stripe_count() {
                        let records: Vec<Vec<u8>> = p.stripes[si * p.ngroups()..][..p.ngroups()]
                            .iter()
                            .map(|r| r.to_vec())
                            .collect();
                        let identities: Vec<_> = records.iter().map(|r| r.as_ptr()).collect();
                        let mut state = RecordStripe::new(&p, si, records).unwrap();
                        loop {
                            // A fresh destination on every pull: state must not
                            // refer to any earlier output row or caller stack.
                            let mut row = vec![0xA5; info.row_bytes() + 9];
                            if !state.next_row(&p, &om, &mut row, &|| Ok(())).unwrap() {
                                break;
                            }
                            assert_eq!(
                                &row[..info.row_bytes()],
                                &raw[y * info.row_bytes()..][..info.row_bytes()]
                            );
                            assert!(row[info.row_bytes()..].iter().all(|&b| b == 0xA5));
                            y += 1;
                        }
                        assert!(!state.next_row(&p, &om, &mut [], &|| Ok(())).unwrap());
                        let records = state.into_records();
                        assert_eq!(
                            identities,
                            records.iter().map(|r| r.as_ptr()).collect::<Vec<_>>()
                        );
                    }
                    assert_eq!(y, info.height as usize);
                }
            }
        }
    }

    #[test]
    fn retained_record_row_error_is_sticky_after_partial_group_work() {
        let info = ImageInfo::new(39, 7, crate::Channels::Rgb, SampleType::U8);
        let raw = vec![91; info.checked_buffer_len().unwrap()];
        let cfg = EncodeConfig {
            stripe_rows: Some(3),
            column_groups: Some(17),
            ..Default::default()
        };
        let file = encode(&raw, info.row_bytes(), &info, &Metadata::default(), &cfg).unwrap();
        let p = parse(&file).unwrap();
        let om = OutMap::new(&p.info, &p.layout);
        let records = || {
            p.stripes[..p.ngroups()]
                .iter()
                .map(|r| r.to_vec())
                .collect()
        };
        let mut state = RecordStripe::new(&p, 0, records()).unwrap();
        let calls = Cell::new(0);
        let stop = || {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                Err(Error::Cancelled)
            } else {
                Ok(())
            }
        };
        let mut row = vec![0; info.row_bytes()];
        assert_eq!(
            state.next_row(&p, &om, &mut row, &stop),
            Err(Error::Cancelled)
        );
        assert_eq!(calls.get(), 3);
        assert_eq!(
            state.next_row(&p, &om, &mut row, &|| Ok(())),
            Err(Error::Cancelled)
        );
        let mut state = RecordStripe::new(&p, 0, records()).unwrap();
        assert_eq!(
            state.next_row(&p, &om, &mut [], &|| Ok(())),
            Err(Error::InvalidInput("short output row"))
        );
        assert_eq!(
            state.next_row(&p, &om, &mut row, &|| Ok(())),
            Err(Error::InvalidInput("short output row"))
        );
        assert!(matches!(
            RecordStripe::new(&p, p.stripe_count(), records()),
            Err(Error::Corrupt("column group records"))
        ));
        assert!(matches!(
            RecordStripe::new(&p, 0, Vec::new()),
            Err(Error::Corrupt("column group records"))
        ));
    }
}
