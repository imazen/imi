// Copyright (c) 2018-2022, The rav1e contributors. All rights reserved.
// BSD-2-Clause + Alliance for Open Media Patent License 1.0.
// Adapted from imazen/zenrav1e 1ac9ab56f664d88b133df7ee1aa75387116c2eb7.
// Retained terms: integer_study_licenses/{LICENSE,PATENTS}.rav1e.
// Independent scalar oracle: keep its recursive structure separate from the
// SIMD stage schedule. Use i64 products so accidental SIMD wrap cannot agree.
const INV_COS_BIT: usize = 12;
fn half_btf(w0: i32, x0: i32, w1: i32, x1: i32, bits: usize) -> i32 {
    i32::try_from(
        (i64::from(w0) * i64::from(x0) + i64::from(w1) * i64::from(x1) + (1i64 << (bits - 1)))
            >> bits,
    )
    .unwrap()
}
fn clamp_value(x: i32, range: usize) -> i32 {
    x.clamp(-(1 << (range - 1)), (1 << (range - 1)) - 1)
}
static COSPI_INV: [i32; 64] = [
    4096, 4095, 4091, 4085, 4076, 4065, 4052, 4036, 4017, 3996, 3973, 3948, 3920, 3889, 3857, 3822,
    3784, 3745, 3703, 3659, 3612, 3564, 3513, 3461, 3406, 3349, 3290, 3229, 3166, 3102, 3035, 2967,
    2896, 2824, 2751, 2675, 2598, 2520, 2440, 2359, 2276, 2191, 2106, 2019, 1931, 1842, 1751, 1660,
    1567, 1474, 1380, 1285, 1189, 1092, 995, 897, 799, 700, 601, 501, 401, 301, 201, 101,
];

fn av1_idct4(input: &[i32], output: &mut [i32], range: usize) {
    assert!(input.len() >= 4);
    assert!(output.len() >= 4);

    // stage 1
    let stg1 = [input[0], input[2], input[1], input[3]];

    // stage 2
    let stg2 = [
        half_btf(COSPI_INV[32], stg1[0], COSPI_INV[32], stg1[1], INV_COS_BIT),
        half_btf(COSPI_INV[32], stg1[0], -COSPI_INV[32], stg1[1], INV_COS_BIT),
        half_btf(COSPI_INV[48], stg1[2], -COSPI_INV[16], stg1[3], INV_COS_BIT),
        half_btf(COSPI_INV[16], stg1[2], COSPI_INV[48], stg1[3], INV_COS_BIT),
    ];

    // stage 3
    output[0] = clamp_value(stg2[0] + stg2[3], range);
    output[1] = clamp_value(stg2[1] + stg2[2], range);
    output[2] = clamp_value(stg2[1] - stg2[2], range);
    output[3] = clamp_value(stg2[0] - stg2[3], range);
}

fn av1_idct8(input: &[i32], output: &mut [i32], range: usize) {
    assert!(input.len() >= 8);
    assert!(output.len() >= 8);

    // call idct4
    let temp_in = [input[0], input[2], input[4], input[6]];
    let mut temp_out: [i32; 4] = [0; 4];
    av1_idct4(&temp_in, &mut temp_out, range);

    // stage 0

    // stage 1
    let stg1 = [input[1], input[5], input[3], input[7]];

    // stage 2
    let stg2 = [
        half_btf(COSPI_INV[56], stg1[0], -COSPI_INV[8], stg1[3], INV_COS_BIT),
        half_btf(COSPI_INV[24], stg1[1], -COSPI_INV[40], stg1[2], INV_COS_BIT),
        half_btf(COSPI_INV[40], stg1[1], COSPI_INV[24], stg1[2], INV_COS_BIT),
        half_btf(COSPI_INV[8], stg1[0], COSPI_INV[56], stg1[3], INV_COS_BIT),
    ];

    // stage 3
    let stg3 = [
        clamp_value(stg2[0] + stg2[1], range),
        clamp_value(stg2[0] - stg2[1], range),
        clamp_value(-stg2[2] + stg2[3], range),
        clamp_value(stg2[2] + stg2[3], range),
    ];

    // stage 4
    let stg4 = [
        stg3[0],
        half_btf(-COSPI_INV[32], stg3[1], COSPI_INV[32], stg3[2], INV_COS_BIT),
        half_btf(COSPI_INV[32], stg3[1], COSPI_INV[32], stg3[2], INV_COS_BIT),
        stg3[3],
    ];

    // stage 5
    output[0] = clamp_value(temp_out[0] + stg4[3], range);
    output[1] = clamp_value(temp_out[1] + stg4[2], range);
    output[2] = clamp_value(temp_out[2] + stg4[1], range);
    output[3] = clamp_value(temp_out[3] + stg4[0], range);
    output[4] = clamp_value(temp_out[3] - stg4[0], range);
    output[5] = clamp_value(temp_out[2] - stg4[1], range);
    output[6] = clamp_value(temp_out[1] - stg4[2], range);
    output[7] = clamp_value(temp_out[0] - stg4[3], range);
}

