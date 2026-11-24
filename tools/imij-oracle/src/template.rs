//! Template palette stripes (SPEC 2.14): head record 35 and stripe layouts
//! 0x83 / 0x84, decoded pixel by pixel from the specification's text.

use crate::bits::{BitReader, Reader};
use crate::error::{Error, Result};
use crate::model::Table;

/// Head record 35.
pub(crate) struct Tables {
    /// Phase 0: the probability (in 1/1024) that the pixel equals W.
    phase0: Vec<u32>,
    /// Candidate phases: the probability that the pixel equals the
    /// candidate, by `8 j + min(count, 7)`.
    candidates: Vec<u32>,
    /// The direct-index table, when the record has one.
    escape: Option<Table>,
    /// Shared entries: per entry, per slot, the coded value.
    pub(crate) shared: Option<Vec<Vec<u32>>>,
}

/// Parses head record 35 of a payload whose slots have `slot_depths` bits.
pub(crate) fn read_tables(payload: &[u8], slot_depths: &[u32]) -> Result<Tables> {
    let mut bits = BitReader::new(payload);
    let b = 4 + bits.read(3, "template precision")? as u32;
    let probabilities = |count: usize, bits: &mut BitReader<'_>| -> Result<Vec<u32>> {
        let mut out = vec![512u32; count];
        for p in out.iter_mut() {
            if bits.read(1, "template used bit")? == 1 {
                let v = bits.read(b, "template probability")? as u32;
                *p = if b == 11 { v } else { v << (10 - b) };
                if *p > 1024 {
                    return Err(Error::corrupt("template probability above 1"));
                }
            }
        }
        Ok(out)
    };
    let phase0 = probabilities(1024, &mut bits)?;
    let candidates = probabilities(40, &mut bits)?;
    let escape = if bits.read(1, "template escape bit")? == 1 {
        Some(crate::model::read_table(
            &mut bits,
            "template direct-index table",
        )?)
    } else {
        None
    };
    let shared = if bits.read(1, "template shared bit")? == 1 {
        if slot_depths.iter().any(|depth| *depth > 16) {
            return Err(Error::corrupt("shared template entries on a deep slot"));
        }
        let count = bits.read(8, "shared entry count")? as usize + 1;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let mut entry = Vec::with_capacity(slot_depths.len());
            for depth in slot_depths {
                entry.push(bits.read(*depth, "shared entry")? as u32);
            }
            entries.push(entry);
        }
        Some(entries)
    } else {
        None
    };
    if payload.len() != bits.position().div_ceil(8) {
        return Err(Error::corrupt("template table length"));
    }
    Ok(Tables {
        phase0,
        candidates,
        escape,
        shared,
    })
}

/// The template pixels `(dy, dx)` of SPEC 2.10; `t_9` is W.
const TEMPLATE: [(isize, isize); 10] = [
    (-2, -1),
    (-2, 0),
    (-2, 1),
    (-1, -2),
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (-1, 2),
    (0, -2),
    (0, -1),
];

/// The candidates in order, as template indices: N, NE, NW, WW, NN.
const CANDIDATES: [usize; 5] = [5, 6, 4, 8, 1];

/// One stream's lane states and word position.
struct Lanes<'a> {
    stream: &'a [u8],
    states: Vec<u32>,
    pos: usize,
}

impl Lanes<'_> {
    fn renormalize(&mut self, lane: usize) -> Result<()> {
        if self.states[lane] < 1 << 16 {
            if self.pos + 2 > self.stream.len() {
                return Err(Error::corrupt("rANS word underrun"));
            }
            let word = u16::from_le_bytes([self.stream[self.pos], self.stream[self.pos + 1]]);
            self.states[lane] = self.states[lane] << 16 | u32::from(word);
            self.pos += 2;
        }
        Ok(())
    }

    /// One binary symbol with probability `p` of a 0: 1 when the slot is at
    /// least `p`.
    fn binary(&mut self, lane: usize, p: u32) -> Result<bool> {
        let state = self.states[lane];
        let slot = state % 1024;
        let one = slot >= p;
        self.states[lane] = if one {
            (1024 - p) * (state >> 10) + slot - p
        } else {
            p * (state >> 10) + slot
        };
        self.renormalize(lane)?;
        Ok(one)
    }

    /// One token of `table`; without a table, 0 with the state unchanged.
    fn token(&mut self, lane: usize, table: Option<&Table>) -> Result<u32> {
        let Some(table) = table else {
            return Ok(0);
        };
        let state = self.states[lane];
        let slot = state % 1024;
        let symbol = table.lookup[slot as usize] as usize;
        self.states[lane] = table.freq[symbol] * (state >> 10) + slot - table.cum[symbol];
        self.renormalize(lane)?;
        Ok(symbol as u32)
    }
}

