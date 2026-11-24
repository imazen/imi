use crate::Error;

/// Physical ordering of spatial tiles. Groups inside a tile are row-major.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TileOrder {
    /// Rows of spatial tiles, each containing its own rows of groups.
    #[default]
    Raster,
    /// Morton order with empty coordinates omitted on rectangular grids.
    Morton,
}

impl TryFrom<u64> for TileOrder {
    type Error = Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Raster),
            1 => Ok(Self::Morton),
            _ => Err(Error::Unsupported("V4 spatial tile order")),
        }
    }
}

impl TileOrder {
    /// Canonical unsigned-varint value in the V4 layout declaration.
    pub const fn wire_value(self) -> u64 {
        match self {
            Self::Raster => 0,
            Self::Morton => 1,
        }
    }
}

/// A spatial tile's rectangle in coding-group coordinates, with clipped edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpatialTile {
    /// Geometry-derived row-major identity, independent of disk order.
    pub id: u64,
    /// First coding-group column.
    pub group_x: u32,
    /// First coding-group row.
    pub group_y: u32,
    /// Present coding-group columns, excluding absent edge cells.
    pub groups_wide: u32,
    /// Present coding-group rows, excluding absent edge cells.
    pub groups_high: u32,
}

impl SpatialTile {
    /// Number of coding groups in this tile.
    pub const fn group_count(self) -> u64 {
        self.groups_wide as u64 * self.groups_high as u64
    }
}

/// One independently schedulable group. Identity does not depend on tile order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    /// Row-major identity in the complete coding-group grid.
    pub id: u64,
    /// Column in the complete coding-group grid.
    pub x: u32,
    /// Row in the complete coding-group grid.
    pub y: u32,
}

/// Checked V4 spatial layout, in coding groups rather than pixel dimensions.
///
/// Counts and identities use u64; no allocation is sized by an ID. Morton
/// lookup visits at most 32 quadtree levels, including for extremely narrow
/// grids. It never enumerates a padded bounding square or materializes a
/// permutation. Decoder pixel/work limits are a separate admission step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileLayout {
    groups_x: u32,
    groups_y: u32,
    tile_cols: u32,
    tile_rows: u32,
    tiles_x: u32,
    tiles_y: u32,
    order: TileOrder,
}

impl TileLayout {
    /// Validate a nonempty group grid and positive spatial tile dimensions.
    pub fn new(
        groups_x: u32,
        groups_y: u32,
        tile_cols: u32,
        tile_rows: u32,
        order: TileOrder,
    ) -> Result<Self, Error> {
        if groups_x == 0 || groups_y == 0 || tile_cols == 0 || tile_rows == 0 {
            return Err(Error::Corrupt("zero V4 tile/group geometry"));
        }
        Ok(Self {
            groups_x,
            groups_y,
            tile_cols,
            tile_rows,
            tiles_x: groups_x.div_ceil(tile_cols),
            tiles_y: groups_y.div_ceil(tile_rows),
            order,
        })
    }

    /// Complete coding-group grid dimensions.
    pub const fn group_grid(self) -> (u32, u32) {
        (self.groups_x, self.groups_y)
    }

    /// Spatial tile-grid dimensions.
    pub const fn tile_grid(self) -> (u32, u32) {
        (self.tiles_x, self.tiles_y)
    }

    /// Configured tile size in groups, before clipping the right/bottom edges.
    pub const fn tile_shape(self) -> (u32, u32) {
        (self.tile_cols, self.tile_rows)
    }

    /// Physical tile traversal order.
    pub const fn order(self) -> TileOrder {
        self.order
    }

    /// Number of spatial tiles, without a count-dependent allocation.
    pub const fn tile_count(self) -> u64 {
        self.tiles_x as u64 * self.tiles_y as u64
    }

    /// Number of coding groups, without a count-dependent allocation.
    pub const fn group_count(self) -> u64 {
        self.groups_x as u64 * self.groups_y as u64
    }