fn av1_idct16(input: &[i32], output: &mut [i32], range: usize) {
    assert!(input.len() >= 16);
    assert!(output.len() >= 16);

    // call idct8
    let temp_in = [
        input[0], input[2], input[4], input[6], input[8], input[10], input[12], input[14],
    ];
    let mut temp_out: [i32; 8] = [0; 8];
    av1_idct8(&temp_in, &mut temp_out, range);

    // stage 1
    let stg1 = [
        input[1], input[9], input[5], input[13], input[3], input[11], input[7], input[15],
    ];

    // stage 2
    let stg2 = [
        half_btf(COSPI_INV[60], stg1[0], -COSPI_INV[4], stg1[7], INV_COS_BIT),
        half_btf(COSPI_INV[28], stg1[1], -COSPI_INV[36], stg1[6], INV_COS_BIT),
        half_btf(COSPI_INV[44], stg1[2], -COSPI_INV[20], stg1[5], INV_COS_BIT),
        half_btf(COSPI_INV[12], stg1[3], -COSPI_INV[52], stg1[4], INV_COS_BIT),
        half_btf(COSPI_INV[52], stg1[3], COSPI_INV[12], stg1[4], INV_COS_BIT),
        half_btf(COSPI_INV[20], stg1[2], COSPI_INV[44], stg1[5], INV_COS_BIT),
        half_btf(COSPI_INV[36], stg1[1], COSPI_INV[28], stg1[6], INV_COS_BIT),
        half_btf(COSPI_INV[4], stg1[0], COSPI_INV[60], stg1[7], INV_COS_BIT),
    ];

    // stage 3
    let stg3 = [
        clamp_value(stg2[0] + stg2[1], range),
        clamp_value(stg2[0] - stg2[1], range),
        clamp_value(-stg2[2] + stg2[3], range),
        clamp_value(stg2[2] + stg2[3], range),
        clamp_value(stg2[4] + stg2[5], range),
        clamp_value(stg2[4] - stg2[5], range),
        clamp_value(-stg2[6] + stg2[7], range),
        clamp_value(stg2[6] + stg2[7], range),
    ];

    // stage 4
    let stg4 = [
        stg3[0],
        half_btf(-COSPI_INV[16], stg3[1], COSPI_INV[48], stg3[6], INV_COS_BIT),
        half_btf(
            -COSPI_INV[48],
            stg3[2],
            -COSPI_INV[16],
            stg3[5],
            INV_COS_BIT,
        ),
        stg3[3],
        stg3[4],
        half_btf(-COSPI_INV[16], stg3[2], COSPI_INV[48], stg3[5], INV_COS_BIT),
        half_btf(COSPI_INV[48], stg3[1], COSPI_INV[16], stg3[6], INV_COS_BIT),
        stg3[7],
    ];

    // stage 5
    let stg5 = [
        clamp_value(stg4[0] + stg4[3], range),
        clamp_value(stg4[1] + stg4[2], range),
        clamp_value(stg4[1] - stg4[2], range),
        clamp_value(stg4[0] - stg4[3], range),
        clamp_value(-stg4[4] + stg4[7], range),
        clamp_value(-stg4[5] + stg4[6], range),
        clamp_value(stg4[5] + stg4[6], range),
        clamp_value(stg4[4] + stg4[7], range),
    ];

    // stage 6
    let stg6 = [
        stg5[0],
        stg5[1],
        half_btf(-COSPI_INV[32], stg5[2], COSPI_INV[32], stg5[5], INV_COS_BIT),
        half_btf(-COSPI_INV[32], stg5[3], COSPI_INV[32], stg5[4], INV_COS_BIT),
        half_btf(COSPI_INV[32], stg5[3], COSPI_INV[32], stg5[4], INV_COS_BIT),
        half_btf(COSPI_INV[32], stg5[2], COSPI_INV[32], stg5[5], INV_COS_BIT),
        stg5[6],
        stg5[7],
    ];

    // stage 7
    output[0] = clamp_value(temp_out[0] + stg6[7], range);
    output[1] = clamp_value(temp_out[1] + stg6[6], range);
    output[2] = clamp_value(temp_out[2] + stg6[5], range);
    output[3] = clamp_value(temp_out[3] + stg6[4], range);
    output[4] = clamp_value(temp_out[4] + stg6[3], range);
    output[5] = clamp_value(temp_out[5] + stg6[2], range);
    output[6] = clamp_value(temp_out[6] + stg6[1], range);
    output[7] = clamp_value(temp_out[7] + stg6[0], range);
    output[8] = clamp_value(temp_out[7] - stg6[0], range);
    output[9] = clamp_value(temp_out[6] - stg6[1], range);
    output[10] = clamp_value(temp_out[5] - stg6[2], range);
    output[11] = clamp_value(temp_out[4] - stg6[3], range);
    output[12] = clamp_value(temp_out[3] - stg6[4], range);
    output[13] = clamp_value(temp_out[2] - stg6[5], range);
    output[14] = clamp_value(temp_out[1] - stg6[6], range);
    output[15] = clamp_value(temp_out[0] - stg6[7], range);
}
pub(super) fn inverse(input: &[i32], output: &mut [i32], n: usize) {
    match n {
        8 => av1_idct8(input, output, 19),
        16 => av1_idct16(input, output, 19),
        _ => panic!("integer study shape"),
    }
}