/// Decodes a template palette stripe after its layout byte: `decor` 0x83
/// (own entries) or 0x84 (the shared ones). Returns per stripe row and
/// group column the selected entry (slot values). `widths` are the active
/// lanes' widths and `lane_width` the lanes' width.
pub(crate) fn decode(
    r: &mut Reader<'_>,
    decor: u8,
    tables: &Tables,
    slot_depths: &[u32],
    rows: usize,
    widths: &[usize],
    lane_width: usize,
) -> Result<Vec<Vec<Vec<u32>>>> {
    let entries: Vec<Vec<u32>> = if decor == 0x83 {
        if slot_depths.iter().any(|depth| *depth > 16) {
            return Err(Error::corrupt("template palette with a deep slot"));
        }
        let count = r.u8("template palette entries")? as usize + 1;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let mut entry = Vec::new();
            for depth in slot_depths {
                let value = if *depth <= 8 {
                    u32::from(r.u8("palette entry")?)
                } else {
                    u32::from(u16::from_le_bytes(
                        r.take(2, "palette entry")?.try_into().unwrap(),
                    ))
                };
                if value >= (1u32 << *depth) {
                    return Err(Error::corrupt("palette entry out of range"));
                }
                entry.push(value);
            }
            entries.push(entry);
        }
        entries
    } else {
        tables
            .shared
            .clone()
            .ok_or_else(|| Error::corrupt("shared template stripe without shared entries"))?
    };
    let n = entries.len();
    let bitmap = r.take(rows.div_ceil(8), "typical-row bitmap")?;
    let typical: Vec<bool> = (0..rows)
        .map(|row| bitmap[row / 8] >> (row % 8) & 1 == 1)
        .collect();
    for bit in rows..bitmap.len() * 8 {
        if bitmap[bit / 8] >> (bit % 8) & 1 == 1 {
            return Err(Error::corrupt("typical-row bitmap padding"));
        }
    }
    let stream = r.take(r.remaining(), "template stream")?;
    if stream.len() < 4 * widths.len() {
        return Err(Error::corrupt("short rANS state list"));
    }
    let states: Vec<u32> = (0..widths.len())
        .map(|lane| u32::from_le_bytes(stream[4 * lane..4 * lane + 4].try_into().unwrap()))
        .collect();
    if states.iter().any(|state| *state < 1 << 16) {
        return Err(Error::corrupt("rANS state below 2^16"));
    }
    let mut lanes = Lanes {
        stream,
        states,
        pos: 4 * widths.len(),
    };
    let group_width: usize = widths.iter().sum();
    let mut index = vec![vec![0u32; group_width]; rows];
    for row in 0..rows {
        if typical[row] {
            // A copy of the row above (indices 0 above the stripe).
            if row > 0 {
                index[row] = index[row - 1].clone();
            }
            continue;
        }
        for x in 0..lane_width {
            let active: Vec<usize> = (0..widths.len()).filter(|lane| x < widths[*lane]).collect();
            // The template of each active lane's pixel at this step.
            let template: Vec<[u32; 10]> = active
                .iter()
                .map(|lane| {
                    let lane0 = (lane * lane_width) as isize;
                    let column = lane0 + x as isize;
                    let mut t = [0u32; 10];
                    for (k, (dy, dx)) in TEMPLATE.iter().enumerate() {
                        let (mut y, c) = (row as isize + dy, column + dx);
                        if *dy == 0 && c < lane0 {
                            y -= 1;
                        }
                        if y >= 0 && c >= 0 && (c as usize) < group_width {
                            t[k] = index[y as usize][c as usize];
                        }
                    }
                    t
                })
                .collect();
            let mut value: Vec<Option<u32>> = vec![None; active.len()];
            // Phase 0.
            for (i, lane) in active.iter().enumerate() {
                let t = &template[i];
                let w = t[9];
                let mut ctx = usize::from(w == 0) << 9;
                for (k, tk) in t.iter().take(9).enumerate() {
                    ctx |= usize::from(*tk == w) << k;
                }
                if !lanes.binary(*lane, tables.phase0[ctx])? {
                    value[i] = Some(w);
                } else if n <= 2 {
                    value[i] = Some(1 - w.min(1));
                }
            }
            // Candidate phases.
            for (j, candidate) in CANDIDATES.iter().enumerate() {
                for (i, lane) in active.iter().enumerate() {
                    if value[i].is_some() {
                        continue;
                    }
                    let t = &template[i];
                    let c = t[*candidate];
                    if c == t[9] || CANDIDATES[..j].iter().any(|earlier| t[*earlier] == c) {
                        continue;
                    }
                    let count = t.iter().filter(|tk| **tk == c).count().min(7);
                    if !lanes.binary(*lane, tables.candidates[8 * j + count])? {
                        value[i] = Some(c);
                    }
                }
            }
            // Escape phase.
            for (i, lane) in active.iter().enumerate() {
                if value[i].is_none() {
                    value[i] = Some(lanes.token(*lane, tables.escape.as_ref())?);
                }
            }
            for (i, lane) in active.iter().enumerate() {
                index[row][lane * lane_width + x] = value[i].unwrap();
            }
        }
    }
    Ok(index
        .iter()
        .map(|row| {
            row.iter()
                .map(|i| entries[if (*i as usize) < n { *i as usize } else { 0 }].clone())
                .collect()
        })
        .collect())
}
