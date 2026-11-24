//! Seven-output AAN candidate, derived from the existing portable reconstruction.
//! Both passes omit their unused terminal result; dummy lanes are zeroed before
//! transposition. Blend still needs all eight outputs and uses the full kernel.
use super::*;
use archmage::prelude::*;

#[magetypes(define(f32x8, i32x8), v3, neon, wasm128, scalar)]
fn row_impl(
    token: Token,
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    bx0: usize,
    bx1: usize,
    dq: &[[f32; 64]; 3],
    aq: &[f32],
    planes: &mut [f32],
    pw: usize,
) {
    let r2 = f32x8::splat_t(token, dct::AAN_R2);
    let c1 = f32x8::splat_t(token, dct::AAN_C1);
    let c2 = f32x8::splat_t(token, dct::AAN_C2);
    let c3 = f32x8::splat_t(token, dct::AAN_C3);
    let pass = |v: &mut [f32x8; 8]| {
        let tmp10 = v[0] + v[4];
        let tmp11 = v[0] - v[4];
        let tmp13 = v[2] + v[6];
        let tmp12 = (v[2] - v[6]) * r2 - tmp13;
        let e0 = tmp10 + tmp13;
        let e3 = tmp10 - tmp13;
        let e1 = tmp11 + tmp12;
        let e2 = tmp11 - tmp12;
        let z13 = v[5] + v[3];
        let z10 = v[5] - v[3];
        let z11 = v[1] + v[7];
        let z12 = v[1] - v[7];
        let t7 = z11 + z13;
        let t11 = (z11 - z13) * r2;
        let z5 = (z10 + z12) * c1;
        let t10 = z5 - z12 * c2;
        let t12 = z5 - z10 * c3;
        let t6 = t12 - t7;
        let t5 = t11 - t6;
        let t4 = t10 - t5;
        v[0] = e0 + t7;
        v[7] = f32x8::splat_t(token, 0.0);
        v[1] = e1 + t6;
        v[6] = e1 - t6;
        v[2] = e2 + t5;
        v[5] = e2 - t5;
        v[3] = e3 + t4;
        v[4] = e3 - t4;
    };
    for bx in bx0..bx1 {
        for ch in 0..3 {
            let b = r * bw + bx;
            let base = ch * 8 * pw + (bx - bx0) * 8;
            let qb = &mut q[(b * 3 + ch) * 64..][..64];
            let factors = aq_dequant(&dq[ch], aq.get(bx).copied());
            if eobs[b * 3 + ch] == 0 {
                let v = qb[0] as f32 * factors[0];
                qb[0] = 0;
                for y in 0..7 {
                    planes[base + y * pw..][..8].fill(v);
                }
                continue;
            }
            let mut values: [f32x8; 8] = core::array::from_fn(|y| {
                let qi: &[i32; 8] = qb[y * 8..][..8].try_into().unwrap();
                let d: &[f32; 8] = factors[y * 8..][..8].try_into().unwrap();
                f32x8::from_i32x8_t(token, i32x8::load_t(token, qi)) * f32x8::load_t(token, d)
            });
            qb.fill(0);
            pass(&mut values);
            f32x8::transpose_8x8(&mut values);
            pass(&mut values);
            f32x8::transpose_8x8(&mut values);
            for (y, row) in values.iter().enumerate().take(7) {
                let dst: &mut [f32; 8] = (&mut planes[base + y * pw..][..8]).try_into().unwrap();
                row.store(dst);
            }
        }
    }
}

pub(super) fn row(
    q: &mut [i32],
    eobs: &[u8],
    r: usize,
    bw: usize,
    bx0: usize,
    bx1: usize,
    dq: &[[f32; 64]; 3],
    aq: &[f32],
    planes: &mut [f32],
    pw: usize,
) {
    if crate::v1::force_scalar() {
        return row_impl_scalar(ScalarToken, q, eobs, r, bw, bx0, bx1, dq, aq, planes, pw);
    }
    #[cfg(all(target_arch = "x86_64", feature = "avx512"))]
    if !crate::v1::force_avx2()
        && let Some(t) = X64V4Token::summon()
    {
        return super::pruned_v4::recon_block_row::<false>(
            t, q, eobs, r, bw, bx0, bx1, dq, aq, None, planes, pw,
        );
    }
    incant!(
        row_impl(q, eobs, r, bw, bx0, bx1, dq, aq, planes, pw),
        [v3, neon, wasm128, scalar]
    );
}
