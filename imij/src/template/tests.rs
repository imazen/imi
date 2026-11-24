use super::*;

fn rng(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// Index planes of `n` entries: flat regions with edges, noise, rows that
/// repeat the row above, a blank start, dithering, and uniform noise (escape
/// heavy).
fn plane(kind: usize, gw: usize, rows: usize, n: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    let mut v = vec![0u8; gw * rows];
    for y in 0..rows {
        for x in 0..gw {
            let r = rng(&mut s);
            let c = match kind {
                0 => (x / 5 + y / 3) % n,
                1 if r.is_multiple_of(9) => (r >> 8) as usize % n,
                1 => (x / 7 + 2 * (y / 4)) % n,
                2 => ((x * x + y * 3) / 11) % n,
                3 if y < rows / 3 => 0,
                3 => ((x ^ y) / 2) % n,
                4 => ((x + y) % 2 + 2 * ((x / 9) % 2)) % n,
                _ => (r >> 8) as usize % n,
            };
            v[y * gw + x] = c as u8;
        }
        // Repeated rows (typical rows after the first).
        if kind != 5 && y > 0 && rng(&mut s).is_multiple_of(3) {
            v.copy_within((y - 1) * gw..y * gw, y * gw);
        }
    }
    v
}

/// SPEC 2.14 pixel by pixel: one unit's indices from the bytes after its
/// entries.
fn reference(t: &Table, bytes: &[u8], rows: usize, gw: usize, lw: usize, n: usize) -> Vec<u8> {
    let widths = crate::v1::lane_widths(gw, lw);
    let (typical, rest) = split_rows(bytes, rows).unwrap();
    let (words, x0) = Words::from_bytes(rest, crate::v1::active_lanes(&widths)).unwrap();
    let mut x = x0;
    let mut wp = 0;
    let mut v = vec![0u8; rows * gw];
    let at = |v: &[u8], y: isize, c: isize| -> u32 {
        if y < 0 || c < 0 || c as usize >= gw {
            0
        } else {
            v[y as usize * gw + c as usize] as u32
        }
    };
    // One binary symbol of lane `l`: 1 when the slot is at least `p`.
    let bin = |x: &mut u32, wp: &mut usize, p: u32| -> bool {
        let slot = *x % M;
        let bit = slot >= p;
        let (f, b) = if bit { (M - p, slot - p) } else { (p, slot) };
        *x = f * (*x / M) + b;
        if *x < RANS_L {
            *x = (*x << 16) | words.word(*wp) as u32;
            *wp += 1;
        }
        bit
    };
    for r in 0..rows {
        if typical[r] {
            for c in 0..gw {
                v[r * gw + c] = at(&v, r as isize - 1, c as isize) as u8;
            }
            continue;
        }
        for s in 0..lw {
            let lanes: Vec<usize> = (0..NL).filter(|&l| s < widths[l] as usize).collect();
            let tmpl: Vec<[u32; 10]> = lanes
                .iter()
                .map(|&l| {
                    let c = (l * lw + s) as isize;
                    core::array::from_fn(|k| {
                        let (dy, dx) = T10[k];
                        let (mut y, cc) = (r as isize + dy, c + dx);
                        if dy == 0 && cc < (l * lw) as isize {
                            y -= 1;
                        }
                        at(&v, y, cc)
                    })
                })
                .collect();
            let mut val: Vec<Option<u32>> = vec![None; lanes.len()];
            for (i, &l) in lanes.iter().enumerate() {
                let t9 = &tmpl[i];
                let w = t9[9];
                let mut ctx = ((w == 0) as usize) << 9;
                for k in 0..9 {
                    ctx |= ((t9[k] == w) as usize) << k;
                }
                if !bin(&mut x[l], &mut wp, t.a[ctx]) {
                    val[i] = Some(w);
                } else if n <= 2 {
                    val[i] = Some(1 - w);
                }
            }
            for j in 0..5 {
                for (i, &l) in lanes.iter().enumerate() {
                    if val[i].is_some() {
                        continue;
                    }
                    let t9 = &tmpl[i];
                    let c = t9[CAND[j]];
                    if c == t9[9] || CAND[..j].iter().any(|&q| t9[q] == c) {
                        continue;
                    }
                    let cnt = t9.iter().filter(|&&e| e == c).count().min(7);
                    if !bin(&mut x[l], &mut wp, t.b[8 * j + cnt]) {
                        val[i] = Some(c);
                    }
                }
            }
            for (i, &l) in lanes.iter().enumerate() {
                if val[i].is_none() {
                    let e = t.dec[(x[l] % M) as usize];
                    x[l] = ((e >> 8) & 0x7FF) * (x[l] / M) + (e >> 19);
                    if x[l] < RANS_L {
                        x[l] = (x[l] << 16) | words.word(wp) as u32;
                        wp += 1;
                    }
                    val[i] = Some(e & 0xFF);
                }
            }
            for (i, &l) in lanes.iter().enumerate() {
                v[r * gw + l * lw + s] = val[i].unwrap() as u8;
            }
        }
    }
    assert_eq!(wp, words.len, "every word read");
    v
}

/// One unit through a kernel tier, rows emitted as bytes.
#[allow(clippy::too_many_arguments)]
fn kernel_decode(
    rowf: kernel::RowFn,
    t: &Table,
    bytes: &[u8],
    rows: usize,
    gw: usize,
    lw: usize,
    n: usize,
) -> Vec<u8> {
    let widths = crate::v1::lane_widths(gw, lw);
    let (typical, rest) = split_rows(bytes, rows).unwrap();
    let (words, x) = Words::from_bytes(rest, crate::v1::active_lanes(&widths)).unwrap();
    let mut st = LaneState {
        x,
        run: [0; NL],
        wp: 0,
    };
    let mut rw = Rows::new(lw, gw, n, typical);
    rw.tbl = (0..=255u8).collect();
    let mut out = vec![0u8; rows * gw];
    for r in 0..rows {
        rw.decode_with(
            rowf,
            t,
            &words,
            &mut st,
            &widths,
            lw,
            &mut out[r * gw..(r + 1) * gw],
        );
    }
    assert_eq!(st.wp, words.len, "every word read");
    out
}

/// Units of every entry count and lane geometry: the encoder's planes come
/// back from the SPEC reference and from every kernel tier.
#[test]
fn tiers_and_reference_reconstruct_the_encoder_planes() {
    let mut checked = 0;
    for n in [1usize, 2, 3, 4, 5, 9, 17, 64, 256] {
        for &(gw, rows) in &[
            (1usize, 1usize),
            (2, 3),
            (3, 7),
            (16, 5),
            (17, 9),
            (31, 4),
            (40, 33),
            (65, 20),
            (233, 21),
        ] {
            // Full lanes, and narrow lanes that leave some empty.
            for lw in [gw.div_ceil(NL), gw.div_ceil(4).max(1)] {
                let planes: Vec<Vec<u8>> = (0..6)
                    .map(|k| {
                        plane(
                            k,
                            gw,
                            rows,
                            n,
                            (n * 7919 + gw * 13 + rows) as u64 + k as u64,
                        )
                    })
                    .collect();
                // Entry counts as the planes use them (at most n).
                let units: Vec<UnitIndices<'_>> = planes
                    .iter()
                    .map(|p| UnitIndices {
                        unit: (0, rows, 0, gw, lw),
                        idx: p,
                        n: *p.iter().max().unwrap() as usize + 1,
                    })
                    .collect();
                let (rec, streams) = encode(&units, None, 1);
                let t = Table::read(&rec, &[8]).unwrap();
                for (u, st) in units.iter().zip(&streams) {
                    let want = u.idx;
                    let got = reference(&t, st, rows, gw, lw, u.n);
                    assert_eq!(got, want, "reference n={n} {gw}x{rows} lw={lw}");
                    for (name, f) in kernel::tiers() {
                        let got = kernel_decode(f, &t, st, rows, gw, lw, u.n);
                        assert_eq!(got, want, "{name} n={n} {gw}x{rows} lw={lw}");
                    }
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 9 * 9 * 2 * 6);
}

/// Parallel and serial encoding produce the same bytes.
#[test]
fn encoding_is_deterministic_across_threads() {
    let planes: Vec<Vec<u8>> = (0..6).map(|k| plane(k, 70, 30, 12, 5 + k as u64)).collect();
    let units: Vec<UnitIndices<'_>> = planes
        .iter()
        .map(|p| UnitIndices {
            unit: (0, 30, 0, 70, 5),
            idx: p,
            n: 12,
        })
        .collect();
    assert_eq!(encode(&units, None, 1), encode(&units, None, 4));
}

/// Record 35: exact length, probabilities at most M, and the escape table.
#[test]
fn record_admission() {
    let p = plane(1, 40, 20, 9, 3);
    let units = [UnitIndices {
        unit: (0, 20, 0, 40, 3),
        idx: &p,
        n: 9,
    }];
    let (rec, _) = encode(&units, None, 1);
    assert!(Table::read(&rec, &[8]).is_ok());
    let mut long = rec.clone();
    long.push(0);
    assert!(matches!(Table::read(&long, &[8]), Err(Error::Corrupt(_))));
    assert!(matches!(
        Table::read(&rec[..rec.len() - 1], &[8]),
        Err(Error::Corrupt(_))
    ));
    // The first context's probability set to M + 1 (11 bits).
    let mut w = BitWriter::new();
    w.put(7, 3);
    w.put(1, 1);
    w.put(M + 1, 11);
    for _ in 1..1024 + NBCTX {
        w.put(0, 1);
    }
    w.put(0, 2);
    assert!(matches!(
        Table::read(&w.finish(), &[8]),
        Err(Error::Corrupt(_))
    ));
    // No escape table: every slot decodes 0 and keeps the state.
    let mut w = BitWriter::new();
    w.put(0, 3);
    for _ in 0..1024 + NBCTX + 2 {
        w.put(0, 1);
    }
    let t = Table::read(&w.finish(), &[8]).unwrap();
    for slot in [0, 1, 511, 1023] {
        let e = t.dec[slot];
        let x = 5 * M + slot as u32;
        assert_eq!(
            (e & 0xFF, ((e >> 8) & 0x7FF) * (x >> LOG_M) + (e >> 19)),
            (0, x)
        );
    }
    // Precision below 11 bits scales the value; shared entries take each
    // slot's depth.
    let mut w = BitWriter::new();
    w.put(2, 3);
    w.put(1, 1);
    w.put(37, 6);
    for _ in 1..1024 + NBCTX + 1 {
        w.put(0, 1);
    }
    w.put(1, 1);
    w.put(1, 8);
    for v in [5u32, 300, 7, 1023] {
        w.put(v, if v > 255 || v == 1023 { 10 } else { 3 });
    }
    let t = Table::read(&w.finish(), &[3, 10]).unwrap();
    assert_eq!(t.a[0], 37 << 4);
    assert_eq!(t.shared, Some(vec![vec![5, 7], vec![300, 1023]]));
}

/// The typical-row bitmap's padding bits must be 0.
#[test]
fn typical_row_padding() {
    assert!(split_rows(&[0b0000_0101], 3).is_ok());
    assert!(split_rows(&[0b0000_1101], 3).is_err());
    assert!(split_rows(&[0xFF], 8).is_ok());
    assert!(split_rows(&[], 1).is_err());
    // Nine rows take two bytes: rows 1 and 8 repeat the row above.
    let (t, rest) = split_rows(&[0b10, 1, 2], 9).unwrap();
    assert_eq!(t.iter().filter(|&&b| b).count(), 2);
    assert!(t[1] && t[8]);
    assert_eq!(rest, &[2]);
    assert!(split_rows(&[0b10, 0b11], 9).is_err());
}
