//! V4 settings resolution; entropy state and model lifetimes are separate.

use alloc::vec::Vec;

use crate::decode_resources::{DecodeSession, V4Working};
use crate::parse::limits::Admission;
use crate::{ByteReader, Error, Feature, Features};

pub(crate) const RANGE_PARTITIONS: u8 = 3;
pub(crate) const JOINT_SETTINGS: u8 = 4;

/// Scratch bytes and work charged for the maps outside the ordinary settings
/// walk. The read and reconstruction paths use the same geometry.
pub(crate) fn map_budget(bytes: &[u8], rows: u32, columns: u32) -> (u128, u128) {
    let cells = u128::from(rows.div_ceil(2)) * u128::from(columns.div_ceil(2));
    let scratch = match bytes.first().copied() {
        Some(RANGE_PARTITIONS) => cells,
        Some(JOINT_SETTINGS) => cells + u128::from(rows) * u128::from(columns),
        _ => return (0, 0),
    };
    (scratch, 8 * scratch + bytes.len() as u128)
}

/// Resolved settings for one 2×2-parent transform cell, in raster order.
/// Spatial tiles are larger storage extents and do not change this state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellSettings {
    partition: u8,
    plans: [u8; 12],
    ac: [u8; 4],
}

impl CellSettings {
    /// Existing partition symbol, 0..7.
    pub const fn partition(self) -> u8 {
        self.partition
    }

    /// Parent-major, then color-plane order. 0 is ordinary 8×8, 1 is the
    /// existing four-4×4 path, and 2+layout names the channel-layout path.
    pub const fn plane_plans(self) -> [u8; 12] {
        self.plans
    }

    /// Parent AC codes. Zero inherits numeric AQ (or 64 without AQ); positive
    /// codes replace it. Exact/keep bypass is evaluated before applying these.
    pub const fn ac_codes(self) -> [u8; 4] {
        self.ac
    }

    fn apply(mut self, delta: Delta) -> Self {
        if let Some(v) = delta.partition {
            self.partition = v;
        }
        for (i, plan) in self.plans.iter_mut().enumerate() {
            if delta.edits & (1 << i) != 0 {
                *plan = delta.plans[i];
            }
        }
        if let Some(v) = delta.ac {
            self.ac = v;
        }
        self
    }

