//! Lossy color transforms.
//!
//! The forward transforms are encoder-only and not normative. The inverse
//! transforms are normative: every operation is a single IEEE f32 operation
//! in the order written (no fused multiply-add), so every implementation
//! produces the same bits.

/// Coding color space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LossyColor {
    /// JPEG XL's XYB (opsin) space; the third channel is coded as B - Y.
    Xyb = 0,
    /// YCoCg in float form (Co = R - B, Cg = G - (B + Co / 2)).
    YCoCg = 1,
}

impl LossyColor {
    /// Default per-channel plane scales (see the header's `color_scale`).
    pub(crate) fn default_scales(self) -> [f32; 3] {
        match self {
            Self::Xyb => [300.0, 2400.0, 300.0],
            Self::YCoCg => [1.0, 1.0, 1.0],
        }
    }
}

#[cfg(feature = "encode")]
const OPSIN: [[f64; 3]; 3] = [
    [0.30, 0.622, 0.078],
    [0.23, 0.692, 0.078],
    [
        0.243_422_689_245_478_2,
        0.204_767_444_244_968_2,
        0.551_809_866_509_553_6,
    ],
];
#[cfg(feature = "encode")]
pub(crate) const OPSIN_F32: [[f32; 3]; 3] = [
    [OPSIN[0][0] as f32, OPSIN[0][1] as f32, OPSIN[0][2] as f32],
    [OPSIN[1][0] as f32, OPSIN[1][1] as f32, OPSIN[1][2] as f32],
    [OPSIN[2][0] as f32, OPSIN[2][1] as f32, OPSIN[2][2] as f32],
];

/// Normative constants of the XYB inverse.
pub(crate) const OPSIN_BIAS: f32 = 0.003_793_073_3;
pub(crate) const CBRT_BIAS: f32 = 0.155_954_2;
pub(crate) const INV_OPSIN: [[f32; 3]; 3] = [
    [11.031_567, -9.866_944, -0.164_623],
    [-3.254_147_3, 4.418_770_3, -0.164_623],
    [-3.658_851_2, 2.712_923, 1.945_928_2],
];