    /// Tile by geometry identity, independent of physical order.
    pub fn tile(self, id: u64) -> Result<SpatialTile, Error> {
        if id >= self.tile_count() {
            return Err(Error::Corrupt("V4 spatial tile identity"));
        }
        let x = id % u64::from(self.tiles_x);
        let y = id / u64::from(self.tiles_x);
        // Nonempty tile coordinates guarantee these products fit the grid.
        let group_x = (x * u64::from(self.tile_cols)) as u32;
        let group_y = (y * u64::from(self.tile_rows)) as u32;
        Ok(SpatialTile {
            id,
            group_x,
            group_y,
            groups_wide: self.tile_cols.min(self.groups_x - group_x),
            groups_high: self.tile_rows.min(self.groups_y - group_y),
        })
    }

    /// Tile at a physical disk ordinal. No earlier tiles need to be visited.
    pub fn tile_at(self, ordinal: u64) -> Result<SpatialTile, Error> {
        if ordinal >= self.tile_count() {
            return Err(Error::Corrupt("V4 spatial tile ordinal"));
        }
        let (x, y) = match self.order {
            TileOrder::Raster => (
                ordinal % u64::from(self.tiles_x),
                ordinal / u64::from(self.tiles_x),
            ),
            TileOrder::Morton => morton_select(self.tiles_x, self.tiles_y, ordinal),
        };
        self.tile(y * u64::from(self.tiles_x) + x)
    }

    /// Physical ordinal of a tile's stable geometry identity.
    pub fn tile_ordinal(self, id: u64) -> Result<u64, Error> {
        if id >= self.tile_count() {
            return Err(Error::Corrupt("V4 spatial tile identity"));
        }
        Ok(match self.order {
            TileOrder::Raster => id,
            TileOrder::Morton => morton_rank(
                self.tiles_x,
                self.tiles_y,
                id % u64::from(self.tiles_x),
                id / u64::from(self.tiles_x),
            ),
        })
    }

    /// Independently iterable groups of one tile, in their physical order.
    pub fn groups(self, tile_id: u64) -> Result<Groups, Error> {
        Ok(Groups {
            tile: self.tile(tile_id)?,
            grid_width: self.groups_x,
            next: 0,
        })
    }

    /// Spatial tile containing a geometry-derived coding-group identity.
    pub fn tile_for_group(self, group_id: u64) -> Result<SpatialTile, Error> {
        if group_id >= self.group_count() {
            return Err(Error::Corrupt("V4 coding group identity"));
        }
        let x = group_id % u64::from(self.groups_x);
        let y = group_id / u64::from(self.groups_x);
        self.tile(
            (y / u64::from(self.tile_rows)) * u64::from(self.tiles_x)
                + x / u64::from(self.tile_cols),
        )
    }
}

/// Allocation-free group iterator for one spatial tile.
#[derive(Clone, Debug)]
pub struct Groups {
    tile: SpatialTile,
    grid_width: u32,
    next: u64,
}