    fn validate(self, present: u8, declared: Features) -> Result<(), Error> {
        // Footprints, independently of coefficient/merge-role representation.
        const MERGES: [&[u8]; 8] = [&[], &[15], &[3], &[12], &[3, 12], &[5], &[10], &[5, 10]];
        if self.partition != 0 && !declared.contains(Feature::Superblocks) {
            return Err(Error::Corrupt("V4 merge without capability"));
        }
        for &mask in MERGES[self.partition as usize] {
            if mask & present != mask {
                return Err(Error::Corrupt("V4 merge crosses group edge"));
            }
            for p in 0..4 {
                if mask & (1 << p) != 0 && self.plans[p * 3..p * 3 + 3] != [0; 3] {
                    return Err(Error::Corrupt("V4 merge with plane plan"));
                }
            }
        }
        for p in 0..4 {
            if present & (1 << p) == 0 {
                continue;
            }
            for &plan in &self.plans[p * 3..p * 3 + 3] {
                let feature = match plan {
                    0 => continue,
                    1 => Feature::SmallTransforms,
                    _ => Feature::ChannelLayouts,
                };
                if !declared.contains(feature) {
                    return Err(Error::Corrupt("V4 plane plan without capability"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Delta {
    partition: Option<u8>,
    edits: u16,
    plans: [u8; 12],
    ac: Option<[u8; 4]>,
}

impl Delta {
    fn read(r: &mut ByteReader<'_>) -> Result<Self, Error> {
        let mask = crate::trace::tf!("settings fields", r, r.varint()?);
        if mask & !7 != 0 {
            return Err(Error::Unsupported("V4 settings field"));
        }
        let mut out = Self::default();
        if mask & 1 != 0 {
            let value = crate::trace::tf!("partition", r, r.varint()?);
            if value > 7 {
                return Err(Error::Corrupt("V4 partition"));
            }
            out.partition = Some(value as u8);
        }
        if mask & 2 != 0 {
            let edits = crate::trace::tf!("plane edits", r, r.varint()?);
            if edits > 4095 {
                return Err(Error::Corrupt("V4 plane-plan edit mask"));
            }
            out.edits = edits as u16;
            for (i, plan) in out.plans.iter_mut().enumerate() {
                if out.edits & (1 << i) != 0 {
                    let value = crate::trace::tf!("plane plan", r, r.varint()?);
                    if value > 13 || value == 10 {
                        return Err(Error::Unsupported("V4 plane plan"));
                    }
                    *plan = value as u8;
                }
            }
        }
        if mask & 4 != 0 {
            out.ac = Some(match crate::trace::tf!("AC scale", r, r.varint()?) {
                v @ 0..=255 => [v as u8; 4],
                256 => {
                    let mut values = [0; 4];
                    for value in &mut values {
                        *value = u8::try_from(crate::trace::tf!("parent AC scale", r, r.varint()?))
                            .map_err(|_| Error::Corrupt("V4 parent AC multiplier"))?;
                    }
                    values
                }
                _ => return Err(Error::Unsupported("V4 AC multiplier form")),
            });
        }
        Ok(out)
    }
}

/// Immutable image settings and reusable, nonrecursive deltas from V4 setup.
/// The owning driver admits the framed setup bytes. This component charges
/// retained capacity and work; it does not parse a lossy file or render pixels.
pub struct LossySettings {
    sets: Vec<(u64, Delta)>,
    image: CellSettings,
    _charge: crate::decode_resources::V4Retained,
}

impl LossySettings {
    pub fn read<C: Fn() -> bool + Sync>(
        bytes: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        let scope = session.v4_model_scope();
        session.adapter_work(bytes.len() as u128 + 1)?;
        let mut r = ByteReader::new(bytes);
        let count = crate::trace::tf!("reusable settings", r, r.var_usize()?);
        // Every set needs an ID and a mask, followed by the image delta.
        if r.remaining() == 0 || count > (r.remaining() - 1) / 2 {
            return Err(Error::Corrupt("V4 settings set count"));
        }
        let capacity = count
            .checked_mul(core::mem::size_of::<(u64, Delta)>())
            .ok_or(Error::TooLarge)?;
        session.v4_models().hint(capacity)?;
        let mut sets = Vec::new();
        sets.try_reserve_exact(count).map_err(|_| Error::TooLarge)?;
        let mut previous = 0;
        for _ in 0..count {
            session.adapter_work(1)?;
            let id = crate::trace::tf!("settings ID", r, r.varint()?);
            if id <= previous {
                return Err(Error::Corrupt("V4 settings set IDs"));
            }
            previous = id;
            sets.push((id, Delta::read(&mut r)?));
        }
        let image = CellSettings::default().apply(Delta::read(&mut r)?);
        if r.remaining() != 0 {
            return Err(Error::Corrupt("V4 image settings trailing bytes"));
        }
        Ok(Self {
            sets,
            image,
            _charge: scope.commit()?,
        })
    }

    pub const fn image(&self) -> CellSettings {
        self.image
    }

    fn selection(
        &self,
        r: &mut ByteReader<'_>,
        parent: CellSettings,
    ) -> Result<CellSettings, Error> {
        match r.varint()? {
            0 => Ok(parent),
            1 => Ok(parent.apply(Delta::read(r)?)),
            2 => {
                let id = r.varint()?;
                let i = self
                    .sets
                    .binary_search_by_key(&id, |&(id, _)| id)
                    .map_err(|_| Error::Corrupt("V4 undefined settings set"))?;
                Ok(parent.apply(self.sets[i].1))
            }
            _ => Err(Error::Unsupported("V4 settings selection")),
        }
    }

    /// Resolve a length-delimited AC unit settings body over parent geometry.
    /// Callbacks receive transform-cell coordinates and never inherit a prior
    /// cell's override. The complete prefix, capability gates and edge merges
    /// validate before the first callback. Cancellation/callback errors may
    /// stop subsequent delivery. Range-coded partitions retain one byte per
    /// transform cell while the group is being visited.
    ///
    /// The owning driver validates exact/keep conflicts after decoding DC and
    /// before coefficient reconstruction, and charges framed input separately.
    pub fn visit_unit<C: Fn() -> bool + Sync>(
        &self,
        bytes: &[u8],
        parent_rows: u32,
        parent_columns: u32,
        declared: Features,
        session: &DecodeSession<C>,
        cell: impl FnMut(u32, u32, CellSettings) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.prepare_unit(bytes, (parent_rows, parent_columns), declared, session)?
            .visit(cell)
    }

    pub(crate) fn prepare_unit<'s, 'a, C: Fn() -> bool + Sync>(
        &'a self,
        bytes: &'a [u8],
        (parent_rows, parent_columns): (u32, u32),
        declared: Features,
        session: &'s DecodeSession<C>,
    ) -> Result<PreparedUnit<'s, 'a, C>, Error> {
        if parent_rows == 0 || parent_columns == 0 {
            return Err(Error::Corrupt("V4 settings group geometry"));
        }
        let columns = u64::from(parent_columns.div_ceil(2));
        let n = u64::from(parent_rows.div_ceil(2)) * columns;
        session.adapter_work(2 * (u128::from(n) + bytes.len() as u128 + 1))?;
        let mut reader = ByteReader::new(bytes);
        if bytes.first() == Some(&JOINT_SETTINGS) {
            crate::trace::tf!("joint settings selector", reader, reader.u8()?);
            let range = &bytes[reader.pos..];
            if !declared.contains(Feature::SmallTransforms) || self.image != CellSettings::default()
            {
                return Err(Error::Corrupt(
                    "V4 joint settings image defaults or capability",
                ));
            }
            let parents = u128::from(parent_rows) * u128::from(parent_columns);
            if range.len() < 4
                || range.len() as u128
                    > crate::lossy::joint_settings::max_bytes(u128::from(n), parents)
            {
                return Err(Error::Corrupt("V4 joint settings range length"));
            }
            let (scratch, work) = map_budget(bytes, parent_rows, parent_columns);
            let charge = session.v4_working(scratch)?;
            session.adapter_work(work)?;
            let total = usize::try_from(scratch).map_err(|_| Error::TooLarge)?;
            let mut maps =
                bytemuck::allocation::try_zeroed_vec(total).map_err(|_| Error::TooLarge)?;
            let (partitions, flags) = maps.split_at_mut(n as usize);
            crate::lossy::joint_settings::decode(
                range,
                parent_rows as usize,
                parent_columns as usize,
                declared.contains(Feature::Superblocks),
                partitions,
                flags,
                || session.adapter_work(0),
            )?;
            return Ok(PreparedUnit {
                settings: self,
                group: self.image,
                overrides: &[],
                count: 0,
                columns,
                n,
                parent_dimensions: (parent_rows, parent_columns),
                maps,
                session,
                _charge: Some(charge),
            });
        }
        let dense = bytes.first() == Some(&RANGE_PARTITIONS);
        if dense {
            crate::trace::tf!("range partition selector", reader, reader.u8()?);
        }
        let group = if bytes.is_empty() {
            self.image
        } else {
            self.selection(&mut reader, self.image)?
        };
        let mut partitions = Vec::new();
        let charge = if dense {
            if !declared.contains(Feature::Superblocks) {
                return Err(Error::Corrupt("V4 range map without capability"));
            }
            let length = crate::trace::tf!("partition range bytes", reader, reader.varint()?);
            let map = crate::trace::tf!("partition range map", reader, reader.bytes_len(length)?);
            if length < 4 || u128::from(length) > 4 * u128::from(n) + 4 {
                return Err(Error::Corrupt("V4 partition range length"));
            }
            session.adapter_work(8 * u128::from(n) + u128::from(length))?;
            let charge = session.v4_working(u128::from(n))?;
            let cells = usize::try_from(n).map_err(|_| Error::TooLarge)?;
            partitions
                .try_reserve_exact(cells)
                .map_err(|_| Error::TooLarge)?;
            partitions.resize(cells, 0);
            crate::lossy::decode_v4_partitions(
                map,
                parent_rows as usize,
                parent_columns as usize,
                &mut partitions,
                || session.adapter_work(0),
            )?;
            Some(charge)
        } else {
            None
        };
        let count = if bytes.is_empty() {
            0
        } else {
            reader.varint()?
        };
        if count > n || count > (reader.remaining() / 2) as u64 {
            return Err(Error::Corrupt("V4 settings override count"));
        }
        let unit = PreparedUnit {
            settings: self,
            group,
            overrides: &bytes[reader.pos..],
            count,
            columns,
            n,
            parent_dimensions: (parent_rows, parent_columns),
            maps: partitions,
            session,
            _charge: charge,
        };
        if !unit.inherits_default() {
            unit.visit(|y, x, state| {
                let right = x * 2 + 1 < parent_columns;
                let bottom = y * 2 + 1 < parent_rows;
                let present = 1
                    | (u8::from(right) << 1)
                    | (u8::from(bottom) << 2)
                    | (u8::from(right && bottom) << 3);
                state.validate(present, declared)
            })?;
        }
        Ok(unit)
    }
}

pub(crate) struct PreparedUnit<'s, 'a, C> {
    settings: &'a LossySettings,
    group: CellSettings,
    overrides: &'a [u8],
    count: u64,
    columns: u64,
    n: u64,
    parent_dimensions: (u32, u32),
    // Partition bytes, followed in the joint form by one mask per parent.
    maps: Vec<u8>,
    session: &'s DecodeSession<C>,
    _charge: Option<V4Working<'s, C>>,
}

impl<C: Fn() -> bool + Sync> PreparedUnit<'_, '_, C> {
    /// Ordinary transforms and numeric AQ are valid for every edge shape.
    pub(crate) fn inherits_default(&self) -> bool {
        self.overrides.is_empty() && self.maps.is_empty() && self.group == CellSettings::default()
    }

    pub(crate) fn is_joint(&self) -> bool {
        self.maps.len() as u64 > self.n
    }

    /// Direct access to `(partitions, small_flags)` when this unit uses joint
    /// range-coded settings (no per-cell overrides, default group settings).
    pub(crate) fn joint_maps(&self) -> Option<(&[u8], &[u8])> {
        self.is_joint().then(|| self.maps.split_at(self.n as usize))
    }

    /// Direct access to dense `partitions` when this unit has no overrides,
    /// no small flags, and default group settings.
    pub(crate) fn range_partitions(&self) -> Option<&[u8]> {
        (!self.is_joint()
            && !self.maps.is_empty()
            && self.overrides.is_empty()
            && self.group == CellSettings::default())
        .then_some(self.maps.as_slice())
    }

    fn visit_joint(
        &self,
        mut cell: impl FnMut(u32, u32, CellSettings) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let (rows, columns) = self.parent_dimensions;
        let (partitions, flags) = self.maps.split_at(self.n as usize);
        for (i, &partition) in partitions.iter().enumerate() {
            if i.is_multiple_of(32) {
                self.session.adapter_work(0)?;
            }
            let y = i / self.columns as usize;
            let x = i % self.columns as usize;
            let mut state = CellSettings {
                partition,
                ..CellSettings::default()
            };
            for dy in 0..2.min(rows as usize - 2 * y) {
                for dx in 0..2.min(columns as usize - 2 * x) {
                    let mask = flags[(2 * y + dy) * columns as usize + 2 * x + dx];
                    let start = 3 * (2 * dy + dx);
                    state.plans[start..][..3].copy_from_slice(&[
                        mask & 1,
                        mask >> 1 & 1,
                        mask >> 2,
                    ]);
                }
            }
            cell(y as u32, x as u32, state)?;
        }
        Ok(())
    }

    pub(crate) fn visit(
        &self,
        mut cell: impl FnMut(u32, u32, CellSettings) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.is_joint() {
            return self.visit_joint(cell);
        }
        let mut r = ByteReader::new(self.overrides);
        let base = |i: u64| {
            let mut state = self.group;
            if !self.maps.is_empty() {
                state.partition = self.maps[i as usize];
            }
            state
        };
        let mut next = 0u64;
        let mut emit = |i: u64, state| {
            if i.is_multiple_of(32) {
                self.session.adapter_work(0)?;
            }
            cell((i / self.columns) as u32, (i % self.columns) as u32, state)
        };
        for _ in 0..self.count {
            let at = next
                .checked_add(r.varint()?)
                .filter(|&v| v < self.n)
                .ok_or(Error::Corrupt("V4 settings override position"))?;
            for i in next..at {
                emit(i, base(i))?;
            }
            emit(at, self.settings.selection(&mut r, base(at))?)?;
            next = at + 1;
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("V4 unit settings trailing bytes"));
        }
        for i in next..self.n {
            emit(i, base(i))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_resources::DecodePolicy;
    use crate::put_varint;
    use alloc::vec;

    #[test]
    fn joint_settings_validate_literal_and_reject_before_delivery() {
        let session = DecodeSession::new(DecodePolicy::default());
        let settings = LossySettings::read(&[0, 0], &session).unwrap();
        let small = Features::EMPTY.with(Feature::SmallTransforms);
        let both = small.with(Feature::Superblocks);
        let valid = [JOINT_SETTINGS, 0x5f, 0xff, 0xf8, 0];
        let mut delivered = 0;
        settings
            .visit_unit(&valid, 1, 1, small, &session, |y, x, state| {
                assert_eq!((y, x), (0, 0));
                assert_eq!(state.partition(), 0);
                assert_eq!(state.plane_plans(), [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                assert_eq!(state.ac_codes(), [0; 4]);
                delivered += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(delivered, 1);
        let reject = |bytes: &[u8], caps| {
            assert!(matches!(
                settings.visit_unit(bytes, 1, 1, caps, &session, |_, _, _| panic!(
                    "invalid settings delivered"
                )),
                Err(Error::Corrupt(_)),
            ));
        };
        for end in 1..valid.len() {
            reject(&valid[..end], both);
        }
        reject(&[JOINT_SETTINGS, 0x5f, 0xff, 0xf8, 0, 0], both);
        reject(&[JOINT_SETTINGS, 0xbf, 0xff, 0xf8, 0], both);
        reject(&[JOINT_SETTINGS, 0xaf, 0xff, 0xf8, 0], both);
        reject(&[JOINT_SETTINGS, 0, 0, 0, 0], both);
        reject(&valid, Features::EMPTY);
        let denied = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(1));
        assert!(matches!(
            settings.visit_unit(&valid, 1, 1, small, &denied, |_, _, _| panic!(
                "unadmitted payload delivered"
            )),
            Err(Error::TooLarge),
        ));
    }

    fn fields(values: &[u64]) -> Vec<u8> {
        let mut out = Vec::new();
        for &v in values {
            put_varint(&mut out, v);
        }
        out
    }

    fn features() -> Features {
        Features::EMPTY
            .with(Feature::Superblocks)
            .with(Feature::SmallTransforms)
            .with(Feature::ChannelLayouts)
    }

    fn states(settings: &LossySettings, bytes: &[u8], rows: u32, cols: u32) -> Vec<CellSettings> {
        let session = DecodeSession::new(DecodePolicy::default());
        let mut out = Vec::new();
        settings
            .visit_unit(bytes, rows, cols, features(), &session, |y, x, state| {
                assert_eq!(
                    u64::from(y) * u64::from(cols.div_ceil(2)) + u64::from(x),
                    out.len() as u64
                );
                out.push(state);
                Ok(())
            })
            .unwrap();
        out
    }

    #[test]
    fn settings_inherit_sparse_edits_without_carry_or_recursive_ids() {
        // Set MAX edits TL plane 1 and BR plane 2, and resets all AC codes.
        // Image independently edits TL plane 0 and sets numeric AC replacement.
        let bytes = fields(&[1, u64::MAX, 6, (1 << 1) | (1 << 11), 1, 13, 0, 6, 1, 3, 32]);
        let session = DecodeSession::new(DecodePolicy::default());
        let settings = LossySettings::read(&bytes, &session).unwrap();
        assert_eq!(
            settings.image().plane_plans(),
            [3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(settings.image().ac_codes(), [32; 4]);

        // Group edits TR plane 0, retaining the image's TL edit. Cell 1 uses
        // the reusable delta; cell 3 inherits the group. Cells 0 and 2 must
        // retain the group, including cell 2 after the preceding override.
        let unit = fields(&[1, 2, 1 << 3, 1, 2, 1, 2, u64::MAX, 1, 0]);
        let got = states(&settings, &unit, 4, 4);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].plane_plans(), [3, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(got[1].plane_plans(), [3, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 13]);
        assert_eq!(got[1].ac_codes(), [0; 4]);
        assert_eq!(got[2], got[0]);
        assert_eq!(got[3], got[0]);
        assert_eq!(states(&settings, &[], 2, 2), vec![settings.image()]);
    }

    #[test]
    fn settings_partition_edges_and_plane_paths_follow_existing_footprints() {
        let session = DecodeSession::new(DecodePolicy::default());
        let settings = LossySettings::read(&[0, 0], &session).unwrap();
        // Legal partition symbols for full, horizontal-only, vertical-only,
        // and singleton edge cells, independently from decoder role bytes.
        for (rows, cols, allowed) in [(2, 2, 255u8), (1, 2, 5), (2, 1, 33), (1, 1, 1)] {
            for partition in 0..8 {
                let unit = fields(&[1, 1, partition, 0]);
                let mut delivered = 0;
                let r = settings.visit_unit(&unit, rows, cols, features(), &session, |_, _, _| {
                    delivered += 1;
                    Ok(())
                });
                assert_eq!(
                    r.is_ok(),
                    allowed & (1 << partition) != 0,
                    "{rows} {cols} {partition}"
                );
                assert_eq!(delivered, usize::from(r.is_ok()));
            }
        }
        for plan in 0..=14 {
            let unit = fields(&[1, 2, 1, plan, 0]);
            let result = settings.visit_unit(&unit, 2, 2, features(), &session, |_, _, _| Ok(()));
            assert_eq!(result.is_ok(), plan != 10 && plan < 14, "plan {plan}");
            if plan != 0 {
                assert!(
                    settings
                        .visit_unit(&unit, 2, 2, Features::EMPTY, &session, |_, _, _| Ok(()))
                        .is_err()
                );
            }
        }
        // Absent edge parents create no samples or feature requirement.
        let absent = fields(&[1, 2, 1 << 11, 1, 0]);
        settings
            .visit_unit(&absent, 1, 1, Features::EMPTY, &session, |_, _, _| Ok(()))
            .unwrap();
        let conflict = fields(&[1, 3, 1, 1, 1, 0]);
        assert!(
            settings
                .visit_unit(&conflict, 2, 2, features(), &session, |_, _, _| Ok(()))
                .is_err()
        );
    }

    #[test]
    fn settings_validate_complete_prefix_before_delivery_and_bound_work() {
        let session = DecodeSession::new(DecodePolicy::default());
        let settings = LossySettings::read(&[0, 0], &session).unwrap();
        let valid = fields(&[1, 4, 256, 0, 32, 64, 255, 1, 3, 1, 4, 0]);
        let got = states(&settings, &valid, 4, 4);
        assert_eq!(got[0].ac_codes(), [0, 32, 64, 255]);
        assert_eq!(got[3].ac_codes(), [0; 4]);
        for end in 1..valid.len() {
            let mut calls = 0;
            assert!(
                settings
                    .visit_unit(&valid[..end], 4, 4, features(), &session, |_, _, _| {
                        calls += 1;
                        Ok(())
                    })
                    .is_err(),
                "prefix {end}"
            );
            assert_eq!(calls, 0);
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(
            settings
                .visit_unit(&trailing, 4, 4, features(), &session, |_, _, _| panic!(
                    "premature delivery"
                ))
                .is_err()
        );
        // Inherited defaults with no overrides must still consume the body.
        assert!(matches!(
            settings.prepare_unit(&[0, 0, 0], (3, 3), features(), &session),
            Err(Error::Corrupt("V4 unit settings trailing bytes"))
        ));
        let unknown = fields(&[2, u64::MAX, 0]);
        assert!(
            settings
                .visit_unit(&unknown, 2, 2, features(), &session, |_, _, _| panic!(
                    "unknown set"
                ))
                .is_err()
        );
        let limited = DecodeSession::new(DecodePolicy::default().with_max_work_units(16));
        assert!(matches!(
            settings.visit_unit(
                &[],
                u32::MAX,
                u32::MAX,
                features(),
                &limited,
                |_, _, _| panic!("unadmitted work")
            ),
            Err(Error::TooLarge)
        ));
        let canceled = DecodeSession::with_cancel(DecodePolicy::default(), || true);
        assert!(
            settings
                .visit_unit(&[], 2, 2, features(), &canceled, |_, _, _| panic!(
                    "canceled"
                ))
                .is_err()
        );
        let denied = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(0));
        assert!(matches!(
            LossySettings::read(&fields(&[1, u64::MAX, 0, 0]), &denied),
            Err(Error::TooLarge)
        ));
        for bad in [
            fields(&[1, 0, 0, 0]),
            fields(&[2, 1, 0, 1, 0, 0]),
            fields(&[0, 8]),
            fields(&[0, 4, 257]),
        ] {
            assert!(LossySettings::read(&bad, &session).is_err());
        }
    }
}