/// sRGB8 -> linear light (encoder only).
#[allow(clippy::excessive_precision, clippy::unreadable_literal)]
#[cfg(feature = "encode")]
pub(crate) const SRGB8_TO_LINEAR: [f64; 256] = [
    0.0_f64,
    0.0003035269835488375_f64,
    0.000607053967097675_f64,
    0.0009105809506465125_f64,
    0.00121410793419535_f64,
    0.0015176349177441874_f64,
    0.001821161901293025_f64,
    0.0021246888848418626_f64,
    0.0024282158683907_f64,
    0.0027317428519395373_f64,
    0.003035269835488375_f64,
    0.003346535763899161_f64,
    0.003676507324047436_f64,
    0.004024717018496307_f64,
    0.004391442037410293_f64,
    0.004776953480693729_f64,
    0.005181516702338386_f64,
    0.005605391624202723_f64,
    0.006048833022857054_f64,
    0.006512090792594475_f64,
    0.006995410187265387_f64,
    0.007499032043226175_f64,
    0.008023192985384994_f64,
    0.008568125618069307_f64,
    0.009134058702220787_f64,
    0.00972121732023785_f64,
    0.010329823029626936_f64,
    0.010960094006488246_f64,
    0.011612245179743885_f64,
    0.012286488356915872_f64,
    0.012983032342173012_f64,
    0.013702083047289686_f64,
    0.014443843596092545_f64,
    0.01520851442291271_f64,
    0.01599629336550963_f64,
    0.016807375752887384_f64,
    0.017641954488384078_f64,
    0.018500220128379697_f64,
    0.019382360956935723_f64,
    0.0202885630566524_f64,
    0.021219010376003555_f64,
    0.02217388479338738_f64,
    0.02315336617811041_f64,
    0.024157632448504756_f64,
    0.02518685962736163_f64,
    0.026241221894849898_f64,
    0.027320891639074894_f64,
    0.028426039504420793_f64,
    0.0295568344378088_f64,
    0.030713443732993635_f64,
    0.03189603307301153_f64,
    0.033104766570885055_f64,
    0.03433980680868217_f64,
    0.03560131487502034_f64,
    0.03688945040110004_f64,
    0.0382043715953465_f64,
    0.03954623527673284_f64,
    0.04091519690685319_f64,
    0.042311410620809675_f64,
    0.043735029256973465_f64,
    0.04518620438567554_f64,
    0.046665086336880095_f64,
    0.04817182422688942_f64,
    0.04970656598412723_f64,
    0.05126945837404324_f64,
    0.052860647023180246_f64,
    0.05448027644244237_f64,
    0.05612849004960009_f64,
    0.05780543019106723_f64,
    0.0595112381629812_f64,
    0.06124605423161761_f64,
    0.06301001765316767_f64,
    0.06480326669290577_f64,
    0.06662593864377289_f64,
    0.06847816984440017_f64,
    0.07036009569659588_f64,
    0.07227185068231748_f64,
    0.07421356838014963_f64,
    0.07618538148130785_f64,
    0.07818742180518633_f64,
    0.08021982031446832_f64,
    0.0822827071298148_f64,
    0.08437621154414882_f64,
    0.08650046203654976_f64,
    0.08865558628577294_f64,
    0.09084171118340768_f64,
    0.09305896284668745_f64,
    0.0953074666309647_f64,
    0.09758734714186246_f64,
    0.09989872824711389_f64,
    0.10224173308810132_f64,
    0.10461648409110419_f64,
    0.10702310297826761_f64,
    0.10946171077829933_f64,
    0.1119324278369056_f64,
    0.11443537382697373_f64,
    0.11697066775851084_f64,
    0.11953842798834562_f64,
    0.12213877222960187_f64,
    0.12477181756095049_f64,
    0.12743768043564743_f64,
    0.1301364766903643_f64,
    0.13286832155381798_f64,
    0.13563332965520566_f64,
    0.13843161503245183_f64,
    0.14126329114027164_f64,
    0.14412847085805777_f64,
    0.14702726649759498_f64,
    0.14995978981060856_f64,
    0.15292615199615017_f64,
    0.1559264637078274_f64,
    0.1589608350608804_f64,
    0.162029375639111_f64,
    0.1651321945016676_f64,
    0.16826940018969075_f64,
    0.1714411007328226_f64,
    0.17464740365558504_f64,
    0.17788841598362912_f64,
    0.18116424424986022_f64,
    0.184474994500441_f64,
    0.18782077230067787_f64,
    0.19120168274079138_f64,
    0.1946178304415758_f64,
    0.19806931955994886_f64,
    0.20155625379439707_f64,
    0.20507873639031693_f64,
    0.20863687014525575_f64,
    0.21223075741405523_f64,
    0.21586050011389926_f64,
    0.2195261997292692_f64,
    0.2232279573168085_f64,
    0.22696587351009836_f64,
    0.23074004852434915_f64,
    0.23455058216100522_f64,
    0.238397573812271_f64,
    0.24228112246555486_f64,
    0.24620132670783548_f64,
    0.25015828472995344_f64,
    0.25415209433082675_f64,
    0.2581828529215958_f64,
    0.26225065752969623_f64,
    0.26635560480286247_f64,
    0.2704977910130658_f64,
    0.27467731206038465_f64,
    0.2788942634768104_f64,
    0.2831487404299921_f64,
    0.2874408377269175_f64,
    0.29177064981753587_f64,
    0.2961382707983211_f64,
    0.3005437944157765_f64,
    0.3049873140698863_f64,
    0.30946892281750854_f64,
    0.31398871337571754_f64,
    0.31854677812509186_f64,
    0.32314320911295075_f64,
    0.3277780980565422_f64,
    0.33245153634617935_f64,
    0.33716361504833037_f64,
    0.3419144249086609_f64,
    0.3467040563550296_f64,
    0.35153259950043936_f64,
    0.3564001441459435_f64,
    0.3613067797835095_f64,
    0.3662525955988395_f64,
    0.3712376804741491_f64,
    0.3762621229909065_f64,
    0.38132601143253014_f64,
    0.386429433787049_f64,
    0.39157247774972326_f64,
    0.39675523072562685_f64,
    0.4019777798321958_f64,
    0.4072402119017367_f64,
    0.41254261348390375_f64,
    0.4178850708481375_f64,
    0.4232676699860717_f64,
    0.4286904966139066_f64,
    0.43415363617474895_f64,
    0.4396571738409188_f64,
    0.44520119451622786_f64,
    0.45078578283822346_f64,
    0.45641102318040466_f64,
    0.4620769996544071_f64,
    0.467783796112159_f64,
    0.47353149614800955_f64,
    0.4793201831008268_f64,
    0.4851499400560704_f64,
    0.4910208498478356_f64,
    0.4969329950608704_f64,
    0.5028864580325687_f64,
    0.5088813208549338_f64,
    0.5149176653765214_f64,
    0.5209955732043543_f64,
    0.5271151257058131_f64,
    0.5332764040105052_f64,
    0.5394794890121072_f64,
    0.5457244613701866_f64,
    0.5520114015120001_f64,
    0.5583403896342679_f64,
    0.5647115057049292_f64,
    0.5711248294648731_f64,
    0.5775804404296506_f64,
    0.5840784178911641_f64,
    0.5906188409193369_f64,
    0.5972017883637634_f64,
    0.6038273388553378_f64,
    0.6104955708078648_f64,
    0.6172065624196511_f64,
    0.6239603916750761_f64,
    0.6307571363461468_f64,
    0.6375968739940326_f64,
    0.6444796819705821_f64,
    0.6514056374198242_f64,
    0.6583748172794485_f64,
    0.665387298282272_f64,
    0.6724431569576875_f64,
    0.6795424696330938_f64,
    0.6866853124353135_f64,
    0.6938717612919899_f64,
    0.7011018919329731_f64,
    0.7083757798916868_f64,
    0.7156935005064807_f64,
    0.7230551289219693_f64,
    0.7304607400903537_f64,
    0.7379104087727308_f64,
    0.7454042095403874_f64,
    0.7529422167760779_f64,
    0.7605245046752924_f64,
    0.768151147247507_f64,
    0.7758222183174236_f64,
    0.7835377915261935_f64,
    0.7912979403326302_f64,
    0.799102738014409_f64,
    0.8069522576692516_f64,
    0.8148465722161012_f64,
    0.8227857543962835_f64,
    0.8307698767746546_f64,
    0.83879901174074_f64,
    0.846873231509858_f64,
    0.8549926081242338_f64,
    0.8631572134541023_f64,
    0.8713671191987972_f64,
    0.8796223968878317_f64,
    0.8879231178819663_f64,
    0.8962693533742664_f64,
    0.9046611743911496_f64,
    0.9130986517934192_f64,
    0.9215818562772946_f64,
    0.9301108583754237_f64,
    0.938685728457888_f64,
    0.9473065367331999_f64,
    0.9559733532492861_f64,
    0.9646862478944651_f64,
    0.9734452903984125_f64,
    0.9822505503331171_f64,
    0.9911020971138298_f64,
    1.0_f64,
];

