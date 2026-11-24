//! Frozen writer-token prices, including the contexts changed by a block.
//! rANS uses -log2 normalized probability, Huffman its selected code lengths,
//! both plus raw bits. This excludes table/header bytes and rANS rounding.
//! Absent symbols use the production trellis fallback (one slot / LOG bits).
use super::*;

pub(super) struct Model<'p, 'file> {
    p: &'p LossyParsed<'file>,
    rl: Option<trellis::Prices>,
}

fn support(q: &[i32]) -> u32 {
    (1..64)
        .rev()
        .find(|&k| q[coef::ZIGZAG[k] as usize] != 0)
        .unwrap_or(0) as u32
}

fn count(q: &[i32]) -> u32 {
    support(q).div_ceil(4)
}

impl<'p, 'file> Model<'p, 'file> {
    pub(super) fn new(p: &'p LossyParsed<'file>) -> Self {
        Self {
            p,
            rl: p.huf.is_none().then(|| trellis::Prices::new(&p.tables)),
        }
    }

    fn hbit(&self, map: &[u8], ctx: usize, symbol: usize) -> f64 {
        let t = &self.p.huf.as_ref().unwrap().0;
        t.lens[map[ctx] as usize]
            .get(symbol)
            .copied()
            .filter(|&v| v > 0)
            .unwrap_or(huf::LOG as u8) as f64
    }

    /// Full changed block, plus only its successor's context-sensitive luma
    /// count token. Stripe and Huffman-slice boundaries reset that context.
    fn local(&self, q: &[i32], b: usize, changed: usize) -> f64 {
        let bw = self.p.g.bw;
        let by = b / bw;
        let first = by / self.p.g.sb * self.p.g.sb * bw;
        let end = (first + self.p.g.sb * bw).min(q.len() / 192);
        let block = |index: usize, ch: usize| &q[(index * 3 + ch) * 64..][..64];
        if let Some(prices) = &self.rl {
            let r = (b - first) / bw;
            let x = b % bw;
            let previous = if x >= NL {
                Some(b - NL)
            } else if r > 0 {
                Some(first + (r - 1) * bw + x + (bw - 1 - x) / NL * NL)
            } else {
                None
            };
            let next = if x + NL < bw {
                Some(b + NL)
            } else if b + bw < end {
                Some(b + bw - x / NL * NL)
            } else {
                None
            };
            let luma = support(block(b, 0));
            let mut bits = 0.0;
            for ch in if changed == 0 {
                0..3
            } else {
                changed..changed + 1
            } {
                let coefficients = block(b, ch);
                let eob = support(coefficients);
                let prev = if ch == 0 {
                    previous.map_or(0, |i| support(block(i, 0)))
                } else {
                    luma
                };
                bits += prices.token(eob_ctx(ch, prev), eob);
                let mut start = 1u32;
                for k in 1..=eob {
                    let value = coefficients[coef::ZIGZAG[k as usize] as usize];
                    if value == 0 {
                        continue;
                    }
                    let mut run = k - start;
                    while run > coef::MAX_RUN {
                        bits += prices.token(ac_ctx(ch, start, eob), coef::ZRL);
                        start += 4;
                        run -= 4;
                    }
                    let (token, _, raw) = rl_token(run, value);
                    bits += prices.token(ac_ctx(ch, start, eob), token) + raw as f64;
                    start = k + 1;
                }
            }
            if let Some(next) = next.filter(|_| changed == 0) {
                bits += prices.token(eob_ctx(0, luma), support(block(next, 0)));
            }
            bits
        } else {
            let t = &self.p.huf.as_ref().unwrap().0;
            let slice = huf::slice_len(end - first);
            let previous = if (b - first).is_multiple_of(slice) {
                0
            } else {
                count(block(b - 1, 0))
            };
            let luma = count(block(b, 0));
            let mut bits = if changed == 0 {
                self.hbit(&t.luma_map, huf::class(previous), luma as usize)
            } else {
                0.0
            };
            for ch in if changed == 0 {
                0..3
            } else {
                changed..changed + 1
            } {
                let coefficients = block(b, ch);
                let n = count(coefficients);
                if ch > 0 {
                    bits += self.hbit(
                        &t.chroma_map,
                        (ch - 1) * huf::CLASSES + huf::class(luma),
                        n as usize,
                    );
                }
                for gi in 0..n as usize {
                    let len = coef::pk_len(gi);
                    let magnitude = (0..len)
                        .map(|j| coefficients[coef::ZIGZAG[1 + 4 * gi + j] as usize].unsigned_abs())
                        .max()
                        .unwrap();
                    let width = 32 - magnitude.leading_zeros();
                    bits += self.hbit(&t.wid_map, huf::wid_ctx(ch, gi, luma), width as usize)
                        + (len as u32 * width) as f64;
                    bits += (0..len)
                        .filter(|&j| coefficients[coef::ZIGZAG[1 + 4 * gi + j] as usize] != 0)
                        .count() as f64;
                }
            }
            if changed == 0 && b + 1 < end && !(b + 1 - first).is_multiple_of(slice) {
                bits += self.hbit(
                    &t.luma_map,
                    huf::class(luma),
                    count(block(b + 1, 0)) as usize,
                );
            }
            bits
        }
    }

