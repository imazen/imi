//! AVX-512 (v4x: VBMI byte permutes) unpack of the split-Huffman mode's
//! plain bit streams, one block-channel per iteration with its 16 groups
//! in the lanes of one zmm: field sizes, an in-register prefix sum for the
//! bit offsets, one 64-byte load of the block-channel's magnitude bits,
//! each group's 64-bit window cut out of it with `vpermb` (no per-lane
//! memory round trip), four extractions (coefficient j of every group),
//! signs deposited with `pdep`/`pext`, and one permutation to raster
//! order. Bit-identical to the scalar `split_unpack`; a block-channel with
//! a group wider than one window (len * w > 57) or more than 448 bits of
//! fields takes the scalar path.

use super::coef::{PK_GROUPS, ZIGZAG};
use super::entropy_avx512::st16;
use super::rawbits;
use archmage::prelude::*;

/// For raster position r: source register (j = coefficient within the
/// group) and lane (group) of its zigzag position, or DC.
struct Perm {
    /// Per output register k: indices into (F0 | F1) and (F2 | F3).
    idx: [[u32; 16]; 4],
    /// Per output register: lanes taken from (F2 | F3).
    hi: [u16; 4],
    /// Per output register: lanes that are not DC.
    ac: [u16; 4],
}

const PERM: Perm = {
    let mut idx = [[0u32; 16]; 4];
    let mut hi = [0u16; 4];
    let mut ac = [0u16; 4];
    let mut z = 1;
    while z < 64 {
        let r = ZIGZAG[z] as usize;
        let (gi, j) = ((z - 1) / 4, (z - 1) % 4);
        let (k, m) = (r / 16, r % 16);
        idx[k][m] = (gi + 16 * (j & 1)) as u32;
        if j >= 2 {
            hi[k] |= 1 << m;
        }
        ac[k] |= 1 << m;
        z += 1;
    }
    Perm { idx, hi, ac }
};

/// 64 bytes of `buf` from byte `i`, zero past the end.
#[inline(always)]
fn load64(buf: &[u8], i: usize) -> [u8; 64] {
    if let Some(b) = buf.get(i..i + 64) {
        b.try_into().unwrap()
    } else {
        let mut t = [0u8; 64];
        if i < buf.len() {
            t[..buf.len() - i].copy_from_slice(&buf[i..]);
        }
        t
    }
}