/// Round to nearest, ties to even, for 0 <= v <= 2^22 (exact via the f32 adder).
#[inline(always)]
pub(crate) fn rne_pos(v: f32) -> f32 {
    (v + 8_388_608.0) - 8_388_608.0
}

/// Encoder: sRGB8 pixel -> coded planes (scaled).
#[cfg(all(test, feature = "encode"))]
#[inline]
pub(crate) fn forward(color: LossyColor, scale: [f32; 3], p: [u8; 3]) -> [f32; 3] {
    match color {
        LossyColor::YCoCg => {
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            let co = r - b;
            let t = b + co * 0.5;
            let cg = g - t;
            [t + cg * 0.5, co, cg]
        }
        LossyColor::Xyb => {
            let l = [0, 1, 2].map(|i| SRGB8_TO_LINEAR[p[i] as usize] as f32);
            forward_xyb_linear(scale, l)
        }
    }
}

pub(crate) use super::srgb_table::{SRGB8_LOG_BASE, SRGB8_LOG_N, SRGB8_LOG8K};

/// Encoder: linear RGB (1.0 = SDR white; HDR above) -> scaled XYB planes.
/// Same operations as a SIMD encoder kernel would use (magetypes cbrt_midp).
#[cfg(all(test, feature = "encode"))]
#[inline]
pub(crate) fn forward_xyb_linear(scale: [f32; 3], l: [f32; 3]) -> [f32; 3] {
    let mut lms = [0.0f32; 3];
    for (i, v) in lms.iter_mut().enumerate() {
        let o = &OPSIN_F32[i];
        let m = o[0] * l[0] + o[1] * l[1] + o[2] * l[2] + OPSIN_BIAS;
        *v = magetypes::nostd_math::cbrt_midp_f32(m.max(0.0)) - CBRT_BIAS;
    }
    let x = 0.5 * (lms[0] - lms[1]);
    let y = 0.5 * (lms[0] + lms[1]);
    [y * scale[0], x * scale[1], (lms[2] - y) * scale[2]]
}