    pub(super) fn deltas(&self, q: &mut [i32], qi: usize) -> [f64; 2] {
        assert!(
            !qi.is_multiple_of(64),
            "DC uses a different entropy grammar"
        );
        let old = q[qi];
        let b = qi / 192;
        let changed = qi / 64 % 3;
        let before = self.local(q, b, changed);
        let result = [-1, 1].map(|delta| {
            if (old + delta).unsigned_abs() >= 1 << 22 {
                return f64::INFINITY;
            }
            q[qi] = old + delta;
            self.local(q, b, changed) - before
        });
        q[qi] = old;
        result
    }
}

#[test]
fn local_deltas_match_complete_writer_token_scans() {
    fn writer(model: &Model<'_, '_>, q: &[i32]) -> f64 {
        let p = model.p;
        let mut total = 0.0;
        for s in 0..p.g.stripes() {
            let (a, b) = p.g.rows(s);
            let values = q[a * p.g.bw * 192..b * p.g.bw * 192].to_vec();
            let nz = values
                .as_chunks::<64>()
                .0
                .iter()
                .map(|v| {
                    coef::ZIGZAG
                        .iter()
                        .enumerate()
                        .fold(0, |m, (k, &z)| m | ((v[z as usize] != 0) as u64) << k)
                })
                .collect();
            let sc = StripeCoefs {
                by0: a,
                nby: b - a,
                bw: p.g.bw,
                q: values,
                nz,
                aq: Vec::new(),
                modes: Vec::new(),
                splits: Vec::new(),
            };
            if let Some(prices) = &model.rl {
                stripe_tokens(&sc, sc.bw, Group::ALL, |_, ctx, tok, n, _| {
                    total += prices.token(ctx, tok) + n as f64
                });
            } else {
                let (counts, widths) = stripe_counts_widths(&sc, sc.bw);
                let syms = huf::stripe_syms(&counts, &widths, sc.nby * sc.bw);
                let t = &p.huf.as_ref().unwrap().0;
                for (list, map) in [
                    (&syms.luma, &t.luma_map[..]),
                    (&syms.chroma, &t.chroma_map[..]),
                    (&syms.wid, &t.wid_map[..]),
                ] {
                    for &(ctx, symbol) in list {
                        total += model.hbit(map, ctx as usize, symbol as usize);
                    }
                }
                for bc in 0..counts.len() {
                    for gi in 0..counts[bc] as usize {
                        total +=
                            (coef::pk_len(gi) * widths[bc * coef::PK_GROUPS + gi] as usize) as f64;
                    }
                    total += sc.q[bc * 64 + 1..bc * 64 + 64]
                        .iter()
                        .filter(|&&v| v != 0)
                        .count() as f64;
                }
            }
        }
        total
    }
    let (w, h) = (131, 39);
    let mut state = 47u32;
    let source: Vec<_> = (0..w * h * 3)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    let mut maximum = 0.0f64;
    let mut cases = 0;
    for color in [LossyColor::Xyb, LossyColor::YCoCg] {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for seed in [0, 1] {
                let mut cfg = LossyConfig::new(35.0)
                    .with_color(color)
                    .with_entropy(entropy)
                    .with_stripe_block_rows(4);
                cfg.lab.superblocks = Some(false);
                cfg.column_groups = Some(0);
                cfg.seed_bank = seed;
                let mut info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
                info.cicp = crate::Cicp::SRGB;
                let file = encode_lossy(&source, w * 3, &info, &Metadata::default(), &cfg).unwrap();
                let p = parse_lossy(&file).unwrap();
                let model = Model::new(&p);
                let (_, _, mut q, _, _) = quantized_lab(&file).unwrap();
                for i in 0..75 {
                    let block = (i * 31) % (q.len() / 192);
                    let ch = i % 3;
                    let k = 1 + (i * 17) % 63;
                    let qi = (block * 3 + ch) * 64 + k;
                    let old = q[qi];
                    let before = writer(&model, &q);
                    let deltas = model.deltas(&mut q, qi);
                    assert_eq!(q[qi], old);
                    for (j, delta) in [-1, 1].into_iter().enumerate() {
                        q[qi] = old + delta;
                        let expected = writer(&model, &q) - before;
                        q[qi] = old;
                        let error = (expected - deltas[j]).abs();
                        maximum = maximum.max(error);
                        cases += 1;
                        assert!(
                            error < 1e-8,
                            "{entropy:?}, block {block}, ch {ch}, k {k}: {expected} versus {}",
                            deltas[j]
                        );
                    }
                }
            }
        }
    }
    assert_eq!(cases, 1200);
    println!("1200 frozen writer-token deltas; maximum absolute error {maximum} bits");
}
