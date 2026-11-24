//! Entropy models: token schemes and quantized weight tables (SPEC section 3.3).

use crate::bits::{BitReader, unzigzag};
use crate::error::{Error, Result};

pub(crate) const TABLE_SIZE: usize = 1024;

/// A normalized frequency table with a slot lookup.
pub(crate) struct Table {
    pub(crate) freq: Vec<u32>,
    pub(crate) cum: Vec<u32>,
    pub(crate) lookup: [u16; TABLE_SIZE],
}

impl Table {
    /// Build from raw weights with SPEC's deterministic integer normalization.
    fn from_weights(weights: &[u128]) -> Result<Table> {
        let total: u128 = weights.iter().sum();
        if total == 0 {
            return Err(Error::corrupt("empty entropy table"));
        }
        let mut freq = vec![0u32; weights.len()];
        let mut order: Vec<usize> = Vec::new();
        for (index, weight) in weights.iter().enumerate() {
            if *weight > 0 {
                order.push(index);
            }
        }
        for index in &order {
            let scaled = weights[*index] * TABLE_SIZE as u128 / total;
            freq[*index] = scaled.max(1) as u32;
        }
        order.sort_by(|a, b| weights[*b].cmp(&weights[*a]).then(a.cmp(b)));
        let mut sum: i64 = freq.iter().map(|f| i64::from(*f)).sum();
        let mut cursor = 0usize;
        while sum < TABLE_SIZE as i64 {
            let index = order[cursor % order.len()];
            freq[index] += 1;
            sum += 1;
            cursor += 1;
        }
        cursor = 0;
        let mut guard = 0usize;
        while sum > TABLE_SIZE as i64 {
            let index = order[cursor % order.len()];
            if freq[index] > 1 {
                freq[index] -= 1;
                sum -= 1;
            }
            cursor += 1;
            guard += 1;
            if guard > TABLE_SIZE * 4 {
                return Err(Error::corrupt("table normalization failed"));
            }
        }
        let last = order.iter().copied().max().unwrap_or(0);
        freq.truncate(last + 1);
        let mut cum = vec![0u32; freq.len()];
        let mut running = 0u32;
        for (i, f) in freq.iter().enumerate() {
            cum[i] = running;
            running += f;
        }
        if running as usize != TABLE_SIZE {
            return Err(Error::corrupt("table normalization failed"));
        }
        let mut lookup = [0u16; TABLE_SIZE];
        for (symbol, f) in freq.iter().enumerate() {
            for slot in cum[symbol]..cum[symbol] + f {
                lookup[slot as usize] = symbol as u16;
            }
        }
        Ok(Table { freq, cum, lookup })
    }
}

/// One slot's model.
pub(crate) struct Model {
    pub(crate) k: u32,
    pub(crate) m: u32,
    /// The flag SPEC 3.3 describes as the cross-context bit; the reference
    /// writes it for every slot (SPEC finding), and only slots 1 and 2 of
    /// RGB/RGBA use it.
    pub(crate) cross: bool,
    pub(crate) ctx_shift: u32,
    pub(crate) ctx_map: [u8; 16],
    pub(crate) tables: Vec<Table>,
}

impl Model {
    pub(crate) fn context(&self, sum: u32) -> usize {
        let class = (32 - sum.leading_zeros()) as usize;
        self.ctx_map[class.min(15)] as usize
    }
}

/// Read one model. `depth` is the slot's coded depth (bits); the flag bit is
/// present for every slot, and `ctx_shift` only when the depth exceeds 16.
pub(crate) fn read_model(reader: &mut BitReader<'_>, depth: u32, what: &str) -> Result<Model> {
    let k = reader.read(5, what)? as u32;
    let m = reader.read(3, what)? as u32;
    if m > 4 {
        return Err(Error::corrupt(format!("model m > 4 in {what}")));
    }
    let cross = reader.read(1, what)? != 0;
    let ctx_shift = if depth > 16 {
        reader.read(5, what)? as u32
    } else {
        0
    };
    let mut ctx_map = [0u8; 16];
    for entry in &mut ctx_map {
        *entry = reader.read(2, what)? as u8;
    }
    let table_count = ctx_map.iter().copied().max().unwrap_or(0) as usize + 1;
    let mut tables = Vec::with_capacity(table_count);
    for _ in 0..table_count {
        tables.push(read_table(reader, what)?);
    }
    Ok(Model {
        k,
        m,
        cross,
        ctx_shift,
        ctx_map,
        tables,
    })
}