/// Normative XYB inverse to linear RGB (no transfer function, no clamp).
#[inline]
pub(crate) fn inverse_xyb_linear(inv_scale: [f32; 3], v: [f32; 3]) -> [f32; 3] {
    let y = v[0] * inv_scale[0];
    let x = v[1] * inv_scale[1];
    let b = v[2] * inv_scale[2] + y;
    let l = (y + x + CBRT_BIAS).clamp(-64.0, 64.0);
    let m = (y - x + CBRT_BIAS).clamp(-64.0, 64.0);
    let s = (b + CBRT_BIAS).clamp(-64.0, 64.0);
    let l3 = l * l * l - OPSIN_BIAS;
    let m3 = m * m * m - OPSIN_BIAS;
    let s3 = s * s * s - OPSIN_BIAS;
    let k = &INV_OPSIN;
    [0, 1, 2].map(|i| k[i][0] * l3 + k[i][1] * m3 + k[i][2] * s3)
}

/// f32 -> f16 bits, round to nearest even (overflow to infinity, NaN kept
/// as a quiet NaN).
pub(crate) fn f32_to_f16_rne(f: f32) -> u16 {
    let b = f.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32;
    let man = b & 0x7F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rem = m & ((1 << shift) - 1);
        let mut r = m >> shift;
        if rem > half || (rem == half && r & 1 == 1) {
            r += 1;
        }
        return sign | r as u16;
    }
    let rem = man & 0x1FFF;
    let mut r = ((e as u32) << 10) | (man >> 13);
    if rem > 0x1000 || (rem == 0x1000 && r & 1 == 1) {
        r += 1;
    }
    sign | r as u16
}

/// Normative YCoCg inverse to 16-bit code values.
#[inline]
pub(crate) fn inverse_ycocg(inv_scale: [f32; 3], v: [f32; 3]) -> [f32; 3] {
    let y = v[0] * inv_scale[0];
    let co = v[1] * inv_scale[1];
    let cg = v[2] * inv_scale[2];
    let t = y - cg * 0.5;
    let g = cg + t;
    let b = t - co * 0.5;
    let r = b + co;
    [r, g, b]
}

#[inline]
pub(crate) fn inverse_ycocg16(inv_scale: [f32; 3], v: [f32; 3]) -> [u16; 3] {
    inverse_ycocg(inv_scale, v).map(|c| rne_pos(c.clamp(0.0, 65535.0)) as u16)
}

/// Linear light -> sRGB8, normative: a table indexed by the f32 bits of
/// clamp(lin, 0, 1) shifted right by 14 (exponent and 9 mantissa bits),
/// offset by [`SRGB8_LOG_BASE`] and clamped to the table. Integer-only, so
/// every backend agrees; within 0.1 code of the exact transfer function.
#[inline(always)]
pub(crate) fn lin_to_srgb8(c: f32) -> u8 {
    let b = c.clamp(0.0, 1.0).to_bits() as i32;
    let i = ((b >> 14) - SRGB8_LOG_BASE).clamp(0, SRGB8_LOG_N - 1);
    SRGB8_LOG8K[i as usize & 8191]
}

/// Normative inverse for one pixel: coded planes -> sRGB8.
#[inline]
pub(crate) fn inverse_px(color: LossyColor, inv_scale: [f32; 3], v: [f32; 3]) -> [u8; 3] {
    match color {
        LossyColor::YCoCg => {
            let y = v[0] * inv_scale[0];
            let co = v[1] * inv_scale[1];
            let cg = v[2] * inv_scale[2];
            let t = y - cg * 0.5;
            let g = cg + t;
            let b = t - co * 0.5;
            let r = b + co;
            [r, g, b].map(|c| rne_pos(c.clamp(0.0, 255.0)) as u8)
        }
        LossyColor::Xyb => inverse_xyb_linear(inv_scale, v).map(lin_to_srgb8),
    }
}

