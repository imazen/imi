//! Interleaved rANS decoding (SPEC section 3).

use crate::error::{Error, Result};
use crate::model::{Model, TABLE_SIZE, TokenScheme};

/// Residual planes of one slot in one unit: `u[lane][row][x]` (zigzag values).
pub(crate) type Residuals = Vec<Vec<Vec<u32>>>;

struct WordStream<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> WordStream<'a> {
    fn word(&mut self) -> Result<u16> {
        if self.pos + 2 > self.data.len() {
            return Err(Error::corrupt("rANS word underrun"));
        }
        let word = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Ok(word)
    }
}

/// Decode one slot's stream of one stripe or column group.
///
/// `widths` holds the active lanes' widths (a non-empty prefix of the L
/// lanes); `slot0` is slot 0's residuals for the cross context, when the
/// model sets it. Words are consumed in (step, phase, lane) order over the
/// token, extra-bits and high-extra-bits phases.
pub(crate) fn decode_stream(
    data: &[u8],
    widths: &[usize],
    rows: usize,
    model: &Model,
    depth: u32,
    slot0: Option<&Residuals>,
) -> Result<Residuals> {
    let lanes = widths.len();
    let token_scheme = TokenScheme::new(model, depth);
    let mut stream = WordStream { data, pos: 0 };
    let mut states = Vec::with_capacity(lanes);
    for _ in 0..lanes {
        if stream.pos + 4 > data.len() {
            return Err(Error::corrupt("short rANS state list"));
        }
        let state = u32::from_le_bytes([
            data[stream.pos],
            data[stream.pos + 1],
            data[stream.pos + 2],
            data[stream.pos + 3],
        ]);
        stream.pos += 4;
        if state < (1 << 16) {
            return Err(Error::corrupt("rANS state below 2^16"));
        }
        states.push(state);
    }
    let mut run_remaining = vec![0u64; lanes];
    let mut residuals: Residuals = (0..lanes)
        .map(|lane| vec![vec![0u32; widths[lane]]; rows])
        .collect();

    let context_of = |lane: usize,
                      row: usize,
                      x: usize,
                      residuals: &Residuals,
                      shift: u32,
                      slot0: Option<&Residuals>|
     -> usize {
        let scaled = |value: u32| -> u32 { (value >> shift).min(1 << 16) };
        let left = if x > 0 {
            scaled(residuals[lane][row][x - 1])
        } else {
            0
        };
        let up = if row > 0 {
            scaled(residuals[lane][row - 1][x])
        } else {
            0
        };
        let mut sum = if x > 0 && row > 0 {
            left + up
        } else if x > 0 {
            2 * left
        } else if row > 0 {
            2 * up
        } else {
            0
        };
        if let Some(cross) = slot0 {
            sum += scaled(cross[lane][row][x]);
        }
        model.context(sum)
    };

    fn renorm(state: &mut u32, stream: &mut WordStream<'_>) -> Result<()> {
        if *state < (1 << 16) {
            *state = (*state << 16) | u32::from(stream.word()?);
            if *state < (1 << 16) {
                return Err(Error::corrupt("rANS state stuck below 2^16"));
            }
        }
        Ok(())
    }

    let lane_width = widths.iter().copied().max().unwrap_or(0);
    for row in 0..rows {
        for x in 0..lane_width {
            let active: Vec<usize> = (0..lanes).filter(|lane| x < widths[*lane]).collect();
            // Token phase: lane ascending.
            let mut tokens: Vec<Option<(bool, u64, u32)>> = vec![None; lanes];
            for &lane in &active {
                if run_remaining[lane] > 0 {
                    run_remaining[lane] -= 1;
                    residuals[lane][row][x] = 0;
                    continue;
                }
                let context = context_of(lane, row, x, &residuals, model.ctx_shift, slot0);
                let table = &model.tables[context];
                let state = states[lane];
                let slot = (state as usize) % TABLE_SIZE;
                let symbol = table.lookup[slot] as usize;
                let frequency = table.freq[symbol];
                states[lane] = frequency
                    .wrapping_mul(state >> 10)
                    .wrapping_add(slot as u32 - table.cum[symbol]);
                renorm(&mut states[lane], &mut stream)?;
                tokens[lane] = Some(if (symbol as u64) < token_scheme.value_tokens {
                    let (base, nb) = token_scheme.value(symbol as u64)?;
                    (true, base, nb)
                } else {
                    let run_symbol = symbol as u64 - token_scheme.value_tokens;
                    if run_symbol >= token_scheme.run_tokens {
                        return Err(Error::corrupt("run token out of the alphabet"));
                    }
                    let (base, nb) = token_scheme.run(run_symbol)?;
                    (false, base, nb)
                });
            }
            // Extra-bits phase: lane ascending, then the high-extra-bits
            // phase over all lanes (SPEC 3.1: the phases are lane-major).
            let mut extras: Vec<u64> = vec![0; lanes];
            for &lane in &active {
                let Some((_, _, nb)) = tokens[lane] else {
                    continue;
                };
                if nb == 0 {
                    continue;
                }
                let n1 = nb.min(16);
                extras[lane] = (states[lane] as u64) & ((1u64 << n1) - 1);
                states[lane] >>= n1;
                renorm(&mut states[lane], &mut stream)?;
            }
            for &lane in &active {
                let Some((_, _, nb)) = tokens[lane] else {
                    continue;
                };
                if nb <= 16 {
                    continue;
                }
                let n2 = nb - 16;
                extras[lane] |= ((states[lane] as u64) & ((1u64 << n2) - 1)) << 16;
                states[lane] >>= n2;
                renorm(&mut states[lane], &mut stream)?;
            }
            // Commit.
            for &lane in &active {
                let Some((is_value, base, _)) = tokens[lane] else {
                    continue;
                };
                let value = base + extras[lane];
                if is_value {
                    if depth < 32 && value >= (1u64 << depth) {
                        return Err(Error::corrupt("value token out of range"));
                    }
                    if value > u64::from(u32::MAX) {
                        return Err(Error::corrupt("value token out of range"));
                    }
                    residuals[lane][row][x] = value as u32;
                } else {
                    residuals[lane][row][x] = 0;
                    run_remaining[lane] = value;
                }
            }
        }
    }
    for (lane, remaining) in run_remaining.iter().enumerate() {
        if *remaining > 0 {
            return Err(Error::corrupt(format!(
                "zero run past the end of lane {lane}"
            )));
        }
    }
    Ok(residuals)
}