pub(crate) fn read_table(reader: &mut BitReader<'_>, what: &str) -> Result<Table> {
    let last = reader.read(8, what)? as usize;
    let precision = reader.read(3, what)? as u32 + 1;
    let mut weights: Vec<u128> = Vec::with_capacity(last + 1);
    let mut previous: i64 = 0;
    for _ in 0..=last {
        let gamma = reader.gamma(what)?;
        let delta = unzigzag(gamma - 1);
        if delta > 40 {
            return Err(Error::corrupt(format!("table exponent delta in {what}")));
        }
        let exponent = previous + delta;
        if exponent < 0 {
            return Err(Error::corrupt(format!("negative table exponent in {what}")));
        }
        previous = exponent;
        let weight: u128 = match exponent {
            0 => 0,
            1 => 1,
            e => {
                if e > 100 {
                    return Err(Error::corrupt(format!(
                        "table exponent too large in {what}"
                    )));
                }
                let e = e as u32;
                let mantissa_bits = (e - 1).min(precision - 1);
                let mantissa = reader.read(mantissa_bits, what)? as u128;
                ((1u128 << mantissa_bits) | mantissa) << (e - 1 - mantissa_bits)
            }
        };
        weights.push(weight);
    }
    Table::from_weights(&weights)
}

/// The hybrid-integer token schemes (SPEC section 3.2).
pub(crate) struct TokenScheme {
    pub(crate) k: u32,
    pub(crate) m: u32,
    pub(crate) value_tokens: u64,
    pub(crate) run_k: u32,
    pub(crate) run_tokens: u64,
}

impl TokenScheme {
    pub(crate) fn new(model: &Model, depth: u32) -> TokenScheme {
        let kv = model.k.max(model.m).min(depth);
        let value_tokens = if kv >= depth {
            if depth >= 32 {
                u64::from(u32::MAX) + 1
            } else {
                1u64 << depth
            }
        } else {
            (1u64 << kv) + u64::from(depth - kv) * (1u64 << model.m)
        };
        let run_k = kv.max(model.m);
        let run_tokens = (1u64 << run_k) + u64::from(24 - run_k) * (1u64 << model.m);
        TokenScheme {
            k: kv,
            m: model.m,
            value_tokens,
            run_k,
            run_tokens,
        }
    }

    /// Decode token `t` of a scheme: returns (base, extra_bit_count).
    fn scheme(t: u64, k: u32, m: u32) -> Result<(u64, u32)> {
        if t < (1u64 << k) {
            return Ok((t, 0));
        }
        let q = t - (1u64 << k);
        let msb = k + (q >> m) as u32;
        let nb = msb
            .checked_sub(m)
            .ok_or_else(|| Error::corrupt("bad token"))?;
        if nb > 32 {
            return Err(Error::corrupt("token extra bits too large"));
        }
        let base = ((1u64 << m) | (q & ((1u64 << m) - 1))) << nb;
        Ok((base, nb))
    }

    /// A value token: (base, extra bits).
    pub(crate) fn value(&self, symbol: u64) -> Result<(u64, u32)> {
        Self::scheme(symbol, self.k, self.m)
    }

    /// A run token: (run length minus one, extra bits).
    pub(crate) fn run(&self, run_symbol: u64) -> Result<(u64, u32)> {
        Self::scheme(run_symbol, self.run_k, self.m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_scheme_reproduces_known_bases() {
        // k'=5, m=2: t=41 -> base 160, nb 5; t=42 -> 192; t=43 -> 224.
        assert_eq!(TokenScheme::scheme(41, 5, 2).unwrap(), (160, 5));
        assert_eq!(TokenScheme::scheme(42, 5, 2).unwrap(), (192, 5));
        assert_eq!(TokenScheme::scheme(43, 5, 2).unwrap(), (224, 5));
        // With k'=2, m=2, run 764 = base 640 + 124.
        let (base, nb) = TokenScheme::scheme(33, 2, 2).unwrap();
        assert_eq!((base, nb), (640, 7));
        assert!(base + ((1 << nb) - 1) >= 764);
    }

    #[test]
    fn normalization_sums_to_table_size() {
        let table = Table::from_weights(&[342, 341, 341]).unwrap();
        assert_eq!(table.freq.iter().sum::<u32>(), TABLE_SIZE as u32);
        assert_eq!(
            (table.freq[0], table.freq[1], table.freq[2]),
            (342, 341, 341)
        );
    }
}