/// Normative inverse of a signalled linear color transform (Lossy 0.4) for
/// one pixel: coded planes -> sRGB8. `m[j] = [M_j0, M_j1, M_j2, O_j]`;
/// each output is `M_j0*c0 + M_j1*c1 + M_j2*c2 + O_j` in that order (no FMA).
#[inline]
pub(crate) fn inverse_linear_px(m: &[[f32; 4]; 3], inv_scale: [f32; 3], v: [f32; 3]) -> [u8; 3] {
    let c = [
        v[0] * inv_scale[0],
        v[1] * inv_scale[1],
        v[2] * inv_scale[2],
    ];
    core::array::from_fn(|j| {
        let t = m[j][0] * c[0] + m[j][1] * c[1] + m[j][2] * c[2] + m[j][3];
        rne_pos(t.clamp(0.0, 255.0)) as u8
    })
}

/// Normative decode matrices of the signalled transform presets (Lossy 0.4),
/// on 0..255 code values: BT.601 and BT.709 YCbCr inverses. Identity is the
/// identity; the YCoCg preset is the lifting in [`inverse_px`], not a matrix.
pub(crate) const TRANSFORM_IDENTITY: [[f32; 4]; 3] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];
pub(crate) const TRANSFORM_BT601: [[f32; 4]; 3] = [
    [1.0, 0.0, 1.402, -179.456],
    [1.0, -0.344_136, -0.714_136, 135.458_82],
    [1.0, 1.772, 0.0, -226.816],
];
pub(crate) const TRANSFORM_BT709: [[f32; 4]; 3] = [
    [1.0, 0.0, 1.574_8, -201.574_4],
    [1.0, -0.187_324, -0.468_124, 83.897_34],
    [1.0, 1.855_6, 0.0, -237.516_8],
];

