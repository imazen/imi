//! Consecutive AC-stripe coverage. Storage follows spans, never ID magnitude.
use super::*;

pub(super) const HEAD: u64 = crate::registry::HEAD_DC_COVERAGE;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Span {
    pub start: usize,
    pub end: usize,
    /// DC record index, ordered by start stripe then column group.
    pub unit: usize,
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn coverage_rejects_zero_gaps_overcoverage_and_unbounded_counts() {
        let parse = |b: &[u8]| Coverage::read(b, 6, 3, 2, 12, &crate::parse::limits::Unrestricted);
        for b in [
            &[1, 1, 3, 1, 3, 2][..],
            &[1, 3, 2, 2, 4, 3, 1, 3, 2, 1, 6][..],
        ] {
            let c = parse(b).unwrap();
            for group in 0..3 {
                for stripe in 0..6 {
                    let span = c.span(stripe, group);
                    assert!(span.start <= stripe && stripe < span.end);
                    let (a, b, g) = c.rectangles[span.unit];
                    assert_eq!((a, b, g), (span.start * 2, span.end * 2, group));
                }
            }
        }
        for b in [
            &[1, 1, 1, 0][..],
            &[1, 1, 1, 7][..],
            &[1, 1, 2, 2, 2][..],
            &[1, 2, 1, 6][..],
            &[1, 1, 127][..],
            &[1, 1, 1, 6, 0][..],
        ] {
            assert!(parse(b).is_err());
        }
    }

    #[test]
    fn nonuniform_columns_preserve_pixels_whole_rows_and_indexed_regions() {
        let (w, h) = (131, 97);
        let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
        let px = super::super::tests::test_image(w, h, 37);
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for defaults in [false, true] {
                let mut cfg = LossyConfig::new(85.0).with_stripe_block_rows(2);
                cfg.column_groups = Some(64);
                cfg.entropy = entropy;
                cfg.coding_defaults = defaults;
                let baseline =
                    encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
                let expected = crate::decode(&baseline).unwrap().1;
                cfg.dc_coverage = Some(vec![vec![1, 3, 2, 1], vec![2, 1, 4], vec![3, 4]]);
                let file =
                    encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
                for threads in [1, 2, 4, 8] {
                    let mut out = vec![0; expected.len()];
                    #[cfg(feature = "std")]
                    crate::decode_into_threads(&file, &mut out, w as usize * 3, threads).unwrap();
                    #[cfg(not(feature = "std"))]
                    {
                        let _ = threads;
                        crate::decode_into(&file, &mut out, w as usize * 3).unwrap();
                    }
                    assert_eq!(out, expected);
                }
                #[cfg(feature = "std")]
                {
                    let mut rows = Vec::new();
                    crate::decode_reader(file.as_slice(), &crate::Limits::NONE, |_, _, row| {
                        rows.extend_from_slice(row);
                        Ok(())
                    })
                    .unwrap();
                    assert_eq!(rows, expected);
                    for (x, y, rw, rh) in [(0, 19, 131, 57), (67, 33, 61, 47), (127, 89, 4, 8)] {
                        let rect = crate::planes::Rect::new(x, y, rw, rh);
                        let mut out = vec![0; rw as usize * rh as usize * 3];
                        crate::region::decode_region_threads(
                            &file,
                            rect,
                            &mut out,
                            rw as usize * 3,
                            4,
                        )
                        .unwrap();
                        for row in 0..rh as usize {
                            assert_eq!(
                                &out[row * rw as usize * 3..][..rw as usize * 3],
                                &expected[((y as usize + row) * w as usize + x as usize) * 3..]
                                    [..rw as usize * 3]
                            );
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Coverage {
    lists: Vec<Vec<Span>>,
    pub rectangles: alloc::sync::Arc<Vec<(usize, usize, usize)>>,
    sb: usize,
}

impl Coverage {
    pub fn read(
        bytes: &[u8],
        stripes: usize,
        groups: usize,
        sb: usize,
        bh: usize,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Self, Error> {
        let mut r = ByteReader::new(bytes);
        if r.varint()? != 1 {
            return Err(Error::Unsupported("DC coverage version"));
        }
        let n = r.var_usize()?;
        if n != 1 && n != groups {
            return Err(Error::Corrupt("DC coverage columns"));
        }
        if n > r.remaining() / 2 {
            return Err(Error::Corrupt("DC coverage lists"));
        }
        admission.hint(
            n.checked_mul(core::mem::size_of::<Vec<Span>>())
                .ok_or(Error::TooLarge)?,
        )?;
        admission.wire_extra_bytes((n as u128) * core::mem::size_of::<Vec<Span>>() as u128)?;
        let mut lists = Vec::new();
        lists.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        let mut total = 0usize;
        for _ in 0..n {
            let count = r.var_usize()?;
            if count == 0 || count > stripes || count > r.remaining() {
                return Err(Error::Corrupt("DC coverage count"));
            }
            total = total.checked_add(count).ok_or(Error::TooLarge)?;
            admission.hint(
                count
                    .checked_mul(core::mem::size_of::<Span>())
                    .ok_or(Error::TooLarge)?,
            )?;
            admission.wire_extra_bytes((count as u128) * core::mem::size_of::<Span>() as u128)?;
            let mut spans = Vec::new();
            spans
                .try_reserve_exact(count)
                .map_err(|_| Error::TooLarge)?;
            let mut start = 0usize;
            for _ in 0..count {
                admission.check()?;
                let count = r.var_usize()?;
                let end = start
                    .checked_add(count)
                    .filter(|&end| count != 0 && end <= stripes)
                    .ok_or(Error::Corrupt("DC coverage extent"))?;
                spans.push(Span {
                    start,
                    end,
                    unit: 0,
                });
                start = end;
            }
            if start != stripes {
                return Err(Error::Corrupt("incomplete DC coverage"));
            }
            lists.push(spans);
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("DC coverage tail"));
        }
        let count = if n == 1 {
            total.checked_mul(groups).ok_or(Error::TooLarge)?
        } else {
            total
        };
        admission.hint(
            count
                .checked_mul(2 * core::mem::size_of::<(usize, usize, usize)>())
                .ok_or(Error::TooLarge)?,
        )?;
        admission.wire_extra_bytes(
            (count as u128) * 2 * core::mem::size_of::<(usize, usize, usize)>() as u128,
        )?;
        let mut order = Vec::new();
        order
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        for group in 0..groups {
            for (i, span) in lists[if n == 1 { 0 } else { group }].iter().enumerate() {
                order.push((span.start, group, i));
            }
        }
        order.sort_unstable();
        let mut rectangles = Vec::new();
        rectangles
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        for (unit, &(start, group, i)) in order.iter().enumerate() {
            let span = &mut lists[if n == 1 { 0 } else { group }][i];
            // Shared boundaries derive group-specific indices without duplicating lists.
            if n != 1 || group == 0 {
                span.unit = unit;
            }
            let y = start.checked_mul(sb).ok_or(Error::TooLarge)?;
            let end = span.end.checked_mul(sb).ok_or(Error::TooLarge)?.min(bh);
            if y >= end {
                return Err(Error::Corrupt("DC coverage rectangle"));
            }
            rectangles.push((y, end, group));
        }
        Ok(Self {
            lists,
            rectangles: alloc::sync::Arc::new(rectangles),
            sb,
        })
    }

    pub fn span(&self, stripe: usize, group: usize) -> Span {
        let shared = self.lists.len() == 1;
        let list = &self.lists[if shared { 0 } else { group }];
        let i = list.partition_point(|s| s.end <= stripe);
        let mut span = list[i];
        if shared {
            span.unit += group;
        }
        span
    }

    pub fn before(&self, stripe: usize, group: usize) -> usize {
        self.rectangles.partition_point(|&(start, _, g)| {
            let s = start; // rectangles use block rows; callers use before_blocks.
            (s, g) < (stripe * self.sb, group)
        })
    }

    #[cfg(feature = "encode")]
    pub fn write(lists: &[Vec<u32>]) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, 1);
        put_varint(&mut out, lists.len() as u64);
        for list in lists {
            put_varint(&mut out, list.len() as u64);
            for &count in list {
                put_varint(&mut out, u64::from(count));
            }
        }
        out
    }
}
