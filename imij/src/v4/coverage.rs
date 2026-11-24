//! DC coverage is geometry, independent of numeric model selection.

use alloc::vec::Vec;

use crate::decode_resources::DecodeSession;
use crate::parse::limits::Admission;
use crate::{ByteReader, Error};

pub(crate) enum DcCoverage {
    Uniform(u32),
    /// Exclusive stripe ends. One shared list or one list per column group;
    /// never expanded into an image-wide positional unit map.
    Explicit(Vec<Vec<u32>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub start: u32,
    pub end: u32,
    pub id: u64,
}

impl DcCoverage {
    pub(crate) fn read<C: Fn() -> bool + Sync>(
        bytes: &[u8],
        stripes: u32,
        groups: u32,
        tile_rows: u32,
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        if stripes == 0 || groups == 0 || tile_rows == 0 {
            return Err(Error::Corrupt("V4 DC coverage geometry"));
        }
        session.adapter_work(bytes.len() as u128 + 1)?;
        let mut r = ByteReader::new(bytes);
        let out = match r.varint()? {
            0 => {
                let length = r.var_u32()?;
                if length == 0 {
                    return Err(Error::Corrupt("zero V4 DC coverage span"));
                }
                Self::Uniform(length)
            }
            1 => {
                let n = r.var_u32()?;
                if (n != 1 && n != groups) || u64::from(n) > (r.remaining() / 2) as u64 {
                    return Err(Error::Corrupt("V4 DC coverage list count"));
                }
                let n = usize::try_from(n).map_err(|_| Error::TooLarge)?;
                session.v4_models().hint(
                    n.checked_mul(core::mem::size_of::<Vec<u32>>())
                        .ok_or(Error::TooLarge)?,
                )?;
                let mut lists = Vec::new();
                lists.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
                for _ in 0..n {
                    let count = r.var_u32()?;
                    if count == 0 || count > stripes || u64::from(count) > r.remaining() as u64 {
                        return Err(Error::Corrupt("V4 DC coverage span count"));
                    }
                    let count = usize::try_from(count).map_err(|_| Error::TooLarge)?;
                    session
                        .v4_models()
                        .hint(count.checked_mul(4).ok_or(Error::TooLarge)?)?;
                    let mut ends = Vec::new();
                    ends.try_reserve_exact(count).map_err(|_| Error::TooLarge)?;
                    let mut start = 0u32;
                    for _ in 0..count {
                        session.adapter_work(1)?;
                        let length = r.var_u32()?;
                        let end = start
                            .checked_add(length)
                            .filter(|&end| length != 0 && end <= stripes)
                            .ok_or(Error::Corrupt("V4 DC coverage extent"))?;
                        if start / tile_rows != (end - 1) / tile_rows {
                            return Err(Error::Corrupt("V4 DC span crosses spatial tile"));
                        }
                        ends.push(end);
                        start = end;
                    }
                    if start != stripes {
                        return Err(Error::Corrupt("incomplete V4 DC coverage"));
                    }
                    lists.push(ends);
                }
                Self::Explicit(lists)
            }
            _ => return Err(Error::Unsupported("V4 DC coverage form")),
        };
        if r.remaining() != 0 {
            return Err(Error::Corrupt("V4 DC coverage trailing bytes"));
        }
        Ok(out)
    }

    pub(crate) fn span(
        &self,
        stripe: u32,
        column: u32,
        stripes: u32,
        groups: u32,
        tile_rows: u32,
    ) -> Result<Span, Error> {
        if stripe >= stripes || column >= groups {
            return Err(Error::Corrupt("V4 DC coverage identity"));
        }
        let (start, end) = match self {
            Self::Uniform(length) => {
                let base = stripe / tile_rows * tile_rows;
                let start = base + (stripe - base) / length * length;
                let end = (u64::from(start) + u64::from(*length))
                    .min(u64::from(base) + u64::from(tile_rows))
                    .min(u64::from(stripes)) as u32;
                (start, end)
            }
            Self::Explicit(lists) => {
                let list = &lists[if lists.len() == 1 { 0 } else { column as usize }];
                let at = list.partition_point(|&end| end <= stripe);
                (if at == 0 { 0 } else { list[at - 1] }, list[at])
            }
        };
        Ok(Span {
            start,
            end,
            id: u64::from(start) * u64::from(groups) + u64::from(column),
        })
    }

    pub(crate) fn max_span(&self, stripes: u32, tile_rows: u32) -> u32 {
        match self {
            Self::Uniform(length) => (*length).min(stripes).min(tile_rows),
            Self::Explicit(lists) => lists
                .iter()
                .map(|ends| {
                    let mut start = 0;
                    ends.iter()
                        .map(|&end| {
                            let length = end - start;
                            start = end;
                            length
                        })
                        .max()
                        .unwrap_or(0)
                })
                .max()
                .unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_resources::DecodePolicy;

    #[test]
    fn coverage_clips_uniform_spans_and_explicit_lists_at_spatial_boundaries() {
        let session = DecodeSession::new(DecodePolicy::default());
        for bytes in [&[0, 2][..], &[1, 1, 5, 2, 1, 2, 1, 1][..]] {
            let coverage = DcCoverage::read(bytes, 7, 2, 3, &session).unwrap();
            for (stripe, (start, end)) in [(0, 2), (0, 2), (2, 3), (3, 5), (3, 5), (5, 6), (6, 7)]
                .into_iter()
                .enumerate()
            {
                for column in 0..2 {
                    assert_eq!(
                        coverage.span(stripe as u32, column, 7, 2, 3).unwrap(),
                        Span {
                            start,
                            end,
                            id: u64::from(start) * 2 + u64::from(column)
                        }
                    );
                }
            }
        }
        let coverage =
            DcCoverage::read(&[1, 2, 5, 2, 1, 2, 1, 1, 3, 3, 3, 1], 7, 2, 3, &session).unwrap();
        assert_eq!(
            coverage.span(4, 1, 7, 2, 3).unwrap(),
            Span {
                start: 3,
                end: 6,
                id: 7
            }
        );
        assert_eq!(coverage.max_span(7, 3), 3);
        let huge = DcCoverage::read(&[0, 1], u32::MAX, u32::MAX, u32::MAX, &session).unwrap();
        let at = huge
            .span(u32::MAX - 1, u32::MAX - 1, u32::MAX, u32::MAX, u32::MAX)
            .unwrap();
        assert_eq!((at.start, at.end), (u32::MAX - 1, u32::MAX));
        assert!(at.id > u64::from(u32::MAX));
        for bad in [
            &[0, 0][..],
            &[0, 1, 0][..],
            &[1, 3][..],
            &[1, 1, 1, 7][..],
            &[1, 1, 2, 3, 3][..],
            &[1, 1, 3, 3, 3, 2][..],
            &[1, 1, 2, 4, 3][..],
        ] {
            assert!(DcCoverage::read(bad, 7, 2, 3, &session).is_err(), "{bad:?}");
        }
        let denied = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(0));
        assert!(matches!(
            DcCoverage::read(&[1, 1, 3, 3, 3, 1], 7, 2, 3, &denied),
            Err(Error::TooLarge)
        ));
    }
}