/// Encoder forward of a decode matrix `[M | O]`: `A = M^-1`, `B = -A O`,
/// computed in f64 from the normative f32 values. The coded planes are
/// `A * rgb + B` (then scaled). A singular matrix is rejected.
#[cfg(feature = "encode")]
pub(crate) fn forward_linear(decode: &[[f32; 4]; 3]) -> Option<[[f32; 4]; 3]> {
    let m: [[f64; 3]; 3] = core::array::from_fn(|j| core::array::from_fn(|i| decode[j][i] as f64));
    let o: [f64; 3] = core::array::from_fn(|j| decode[j][3] as f64);
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if !det.is_finite() || det == 0.0 {
        return None;
    }
    let inv = |r: usize, c: usize| {
        let (a, b, d, e) = match (r, c) {
            (0, 0) => (m[1][1], m[1][2], m[2][1], m[2][2]),
            (0, 1) => (m[1][0], m[1][2], m[2][0], m[2][2]),
            (0, 2) => (m[1][0], m[1][1], m[2][0], m[2][1]),
            (1, 0) => (m[0][1], m[0][2], m[2][1], m[2][2]),
            (1, 1) => (m[0][0], m[0][2], m[2][0], m[2][2]),
            (1, 2) => (m[0][0], m[0][1], m[2][0], m[2][1]),
            (2, 0) => (m[0][1], m[0][2], m[1][1], m[1][2]),
            (2, 1) => (m[0][0], m[0][2], m[1][0], m[1][2]),
            _ => (m[0][0], m[0][1], m[1][0], m[1][1]),
        };
        // cofactor / det, transposed indexing done by the caller's (r, c)
        let cof = a * e - b * d;
        let s = if (r + c).is_multiple_of(2) { 1.0 } else { -1.0 };
        s * cof / det
    };
    let a: [[f64; 3]; 3] = core::array::from_fn(|i| core::array::from_fn(|j| inv(j, i)));
    let mut out = [[0.0f32; 4]; 3];
    for j in 0..3 {
        for i in 0..3 {
            out[j][i] = a[j][i] as f32;
        }
        let b = -(a[j][0] * o[0] + a[j][1] * o[1] + a[j][2] * o[2]);
        out[j][3] = b as f32;
        if !out[j].iter().all(|v| v.is_finite()) {
            return None;
        }
    }
    Some(out)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    /// The signalled transform presets: the encoder's f64 forward of the
    /// normative decode matrix round-trips every sampled RGB8 color to
    /// within one code (exact for identity).
    #[test]
    fn linear_transform_presets_round_trip_rgb8() {
        for (name, decode) in [
            ("identity", TRANSFORM_IDENTITY),
            ("bt601", TRANSFORM_BT601),
            ("bt709", TRANSFORM_BT709),
        ] {
            let fwd = forward_linear(&decode).unwrap();
            let mut worst = 0i32;
            for r in (0..256).step_by(3) {
                for g in (0..256).step_by(5) {
                    for b in (0..256).step_by(7) {
                        let rgb = [r as f32, g as f32, b as f32];
                        let coded: [f32; 3] = core::array::from_fn(|j| {
                            fwd[j][0] * rgb[0] + fwd[j][1] * rgb[1] + fwd[j][2] * rgb[2] + fwd[j][3]
                        });
                        let back = inverse_linear_px(&decode, [1.0; 3], coded);
                        for c in 0..3 {
                            worst = worst.max((back[c] as i32 - [r, g, b][c]).abs());
                        }
                    }
                }
            }
            let bound = if name == "identity" { 0 } else { 1 };
            assert!(worst <= bound, "{name}: worst {worst}");
        }
        // A singular decode matrix has no forward.
        assert!(forward_linear(&[[0.0; 4]; 3]).is_none());
    }

    #[test]
    fn inverse_opsin_is_inverse() {
        for i in 0..3 {
            for j in 0..3 {
                let v: f64 = (0..3).map(|k| INV_OPSIN[i][k] as f64 * OPSIN[k][j]).sum();
                let e = if i == j { 1.0 } else { 0.0 };
                assert!((v - e).abs() < 1e-5, "{i},{j}: {v}");
            }
        }
    }

    #[test]
    fn f16_rne_roundtrip_and_ties() {
        for h in 0..=u16::MAX {
            let f = f32::from_bits(crate::mapping::f16_to_f32_bits(h));
            if f.is_nan() {
                assert!(f32_to_f16_rne(f) & 0x7C00 == 0x7C00);
                continue;
            }
            assert_eq!(f32_to_f16_rne(f), h, "{h:#x}");
            // Midpoint to the next value rounds to the even one.
            if h & 0x7FFF < 0x7BFF {
                let g = f32::from_bits(crate::mapping::f16_to_f32_bits(h + 1));
                let mid = (f as f64 + g as f64) / 2.0;
                let even = if h & 1 == 0 { h } else { h + 1 };
                assert_eq!(f32_to_f16_rne(mid as f32), even, "{h:#x}");
            }
        }
    }

    #[test]
    fn ycocg_rgb8_exhaustive() {
        // Exercise the actual dispatched encoder color kernel, then the
        // normative inverse. No DCT/quantization: every RGB8 color must survive.
        // Co is signed, Cg has half steps and Y has quarter steps; none are u8.
        for r in 0..256 {
            for g in 0..256 {
                let mut p0 = [r as f32; 256];
                let mut p1 = [g as f32; 256];
                let mut p2 = core::array::from_fn::<_, 256, _>(|b| b as f32);
                super::super::enc_simd::color_rows(
                    false, [1.0; 3], &OPSIN_F32, OPSIN_BIAS, CBRT_BIAS, &mut p0, &mut p1, &mut p2,
                );
                for b in 0..256 {
                    let rgb = [r as u8, g as u8, b as u8];
                    let coded = [p0[b], p1[b], p2[b]];
                    assert_eq!(coded, forward(LossyColor::YCoCg, [1.0; 3], rgb));
                    assert_eq!(inverse_px(LossyColor::YCoCg, [1.0; 3], coded), rgb);
                }
            }
        }
    }

    #[test]
    fn roundtrip_without_quantization() {
        for color in [LossyColor::Xyb, LossyColor::YCoCg] {
            let sc = color.default_scales();
            let inv = sc.map(|s| 1.0 / s);
            let mut worst = 0;
            for r in (0..256).step_by(5) {
                for g in (0..256).step_by(7) {
                    for b in (0..256).step_by(3) {
                        let p = [r as u8, g as u8, b as u8];
                        let o = inverse_px(color, inv, forward(color, sc, p));
                        for c in 0..3 {
                            worst = worst.max((o[c] as i32 - p[c] as i32).abs());
                        }
                    }
                }
            }
            assert!(
                worst <= if color == LossyColor::Xyb { 1 } else { 0 },
                "{color:?} {worst}"
            );
        }
    }
}
