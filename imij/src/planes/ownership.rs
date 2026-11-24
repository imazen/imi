//! Validate backing ownership using level/attachment intervals, not a bitmap
//! proportional to every tile in a gigapixel image.
use super::*;
#[derive(Clone, Copy)]
pub(super) struct OwnerSpan {
    pub start: usize,
    pub end: usize,
    pub plane: Option<(usize, usize)>,
}
pub(super) fn validate(
    dir: &Directory,
    mut spans: Vec<OwnerSpan>,
    record_count: usize,
) -> Result<(), Error> {
    spans.sort_unstable_by_key(|s| (s.start, s.end));
    let mut at = 0;
    let mut cursor = 0;
    while at < spans.len() {
        let first = spans[at];
        if first.start > cursor {
            return Err(Error::Corrupt("unreferenced plane record"));
        }
        if first.start < cursor {
            return Err(Error::Corrupt("partial shared color backing"));
        }
        let mut next = at + 1;
        while next < spans.len() && spans[next].start == first.start && spans[next].end == first.end
        {
            next += 1
        }
        if let Some((pi, li)) = first.plane {
            let p = &dir.planes[pi];
            let l = &p.levels[li];
            let mut mask = 0u8;
            for owner in &spans[at..next] {
                let Some((oi, ol)) = owner.plane else {
                    return Err(Error::Corrupt("aliased attachment record"));
                };
                let op = &dir.planes[oi];
                let other = &op.levels[ol];
                let bit = 1 << other.component;
                if mask & bit != 0
                    || p.sample != op.sample
                    || p.group != op.group
                    || p.origin != op.origin
                    || p.cicp != op.cicp
                    || p.icc_profile != op.icc_profile
                    || (
                        l.width,
                        l.height,
                        l.tile_width,
                        l.tile_height,
                        l.coding,
                        l.channels,
                        l.source,
                    ) != (
                        other.width,
                        other.height,
                        other.tile_width,
                        other.tile_height,
                        other.coding,
                        other.channels,
                        other.source,
                    )
                {
                    return Err(Error::Corrupt("incompatible shared plane record"));
                }
                mask |= bit;
            }
            if mask != (1 << l.channels.count()) - 1 {
                return Err(Error::Corrupt("incomplete shared color components"));
            }
        } else if next != at + 1 {
            return Err(Error::Corrupt("aliased attachment record"));
        }
        if spans.get(next).is_some_and(|v| v.start < first.end) {
            return Err(Error::Corrupt(
                if first.plane.is_none() || spans[next].plane.is_none() {
                    "aliased attachment record"
                } else {
                    "partial shared color backing"
                },
            ));
        }
        cursor = first.end;
        at = next;
    }
    if cursor != record_count {
        return Err(Error::Corrupt("unreferenced plane record"));
    }
    Ok(())
}