impl Iterator for Groups {
    type Item = Group;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == self.tile.group_count() {
            return None;
        }
        let x = u64::from(self.tile.group_x) + self.next % u64::from(self.tile.groups_wide);
        let y = u64::from(self.tile.group_y) + self.next / u64::from(self.tile.groups_wide);
        self.next += 1;
        Some(Group {
            id: y * u64::from(self.grid_width) + x,
            x: x as u32,
            y: y as u32,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.tile.group_count() - self.next;
        match usize::try_from(left) {
            Ok(n) => (n, Some(n)),
            Err(_) => (usize::MAX, None),
        }
    }
}

impl core::iter::FusedIterator for Groups {}

fn quadrant_count(width: u32, height: u32, x: u64, y: u64, side: u64) -> u64 {
    u64::from(width).saturating_sub(x).min(side) * u64::from(height).saturating_sub(y).min(side)
}

fn morton_select(width: u32, height: u32, mut ordinal: u64) -> (u64, u64) {
    let mut side = u64::from(width.max(height)).next_power_of_two();
    let (mut x, mut y) = (0, 0);
    while side > 1 {
        side /= 2;
        for q in 0..4 {
            let qx = x + (q & 1) * side;
            let qy = y + (q >> 1) * side;
            let count = quadrant_count(width, height, qx, qy, side);
            if ordinal < count {
                (x, y) = (qx, qy);
                break;
            }
            ordinal -= count;
        }
    }
    (x, y)
}

fn morton_rank(width: u32, height: u32, target_x: u64, target_y: u64) -> u64 {
    let mut side = u64::from(width.max(height)).next_power_of_two();
    let (mut x, mut y, mut ordinal) = (0, 0, 0);
    while side > 1 {
        side /= 2;
        let q = u64::from(target_x >= x + side) + 2 * u64::from(target_y >= y + side);
        for before in 0..q {
            ordinal += quadrant_count(
                width,
                height,
                x + (before & 1) * side,
                y + (before >> 1) * side,
                side,
            );
        }
        x += (q & 1) * side;
        y += (q >> 1) * side;
    }
    ordinal
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    // Independent oracle: enumerate a small grid and sort interleaved bits.
    fn code(x: u32, y: u32) -> u64 {
        let mut value = 0;
        for bit in 0..32 {
            value |= u64::from((x >> bit) & 1) << (2 * bit);
            value |= u64::from((y >> bit) & 1) << (2 * bit + 1);
        }
        value
    }

    #[test]
    fn rectangular_morton_matches_independent_bit_sort() {
        for width in 1..18 {
            for height in 1..18 {
                let layout = TileLayout::new(width, height, 1, 1, TileOrder::Morton).unwrap();
                let mut expected: Vec<_> = (0..height)
                    .flat_map(|y| (0..width).map(move |x| (code(x, y), x, y)))
                    .collect();
                expected.sort_unstable();
                for (n, (_, x, y)) in expected.into_iter().enumerate() {
                    let tile = layout.tile_at(n as u64).unwrap();
                    assert_eq!((tile.group_x, tile.group_y), (x, y));
                    assert_eq!(layout.tile_ordinal(tile.id).unwrap(), n as u64);
                }
            }
        }
    }

    #[test]
    fn tiles_partition_groups_and_preserve_stable_ids() {
        for order in [TileOrder::Raster, TileOrder::Morton] {
            let layout = TileLayout::new(17, 19, 4, 7, order).unwrap();
            let mut groups = Vec::new();
            for n in 0..layout.tile_count() {
                let tile = layout.tile_at(n).unwrap();
                for group in layout.groups(tile.id).unwrap() {
                    assert_eq!(layout.tile_for_group(group.id).unwrap(), tile);
                    assert_eq!(group.id, u64::from(group.y) * 17 + u64::from(group.x));
                    groups.push(group.id);
                }
            }
            groups.sort_unstable();
            assert_eq!(groups, (0..17 * 19).collect::<Vec<_>>());
        }
    }

    #[test]
    fn portrait_tile_groups_are_not_global_raster_order() {
        let layout = TileLayout::new(4, 4, 2, 2, TileOrder::Raster).unwrap();
        let ids: Vec<_> = (0..layout.tile_count())
            .flat_map(|n| layout.groups(layout.tile_at(n).unwrap().id).unwrap())
            .map(|g| g.id)
            .collect();
        assert_eq!(ids, [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15]);
    }

    #[test]
    fn maximum_and_skinny_grids_need_no_permutation_allocation() {
        for (width, height) in [(1, u32::MAX), (u32::MAX, 1), (u32::MAX, u32::MAX)] {
            let layout = TileLayout::new(width, height, 1, 1, TileOrder::Morton).unwrap();
            for n in [0, layout.tile_count() / 2, layout.tile_count() - 1] {
                let tile = layout.tile_at(n).unwrap();
                assert_eq!(layout.tile_ordinal(tile.id).unwrap(), n);
                assert_eq!(layout.groups(tile.id).unwrap().count(), 1);
            }
        }
    }

    #[test]
    fn malformed_layout_and_out_of_range_ids_are_rejected() {
        for args in [(0, 1, 1, 1), (1, 0, 1, 1), (1, 1, 0, 1), (1, 1, 1, 0)] {
            assert!(TileLayout::new(args.0, args.1, args.2, args.3, TileOrder::Raster).is_err());
        }
        let layout = TileLayout::new(5, 7, 3, 4, TileOrder::Morton).unwrap();
        assert!(layout.tile_at(layout.tile_count()).is_err());
        assert!(layout.tile(layout.tile_count()).is_err());
        assert!(layout.tile_for_group(layout.group_count()).is_err());
        assert!(TileOrder::try_from(2).is_err());
    }
}
