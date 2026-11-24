//! Compare the fixed-model row scanner to the ordinary transpose/context path.
use super::*;

fn compare(width: usize, rows: usize, depth: u32, pattern: usize, padding: usize) {
    let stride = width + padding;
    let mut pixels = vec![0xbadc0ffe; stride * rows];
    let mut tight = Vec::new();
    let mut rng = 0x193ac54bu32;
    for y in 0..rows {
        for x in 0..width {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            let mask = crate::mask_of(depth);
            let value = match pattern {
                0 => 0,
                1 => rng & mask,
                _ => {
                    if (y * width + x) % 47 < 39 {
                        0
                    } else {
                        rng & mask
                    }
                }
            };
            pixels[y * stride + x] = value;
            tight.push(value);
        }
    }
    let lw = width.div_ceil(NL);
    let widths = crate::v1::lane_widths(width, lw);
    let transposed = crate::v1::step_major(&tight, width, lw, 0, rows);
    let tc = TokenCfg::v1(4, 1, depth);
    let mut weights = vec![1; tc.alphabet() as usize];
    enforce_run_cap(&tc, &mut weights);
    let freq = weights_to_freq(&weights).unwrap();
    let model = Model {
        tc,
        ctx_map: [0; CLASSES],
        freq: vec![freq; NCTX],
        weights: vec![weights; NCTX],
        prec: vec![1; NCTX],
        cross: false,
        ctx_shift: 0,
        seeds: [None; NCTX],
    };
    let table = EncodeTables::per_class(&model);
    let mut reference: [Vec<Sym>; NL] = Default::default();
    let mut actual: [Vec<Sym>; NL] = Default::default();
    for lane in 0..NL {
        lane_symbols(
            &transposed,
            None,
            lw,
            rows,
            widths[lane] as usize,
            lane,
            &tc,
            4,
            0,
            &mut reference[lane],
        );
        lane_symbols_rows_flat(
            &pixels,
            stride,
            lw,
            rows,
            widths[lane] as usize,
            lane,
            &tc,
            4,
            &mut actual[lane],
        );
        let fields = |s: &Sym| (s.step, s.tok, s.nb, s.extra);
        assert_eq!(
            actual[lane].iter().map(fields).collect::<Vec<_>>(),
            reference[lane].iter().map(fields).collect::<Vec<_>>(),
            "{width}/{rows}/{depth}/{pattern}/lane{lane}"
        );
        assert!(actual[lane].iter().all(|s| s.ctx == 0));
    }
    let mut old = Vec::new();
    let mut new = Vec::new();
    encode_lanes_tables(
        &table,
        &reference,
        crate::v1::active_lanes(&widths),
        &mut old,
    );
    encode_lanes_tables(&table, &actual, crate::v1::active_lanes(&widths), &mut new);
    assert_eq!(new, old, "full fixed-model state and word bytes");
}

#[test]
fn row_scanning_preserves_tokens_runs_and_complete_fixed_model_bytes() {
    for depth in [8, 16, 32] {
        for width in [1, 2, 15, 16, 17, 31, 64, 257] {
            for rows in [1, 2, 17] {
                for pattern in 0..3 {
                    compare(width, rows, depth, pattern, 7);
                }
            }
        }
        compare(1, RUN_CAP as usize + 7, depth, 0, 0);
    }
}