/// Unpacks every block-channel; returns the final (magnitude, sign) bit
/// offsets for the overrun check.
#[arcane(import_intrinsics)]
pub(crate) fn unpack(
    t: X64V4xToken,
    mag: &[u8],
    sgn: &[u8],
    nbc: usize,
    counts: &[u8],
    wid: &[u8],
    q: &mut [i32],
) -> (u64, u64) {
    let lanes: [u32; 16] = core::array::from_fn(|i| i as u32);
    let lane = _mm512_loadu_si512(&lanes);
    let lens: [u32; 16] = core::array::from_fn(|i| if i == PK_GROUPS - 1 { 3 } else { 4 });
    let lenv = _mm512_loadu_si512(&lens);
    let pidx = PERM.idx.map(|a| _mm512_loadu_si512(&a));
    let zero = _mm512_setzero_si512();
    let one64 = _mm512_set1_epi64(1);
    let (mut om, mut os) = (0u64, 0u64);
    for bc in 0..nbc {
        let n = (counts[bc] as u32).min(PK_GROUPS as u32);
        if n == 0 {
            continue;
        }
        let wb: &[u8; 16] = wid[bc * PK_GROUPS..][..PK_GROUPS].try_into().unwrap();
        let valid = _mm512_cmplt_epu32_mask(lane, _mm512_set1_epi32(n as i32));
        let wv = _mm512_maskz_mov_epi32(valid, _mm512_cvtepu8_epi32(_mm_loadu_si128(wb)));
        let field = _mm512_mullo_epi32(wv, lenv);
        // Inclusive prefix sum of the field sizes, then exclusive offsets.
        let mut inc = field;
        inc = _mm512_add_epi32(inc, _mm512_alignr_epi32::<15>(inc, zero));
        inc = _mm512_add_epi32(inc, _mm512_alignr_epi32::<14>(inc, zero));
        inc = _mm512_add_epi32(inc, _mm512_alignr_epi32::<12>(inc, zero));
        inc = _mm512_add_epi32(inc, _mm512_alignr_epi32::<8>(inc, zero));
        let total = st16(t.v4(), inc)[15] as u64;
        if _mm512_cmpgt_epu32_mask(field, _mm512_set1_epi32(57)) != 0 || (om & 7) + total > 448 {
            let qb: &mut [i32; 64] = (&mut q[bc * 64..bc * 64 + 64]).try_into().unwrap();
            let (m2, s2) = super::split_unpack_bc(mag, sgn, n as usize, wb, qb, om, os);
            (om, os) = (m2, s2);
            continue;
        }
        // The block-channel's fields, from byte om / 8, in one register.
        let base = (om >> 3) as usize;
        let bytes = load64(mag, base);
        let v = _mm512_loadu_si512(&bytes);
        let rel = _mm512_add_epi32(
            _mm512_sub_epi32(inc, field),
            _mm512_set1_epi32((om & 7) as i32),
        );
        let boff = _mm512_srli_epi32::<3>(rel);
        let bsh = _mm512_and_si512(rel, _mm512_set1_epi32(7));
        // Byte k of qword lane i = byte boff[i] + k.
        let ramp = _mm512_set1_epi64(0x0706_0504_0302_0100);
        let spread = _mm512_set1_epi64(0x0101_0101_0101_0101);
        let win = [0, 1].map(|h| {
            let b = if h == 0 {
                _mm512_cvtepu32_epi64(_mm512_castsi512_si256(boff))
            } else {
                _mm512_cvtepu32_epi64(_mm512_extracti64x4_epi64::<1>(boff))
            };
            let sh = if h == 0 {
                _mm512_cvtepu32_epi64(_mm512_castsi512_si256(bsh))
            } else {
                _mm512_cvtepu32_epi64(_mm512_extracti64x4_epi64::<1>(bsh))
            };
            let idx = _mm512_add_epi8(_mm512_mullo_epi64(b, spread), ramp);
            _mm512_srlv_epi64(_mm512_permutexvar_epi8(idx, v), sh)
        });
        let (wl, wh) = (win[0], win[1]);
        om += total;
        // Widths as u64 halves.
        let w64 = [
            _mm512_cvtepu32_epi64(_mm512_castsi512_si256(wv)),
            _mm512_cvtepu32_epi64(_mm512_extracti64x4_epi64::<1>(wv)),
        ];
        let mask64 = w64.map(|w| _mm512_sub_epi64(_mm512_sllv_epi64(one64, w), one64));
        // F[j]: coefficient j of each group (lane = group).
        let f: [__m512i; 4] = core::array::from_fn(|j| {
            let jv = _mm512_set1_epi64(j as i64);
            let a = _mm512_and_si512(
                _mm512_srlv_epi64(wl, _mm512_mullo_epi64(w64[0], jv)),
                mask64[0],
            );
            let b = _mm512_and_si512(
                _mm512_srlv_epi64(wh, _mm512_mullo_epi64(w64[1], jv)),
                mask64[1],
            );
            let v = _mm512_inserti64x4::<1>(
                _mm512_castsi256_si512(_mm512_cvtepi64_epi32(a)),
                _mm512_cvtepi64_epi32(b),
            );
            // Group 15 has three coefficients.
            if j == 3 {
                _mm512_maskz_mov_epi32(0x7FFF, v)
            } else {
                v
            }
        });
        // Nonzero mask in zigzag order (bit 1 + 4 gi + j), signs deposited
        // on it (the i-th nonzero takes sign bit i).
        let nzm: [u64; 4] = core::array::from_fn(|j| _mm512_test_epi32_mask(f[j], f[j]) as u64);
        let rep = 0x1111_1111_1111_1111u64;
        let zz = (_pdep_u64(nzm[0], rep)
            | _pdep_u64(nzm[1], rep << 1)
            | _pdep_u64(nzm[2], rep << 2)
            | _pdep_u64(nzm[3], rep << 3))
            << 1;
        let nz = zz.count_ones() as u64;
        let sb = (os >> 3) as usize;
        let sh = os & 7;
        let lo = rawbits::window(sgn, sb) >> sh;
        let hi = if sh == 0 {
            0
        } else {
            rawbits::window(sgn, sb + 8) << (64 - sh)
        };
        let sbits = (lo | hi) & if nz == 64 { u64::MAX } else { (1u64 << nz) - 1 };
        os += nz;
        let neg = _pdep_u64(sbits, zz) >> 1;
        let f: [__m512i; 4] = core::array::from_fn(|j| {
            let nj = _pext_u64(neg, rep << j) as __mmask16;
            _mm512_mask_sub_epi32(f[j], nj, zero, f[j])
        });
        // To raster order and store.
        let qb: &mut [i32; 64] = (&mut q[bc * 64..bc * 64 + 64]).try_into().unwrap();
        for k in 0..4 {
            let a = _mm512_permutex2var_epi32(f[0], pidx[k], f[1]);
            let b = _mm512_permutex2var_epi32(f[2], pidx[k], f[3]);
            let r = _mm512_maskz_mov_epi32(PERM.ac[k], _mm512_mask_mov_epi32(a, PERM.hi[k], b));
            let o: &mut [i32; 16] = (&mut qb[k * 16..k * 16 + 16]).try_into().unwrap();
            _mm512_storeu_si512(o, r);
        }
    }
    (om, os)
}
