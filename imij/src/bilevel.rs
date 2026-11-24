//! Bilevel stripes (SPEC 2.10, feature `bilevel`): in a payload whose only
//! slot has one bit, a unit may code each pixel as one binary symbol whose
//! context is a causal template of ten pixels (the three-line template of
//! JBIG, ITU-T T.82), with one static probability per context in head
//! record 25. Lanes, rANS states, renormalization and word order are the
//! lossless coder's (SPEC 3), with one symbol and no extra bits per step.
//!
//! The template's bit order keeps each of its rows contiguous with `dx`
//! ascending, so one step right shifts a context down a bit and inserts
//! three new pixels: row -2 at `dx` +1, row -1 at `dx` +2, and the pixel
//! just decoded. The decoder looks probabilities up one step ahead: a
//! 512-entry table pairs the probabilities of both values of a context's
//! newest bit, so the lookup for step `x + 1` needs only what is known when
//! step `x` starts.

use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::bits::BitReader;
#[cfg(feature = "encode")]
use crate::bits::BitWriter;
#[cfg(feature = "encode")]
use crate::rans::{self, LOG_M, Sym};
use crate::rans::{LaneState, M, NL, RANS_L, Words};
use archmage::prelude::*;

/// Stripe layout bytes of a bilevel stripe and of a bilevel palette stripe
/// (two entries, SPEC 2.4).
pub(crate) const STRIPE: u8 = 0x81;
pub(crate) const PAL: u8 = 0x82;
/// Lossless head record id of the bilevel table (SPEC 2).
pub(crate) const RECORD: u64 = crate::registry::HEAD_BILEVEL;

/// Template pixels `(dy, dx)`; pixel `i` is context bit `i`.
const T10: [(isize, isize); 10] = [
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
/// Context bits a step right keeps (bits 2, 7 and 9 take the new pixels).
const SLIDE: u32 = 0x17B;

/// The decoder's table (head record 25): `pairs[b]` holds the probability
/// of a 0 (in 1/M) in context `b` (bits 0-8) with bit 9 clear in its low
/// half and with bit 9 set in its high half; `inv` flips decoded values.
pub(crate) struct Table {
    pub(crate) inv: bool,
    pairs: [u32; 512],
}

impl Table {
    /// Parses head record 25 (SPEC 2.10): an inversion bit, then per context
    /// a used bit and, when set, the 11-bit probability of a 0 (at most M).
    pub(crate) fn read(rec: &[u8]) -> Result<Table, Error> {
        let mut r = BitReader::new(rec);
        let inv = r.read_refill(1) == 1;
        let mut f0 = [M / 2; 1024];
        let mut bits: usize = 1 + 1024;
        for p in &mut f0 {
            if r.read_refill(1) == 1 {
                *p = r.read_refill(11);
                bits += 11;
                if *p > M {
                    return Err(Error::Corrupt("bilevel probability above 1"));
                }
            }
        }
        if r.overrun() || rec.len() != bits.div_ceil(8) {
            return Err(Error::Corrupt("bilevel table length"));
        }
        let pairs = core::array::from_fn(|b| f0[b] | f0[b | 512] << 16);
        Ok(Table { inv, pairs })
    }
}

/// Pixel at group column `col` of a decoded step-major row; 0 outside the
/// group (`gw` columns).
fn at(row: &[u32], gw: usize, lw: usize, col: isize) -> u32 {
    if col < 0 || col as usize >= gw {
        return 0;
    }
    row[(col as usize % lw) * NL + col as usize / lw]
}

/// A bilevel unit's decoder rows: rows -2, -1 and the current one, rotating,
/// step-major with three steps past the lane width; steps `lw` and `lw + 1`
/// hold the two columns after each lane's last.
pub(crate) struct Rows {
    b: [Vec<u32>; 3],
    cur: usize,
    /// Group width.
    gw: usize,
    /// Output pixel bytes of values 0 and 1 (built on the first row).
    pub(crate) tbl: Vec<u8>,
}

impl Rows {
    pub(crate) fn new(lw: usize, gw: usize) -> Rows {
        let n = (lw + 3) * NL;
        Rows {
            b: [vec![0; n], vec![0; n], vec![0; n]],
            cur: 0,
            gw,
            tbl: Vec::new(),
        }
    }

    /// Decodes the next row of the unit into `out` through `tbl`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn decode(
        &mut self,
        t: &Table,
        words: &Words<'_>,
        st: &mut LaneState,
        widths: &[u32; NL],
        lw: usize,
        out: &mut [u8],
    ) {
        let (i, gw) = (self.cur, self.gw);
        let [a, b, c] = &mut self.b;
        let (p2, p1, cur) = match i {
            0 => (&*b, &*c, a),
            1 => (&*c, &*a, b),
            _ => (&*a, &*b, c),
        };
        // Step 0's contexts; its current-row pixels lie left of the lane
        // and read the row above.
        let c0 = core::array::from_fn(|l| {
            T10.iter().enumerate().fold(0, |acc, (k, &(dy, dx))| {
                let src = if dy == -2 { p2 } else { p1 };
                acc | at(src, gw, lw, (l * lw) as isize + dx) << k
            })
        });
        row(&t.pairs, c0, words, st, widths, lw, p2, p1, cur);
        for l in 0..NL {
            for k in 0..2 {
                cur[(lw + k) * NL + l] = at(cur, gw, lw, ((l + 1) * lw + k) as isize);
            }
        }
        self.cur = (i + 1) % 3;
        crate::v1::expand(&self.tbl, &self.b[i], widths, lw, out);
    }
}

