use crate::wire::{Cursor, add, bad, put};
use std::io;

pub const CATALOG_KEY: u64 = 0;
pub const CATALOG_MAGIC: &[u8; 8] = b"IMIJPAT\x01";
const MAX_LEVELS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TileOrder {
    RowMajor = 0,
    Morton = 1,
}

impl TileOrder {
    fn from_byte(b: u8) -> io::Result<Self> {
        match b {
            0 => Ok(Self::RowMajor),
            1 => Ok(Self::Morton),
            _ => Err(bad("unknown pyramid tile order")),
        }
    }
}

fn part1by1(mut x: u64) -> u64 {
    x &= 0xffff_ffff;
    x = (x | (x << 16)) & 0x0000_ffff_0000_ffff;
    x = (x | (x << 8)) & 0x00ff_00ff_00ff_00ff;
    x = (x | (x << 4)) & 0x0f0f_0f0f_0f0f_0f0f;
    x = (x | (x << 2)) & 0x3333_3333_3333_3333;
    (x | (x << 1)) & 0x5555_5555_5555_5555
}

fn compact1by1(mut x: u64) -> u32 {
    x &= 0x5555_5555_5555_5555;
    x = (x | (x >> 1)) & 0x3333_3333_3333_3333;
    x = (x | (x >> 2)) & 0x0f0f_0f0f_0f0f_0f0f;
    x = (x | (x >> 4)) & 0x00ff_00ff_00ff_00ff;
    x = (x | (x >> 8)) & 0x0000_ffff_0000_ffff;
    ((x | (x >> 16)) & 0xffff_ffff) as u32
}

pub fn morton_encode_2d(tx: u32, ty: u32) -> u64 {
    part1by1(u64::from(tx)) | (part1by1(u64::from(ty)) << 1)
}

