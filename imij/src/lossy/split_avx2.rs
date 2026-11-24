//! AVX2 unpack of the split-Huffman mode's plain bit streams, one group
//! per step in one xmm: the group's field (<= 32 bits) broadcast and
//! shifted by `[0, w, 2w, 3w]` gives its four magnitudes, a movemask its
//! nonzeros, a 4-bit table deposits its sign bits, and a masked negate
//! applies them.
//! Bit-identical to the scalar `split_group`, which handles wider groups.

use super::coef::{PK_GROUPS, ZIGZAG};
use super::rawbits;
use archmage::prelude::*;

/// `DEP4[nzm << 4 | s]`: sign bits `s` deposited on the set bits of the
/// 4-bit nonzero mask `nzm` (bit i of `s` -> i-th set bit).
static DEP4: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let (m, sb) = (i >> 4, i & 15);
        let (mut out, mut k, mut j) = (0u8, 0, 0);
        while j < 4 {
            if m & (1 << j) != 0 {
                out |= (((sb >> k) & 1) << j) as u8;
                k += 1;
            }
            j += 1;
        }
        t[i] = out;
        i += 1;
    }
    t
};

/// All-ones lanes for the set bits of a 4-bit mask.
static NEGM: [[i32; 4]; 16] = {
    let mut t = [[0i32; 4]; 16];
    let mut m = 0;
    while m < 16 {
        let mut j = 0;
        while j < 4 {
            t[m][j] = if m & (1 << j) != 0 { -1 } else { 0 };
            j += 1;
        }
        m += 1;
    }
    t
};

/// Raster positions of group gi's coefficients (zigzag 1 + 4 gi + j).
static ZZG: [[u8; 4]; PK_GROUPS] = {
    let mut t = [[0u8; 4]; PK_GROUPS];
    let mut g = 0;
    while g < PK_GROUPS {
        let mut j = 0;
        while j < 4 {
            let z = 1 + 4 * g + j;
            t[g][j] = if z < 64 { ZIGZAG[z] } else { 0 };
            j += 1;
        }
        g += 1;
    }
    t
};

/// Unpacks every block-channel; returns the final (magnitude, sign) bit
/// offsets for the overrun check.
#[arcane(import_intrinsics)]
pub(crate) fn unpack(
    _t: X64V3Token,
    mag: &[u8],
    sgn: &[u8],
    nbc: usize,
    counts: &[u8],
    wid: &[u8],
    q: &mut [i32],
) -> (u64, u64) {
    let jmul = _mm_set_epi32(3, 2, 1, 0);
    let zero = _mm_setzero_si128();
    let (mut om, mut os) = (0u64, 0u64);
    for bc in 0..nbc {
        let n = (counts[bc] as usize).min(PK_GROUPS);
        if n == 0 {
            continue;
        }
        let ws: &[u8; PK_GROUPS] = wid[bc * PK_GROUPS..][..PK_GROUPS].try_into().unwrap();
        let qb: &mut [i32; 64] = (&mut q[bc * 64..bc * 64 + 64]).try_into().unwrap();
        for gi in 0..n {
            let w = ws[gi] as u32;
            let len = 4 - (gi == PK_GROUPS - 1) as u32;
            let field = len * w;
            if field > 32 {
                (om, os) = super::split_group(mag, sgn, gi, w, qb, om, os);
                continue;
            }
            let win = (rawbits::window(mag, (om >> 3) as usize) >> (om & 7)) as u32;
            om += field as u64;
            let v = _mm_set1_epi32(win as i32);
            let sh = _mm_mullo_epi32(_mm_set1_epi32(w as i32), jmul);
            let mask = _mm_set1_epi32(((1u64 << w) - 1) as i32);
            let mut m = _mm_and_si128(_mm_srlv_epi32(v, sh), mask);
            if len == 3 {
                m = _mm_blend_epi32::<0b1000>(m, zero);
            }
            let nzm = _mm_movemask_ps(_mm_castsi128_ps(_mm_cmpgt_epi32(m, zero))) as usize;
            let nz = nzm.count_ones();
            let s = (rawbits::window(sgn, (os >> 3) as usize) >> (os & 7)) as usize & 15;
            os += nz as u64;
            let nm = _mm_loadu_si128(&NEGM[DEP4[(nzm << 4) | s] as usize]);
            let r = _mm_sub_epi32(_mm_xor_si128(m, nm), nm);
            let mut a = [0i32; 4];
            _mm_storeu_si128(&mut a, r);
            let zz = &ZZG[gi];
            for j in 0..len as usize {
                qb[zz[j] as usize & 63] = a[j];
            }
        }
    }
    (om, os)
}