#[allow(clippy::too_many_arguments)]
fn row(
    t: &[u32; 512],
    c0: [u32; NL],
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    p2: &[u32],
    p1: &[u32],
    cur: &mut [u32],
) {
    if crate::v1::force_scalar() {
        return row_impl_scalar(ScalarToken, t, c0, words, st, widths, lw, p2, p1, cur);
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if !crate::v1::force_avx2()
        && let Some(tok) = archmage::X64V4Token::summon()
    {
        return row_impl_v4(tok, t, c0, words, st, widths, lw, p2, p1, cur);
    }
    incant!(
        row_impl(t, c0, words, st, widths, lw, p2, p1, cur),
        [v3, neon, wasm128, scalar]
    );
}

/// One row of a bilevel unit, the 16 lanes as one vector (SPEC 2.10).
#[magetypes(define(u32x16), v4(cfg(avx512)), v3, neon, wasm128, scalar)]
#[allow(clippy::too_many_arguments)]
fn row_impl(
    token: Token,
    pairs: &[u32; 512],
    c0: [u32; NL],
    words: &Words<'_>,
    st: &mut LaneState,
    widths: &[u32; NL],
    lw: usize,
    p2: &[u32],
    p1: &[u32],
    cur: &mut [u32],
) {
    let splat = |v: u32| u32x16::splat_t(token, v);
    let look =
        |b: u32x16| u32x16::from_array_t(token, b.to_array().map(|i| pairs[i as usize & 511]));
    let c0 = u32x16::from_array_t(token, c0);
    let mut base = c0 & splat(511);
    let mut prev = c0.shr_logical_const::<9>();
    let mut pp = look(base);
    let mut xs = u32x16::from_array_t(token, st.x);
    let wv = u32x16::from_array_t(token, *widths);
    let mut wp = st.wp;
    for x in 0..lw {
        let p = u32x16::blend(
            prev.simd_ne(splat(0)),
            pp.shr_logical_const::<16>(),
            pp & splat(0xFFFF),
        );
        // Step x + 1's context without its newest bit, and its lookup.
        base = ((base | prev.shl_const::<9>()).shr_logical_const::<1>() & splat(SLIDE))
            | u32x16::from_slice_t(token, &p2[(x + 2) * NL..]).shl_const::<2>()
            | u32x16::from_slice_t(token, &p1[(x + 3) * NL..]).shl_const::<7>();
        pp = look(base);
        let slot = xs & splat(M - 1);
        let bit = slot.simd_ge(p);
        let f = u32x16::blend(bit, splat(M) - p, p);
        let n = f * xs.shr_logical_const::<10>() + slot - (bit & p);
        let act = wv.simd_gt(splat(x as u32));
        xs = u32x16::blend(act, n, xs);
        let mut m = (act & xs.simd_lt(splat(RANS_L))).bitmask();
        if m != 0 {
            let mut a = xs.to_array();
            while m != 0 {
                let i = m.trailing_zeros() as usize;
                a[i] = (a[i] << 16) | words.word(wp) as u32;
                wp += 1;
                m &= m - 1;
            }
            xs = u32x16::from_array_t(token, a);
        }
        prev = act & bit & splat(1);
        prev.store((&mut cur[x * NL..x * NL + NL]).try_into().unwrap());
    }
    st.x = xs.to_array();
    st.wp = wp;
}

/// A unit of the encoder: first row, rows, first column, width and lane
/// width.
#[cfg(feature = "encode")]
pub(crate) type Unit = (usize, usize, usize, usize, usize);

/// Calls `f(lane, step, context, value)` for every pixel of a unit in lane
/// order; `v(r, c)` is the (inverted) value at unit row `r`, column `c`.
#[cfg(feature = "encode")]
fn walk(
    v: impl Fn(usize, usize) -> u32,
    u: &Unit,
    widths: &[u32; NL],
    mut f: impl FnMut(usize, u32, u32, u32),
) {
    let (_, rows, _, gw, lw) = *u;
    let px = |r: isize, c: isize| {
        if r < 0 || c < 0 || c as usize >= gw {
            0
        } else {
            v(r as usize, c as usize)
        }
    };
    for (l, &width) in widths.iter().enumerate() {
        let lane0 = (l * lw) as isize;
        for r in 0..rows {
            for x in 0..width as usize {
                let c = lane0 + x as isize;
                let ctx = T10.iter().enumerate().fold(0, |acc, (k, &(dy, dx))| {
                    let (y, cc) = (r as isize + dy, c + dx);
                    // Current-row pixels left of the lane read the row above.
                    acc | px(if dy == 0 && cc < lane0 { y - 1 } else { y }, cc) << k
                });
                f(l, (r * lw + x) as u32, ctx, v(r, c as usize));
            }
        }
    }
}

/// The bilevel coding of a 1-bit plane (`plane[y * w + x]` coded values):
/// head record 25 and the stripe bytes of each unit.
#[cfg(feature = "encode")]
pub(crate) fn encode(
    plane: &[u32],
    w: usize,
    units: Vec<Unit>,
    threads: usize,
) -> (Vec<u8>, Vec<Vec<u8>>) {
    // The majority value codes as 0, the value outside the image.
    let inv = 2 * plane.iter().filter(|&&v| v != 0).count() > plane.len();
    let val = |u: Unit| {
        move |r: usize, c: usize| (plane[(u.0 + r) * w + u.2 + c] != 0) as u32 ^ inv as u32
    };
    let geo = |u: &Unit| crate::v1::lane_widths(u.3, u.4);
    let hists = crate::par::par_map(units.clone(), threads, |_, u| {
        let mut h = vec![0u64; 2048];
        walk(val(u), &u, &geo(&u), |_, _, c, t| {
            h[2 * c as usize + t as usize] += 1
        });
        h
    });
    let mut bw = BitWriter::new();
    bw.put(inv as u32, 1);
    let mut freq = Vec::with_capacity(1024);
    for c in 0..1024 {
        let (c0, c1) = hists
            .iter()
            .fold((0, 0), |a, h| (a.0 + h[2 * c], a.1 + h[2 * c + 1]));
        let t = c0 + c1;
        // The probability of a 0, rounded; 0 and M only for one-sided counts.
        let q = match (c0, c1) {
            (0, 0) => M / 2,
            (_, 0) => M,
            (0, _) => 0,
            _ => (((c0 << LOG_M) + t / 2) / t).clamp(1, M as u64 - 1) as u32,
        };
        bw.put((t > 0) as u32, 1);
        if t > 0 {
            bw.put(q, 11);
        }
        freq.push(vec![q, M - q]);
    }
    let tables = rans::EncodeTables::direct(&freq);
    let streams = crate::par::par_map(units, threads, |_, u| {
        let widths = geo(&u);
        let mut lanes: [Vec<Sym>; NL] = Default::default();
        walk(val(u), &u, &widths, |l, step, c, t| {
            let (ctx, tok) = (c as u16, t as u8);
            lanes[l].push(Sym {
                step,
                ctx,
                tok,
                nb: 0,
                extra: 0,
            });
        });
        let mut out = vec![STRIPE];
        rans::encode_lanes_tables(&tables, &lanes, crate::v1::active_lanes(&widths), &mut out);
        out
    });
    (bw.finish(), streams)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn rng(s: &mut u64) -> u64 {
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        *s
    }

    /// 1-bit test planes: blank, full, checker, noise, blobs, text-like
    /// strokes, sparse dots, stripes.
    pub(crate) fn plane(kind: usize, w: usize, h: usize, seed: u64) -> Vec<u32> {
        let mut s = seed | 1;
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let r = rng(&mut s);
                (match kind {
                    0 => false,
                    1 => true,
                    2 => (x + y).is_multiple_of(2),
                    3 => r & 1 == 0,
                    4 => (x / 3 + y / 5).is_multiple_of(2),
                    5 => (x * 7 + y * 3) % 23 < 4 || r.is_multiple_of(97),
                    6 => r % 100 < 7,
                    _ => y.is_multiple_of(7) || x % 11 == 3,
                }) as u32
            })
            .collect()
    }

    /// The SPEC 2.10 definition, pixel by pixel: one unit's values (before
    /// inversion), natural order, from its stripe bytes.
    fn reference(t: &Table, unit: &[u8], rows: usize, gw: usize, lw: usize) -> Vec<u32> {
        let widths = crate::v1::lane_widths(gw, lw);
        let (words, x0) = Words::from_bytes(&unit[1..], crate::v1::active_lanes(&widths)).unwrap();
        let mut x = x0;
        let mut wp = 0;
        let mut v = vec![0u32; rows * gw];
        let f0 = |c: u32| (t.pairs[c as usize & 511] >> (16 * (c >> 9))) & 0xFFFF;
        for r in 0..rows {
            for s in 0..lw {
                for l in 0..NL {
                    if s >= widths[l] as usize {
                        continue;
                    }
                    let c = (l * lw + s) as isize;
                    let mut ctx = 0;
                    for (k, &(dy, dx)) in T10.iter().enumerate() {
                        let (mut y, cc) = (r as isize + dy, c + dx);
                        if dy == 0 && cc < (l * lw) as isize {
                            y -= 1;
                        }
                        if y >= 0 && cc >= 0 && (cc as usize) < gw {
                            ctx |= v[y as usize * gw + cc as usize] << k;
                        }
                    }
                    let p = f0(ctx);
                    let slot = x[l] % M;
                    let bit = slot >= p;
                    let (f, b) = if bit { (M - p, slot - p) } else { (p, slot) };
                    x[l] = f * (x[l] / M) + b;
                    if x[l] < RANS_L {
                        x[l] = (x[l] << 16) | words.word(wp) as u32;
                        wp += 1;
                    }
                    v[r * gw + c as usize] = bit as u32;
                }
            }
        }
        v
    }

    type RowFn = fn(
        &[u32; 512],
        [u32; NL],
        &Words<'_>,
        &mut LaneState,
        &[u32; NL],
        usize,
        &[u32],
        &[u32],
        &mut [u32],
    );

    /// Every kernel tier this host runs.
    fn tiers() -> Vec<(&'static str, RowFn)> {
        let mut v: Vec<(&'static str, RowFn)> = vec![("scalar", |a, b, c, d, e, f, g, h, i| {
            row_impl_scalar(ScalarToken, a, b, c, d, e, f, g, h, i)
        })];
        #[cfg(target_arch = "x86_64")]
        if X64V3Token::summon().is_some() {
            v.push(("v3", |a, b, c, d, e, f, g, h, i| {
                row_impl_v3(X64V3Token::summon().unwrap(), a, b, c, d, e, f, g, h, i)
            }));
        }
        #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
        if X64V4Token::summon().is_some() {
            v.push(("v4", |a, b, c, d, e, f, g, h, i| {
                row_impl_v4(X64V4Token::summon().unwrap(), a, b, c, d, e, f, g, h, i)
            }));
        }
        #[cfg(target_arch = "aarch64")]
        if NeonToken::summon().is_some() {
            v.push(("neon", |a, b, c, d, e, f, g, h, i| {
                row_impl_neon(NeonToken::summon().unwrap(), a, b, c, d, e, f, g, h, i)
            }));
        }
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        v.push(("wasm128", |a, b, c, d, e, f, g, h, i| {
            row_impl_wasm128(Wasm128Token::summon().unwrap(), a, b, c, d, e, f, g, h, i)
        }));
        v
    }

    /// Decodes a unit with `Rows` driving tier `f` (the production row
    /// setup: step 0 contexts, rotation, the columns after each lane).
    fn tiered(f: RowFn, t: &Table, unit: &[u8], rows: usize, gw: usize, lw: usize) -> Vec<u32> {
        let widths = crate::v1::lane_widths(gw, lw);
        let (words, x) = Words::from_bytes(&unit[1..], crate::v1::active_lanes(&widths)).unwrap();
        let mut st = LaneState {
            x,
            run: [0; NL],
            wp: 0,
        };
        let mut rs = Rows::new(lw, gw);
        let mut v = vec![0u32; rows * gw];
        for r in 0..rows {
            let i = rs.cur;
            let [a, b, c] = &mut rs.b;
            let (p2, p1, cur) = match i {
                0 => (&*b, &*c, a),
                1 => (&*c, &*a, b),
                _ => (&*a, &*b, c),
            };
            let c0 = core::array::from_fn(|l| {
                T10.iter().enumerate().fold(0, |acc, (k, &(dy, dx))| {
                    let src = if dy == -2 { p2 } else { p1 };
                    acc | at(src, gw, lw, (l * lw) as isize + dx) << k
                })
            });
            f(&t.pairs, c0, &words, &mut st, &widths, lw, p2, p1, cur);
            for l in 0..NL {
                for k in 0..2 {
                    cur[(lw + k) * NL + l] = at(cur, gw, lw, ((l + 1) * lw + k) as isize);
                }
                for s in 0..widths[l] as usize {
                    v[r * gw + l * lw + s] = cur[s * NL + l];
                }
            }
            rs.cur = (i + 1) % 3;
        }
        assert!(st.wp <= words.len);
        v
    }

    #[test]
    fn every_tier_matches_the_definition() {
        let mut checked = 0;
        for kind in 0..8 {
            for &(w, h, hs, lanes) in &[
                (1, 1, 1, 16),
                (2, 9, 3, 16),
                (5, 4, 4, 1),
                (16, 7, 3, 16),
                (17, 9, 4, 16),
                (33, 5, 5, 4),
                (40, 12, 5, 16),
                (100, 13, 6, 7),
                (233, 20, 8, 16),
                (515, 9, 9, 16),
            ] {
                let p = plane(kind, w, h, (kind * 977 + w * 31 + h) as u64);
                let lw = w.div_ceil(lanes);
                let units: Vec<Unit> = (0..h.div_ceil(hs))
                    .map(|s| (s * hs, hs.min(h - s * hs), 0, w, lw))
                    .collect();
                let (rec, streams) = encode(&p, w, units.clone(), 2);
                let t = Table::read(&rec).unwrap();
                for (u, bytes) in units.iter().zip(&streams) {
                    let want: Vec<u32> = (0..u.1 * w)
                        .map(|i| p[u.0 * w + i] ^ t.inv as u32)
                        .collect();
                    let r = reference(&t, bytes, u.1, w, lw);
                    assert_eq!(r, want, "reference kind {kind} {w}x{h}");
                    for (name, f) in tiers() {
                        let got = tiered(f, &t, bytes, u.1, w, lw);
                        assert_eq!(got, want, "{name} kind {kind} {w}x{h} lanes {lanes}");
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 0);
    }

    #[test]
    fn table_rejects_bad_lengths_and_probabilities() {
        let p = plane(5, 64, 16, 3);
        let (rec, _) = encode(&p, 64, vec![(0, 16, 0, 64, 4)], 1);
        assert!(Table::read(&rec).is_ok());
        let mut long = rec.clone();
        long.push(0);
        assert!(Table::read(&long).is_err());
        assert!(Table::read(&rec[..rec.len() - 1]).is_err());
        // A used context with probability 1025.
        let mut bw = BitWriter::new();
        bw.put(0, 1);
        bw.put(1, 1);
        bw.put(M + 1, 11);
        for _ in 1..1024 {
            bw.put(0, 1);
        }
        assert!(Table::read(&bw.finish()).is_err());
    }
}