pub fn morton_decode_2d(z: u64) -> (u32, u32) {
    (compact1by1(z), compact1by1(z >> 1))
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PyramidLevel {
    pub level_id: u32,
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub order: TileOrder,
    pub key_base: u64,
}

impl PyramidLevel {
    pub fn new(
        level_id: u32,
        width: u32,
        height: u32,
        tile_width: u32,
        tile_height: u32,
        order: TileOrder,
        key_base: u64,
    ) -> io::Result<Self> {
        let level = Self {
            level_id,
            width,
            height,
            tile_width,
            tile_height,
            order,
            key_base,
        };
        level.validate()?;
        Ok(level)
    }

    pub fn cols(&self) -> u32 {
        self.width.div_ceil(self.tile_width)
    }

    pub fn rows(&self) -> u32 {
        self.height.div_ceil(self.tile_height)
    }

    pub fn tile_count(&self) -> u64 {
        u64::from(self.cols()) * u64::from(self.rows())
    }

    pub fn tile_index(&self, tx: u32, ty: u32) -> io::Result<u64> {
        if tx >= self.cols() || ty >= self.rows() {
            return Err(bad("pyramid tile coordinates out of bounds"));
        }
        Ok(match self.order {
            TileOrder::RowMajor => u64::from(ty) * u64::from(self.cols()) + u64::from(tx),
            TileOrder::Morton => morton_encode_2d(tx, ty),
        })
    }

    pub fn tile_key(&self, tx: u32, ty: u32) -> io::Result<u64> {
        add(self.key_base, self.tile_index(tx, ty)?)
    }

    pub fn key_bounds(&self) -> io::Result<(u64, u64)> {
        self.validate()?;
        let max_index = match self.order {
            TileOrder::RowMajor => self.tile_count() - 1,
            TileOrder::Morton => morton_encode_2d(self.cols() - 1, self.rows() - 1),
        };
        Ok((self.key_base, add(self.key_base, max_index)?))
    }

    pub fn keys_for_rect(&self, x: u32, y: u32, width: u32, height: u32) -> io::Result<Vec<u64>> {
        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|e| e > self.width)
            || y.checked_add(height).is_none_or(|e| e > self.height)
        {
            return Err(bad("pyramid region outside level bounds"));
        }
        let tx0 = x / self.tile_width;
        let tx1 = (x + width - 1) / self.tile_width + 1;
        let ty0 = y / self.tile_height;
        let ty1 = (y + height - 1) / self.tile_height + 1;
        let count = usize::try_from(u64::from(tx1 - tx0) * u64::from(ty1 - ty0))
            .ok()
            .filter(|&n| n <= 4096)
            .ok_or_else(|| bad("pyramid region exceeds 4096 tiles"))?;
        let mut keys = Vec::with_capacity(count);
        for ty in ty0..ty1 {
            for tx in tx0..tx1 {
                keys.push(self.tile_key(tx, ty)?);
            }
        }
        keys.sort_unstable();
        Ok(keys)
    }

    fn validate(&self) -> io::Result<()> {
        if self.width == 0
            || self.height == 0
            || self.tile_width == 0
            || self.tile_height == 0
            || self.key_base == CATALOG_KEY
        {
            return Err(bad("invalid pyramid level geometry or key base"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PyramidCatalog {
    levels: Vec<PyramidLevel>,
}

impl PyramidCatalog {
    pub fn new(levels: Vec<PyramidLevel>) -> io::Result<Self> {
        if levels.is_empty() || levels.len() > MAX_LEVELS {
            return Err(bad("pyramid catalog level count"));
        }
        let mut bounds = Vec::with_capacity(levels.len());
        let mut prev_id = None;
        for level in &levels {
            if prev_id.is_some_and(|p| level.level_id <= p) {
                return Err(bad("pyramid level IDs must strictly increase"));
            }
            prev_id = Some(level.level_id);
            bounds.push(level.key_bounds()?);
        }
        bounds.sort_unstable_by_key(|&(start, _)| start);
        for pair in bounds.windows(2) {
            if pair[0].1 >= pair[1].0 {
                return Err(bad("overlapping pyramid level key domains"));
            }
        }
        Ok(Self { levels })
    }

    pub fn levels(&self) -> &[PyramidLevel] {
        &self.levels
    }

    pub fn level(&self, level_id: u32) -> Option<&PyramidLevel> {
        self.levels.iter().find(|l| l.level_id == level_id)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.levels.len() * 24);
        out.extend_from_slice(CATALOG_MAGIC);
        put(&mut out, self.levels.len() as u64);
        for l in &self.levels {
            put(&mut out, u64::from(l.level_id));
            put(&mut out, u64::from(l.width));
            put(&mut out, u64::from(l.height));
            put(&mut out, u64::from(l.tile_width));
            put(&mut out, u64::from(l.tile_height));
            out.push(l.order as u8);
            put(&mut out, l.key_base);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > 64 * 1024 {
            return Err(bad("pyramid catalog exceeds 64 KiB"));
        }
        let mut c = Cursor(bytes);
        if c.take(8)? != CATALOG_MAGIC {
            return Err(bad("pyramid catalog magic"));
        }
        let count = usize::try_from(c.var()?)
            .ok()
            .filter(|&n| (1..=MAX_LEVELS).contains(&n))
            .ok_or_else(|| bad("pyramid catalog level count"))?;
        let mut levels = Vec::with_capacity(count);
        let u32_var = |c: &mut Cursor<'_>| -> io::Result<u32> {
            u32::try_from(c.var()?).map_err(|_| bad("pyramid dimension overflow"))
        };
        for _ in 0..count {
            let level = PyramidLevel::new(
                u32_var(&mut c)?,
                u32_var(&mut c)?,
                u32_var(&mut c)?,
                u32_var(&mut c)?,
                u32_var(&mut c)?,
                TileOrder::from_byte(c.byte()?)?,
                c.var()?,
            )?;
            levels.push(level);
        }
        c.end()?;
        Self::new(levels)
    }
}
