//! Retained V2/V3 wire grammar controls use the explicit legacy producer.
//! The lossless model grammar and its validity rules (SPEC 2, 2.1, 3.3),
//! through files from the independent writer in `common` (SPEC alone): the
//! cross bit in every model, canonical token parameters, table bounds, the
//! padding after the models, and Const values in the order domain.

mod common;

use common::{
    Bits, K4_ALPHABET, data_payload, gray8_file, gray8_stripe, normalize, one_lane_stream,
    payload_head, put_model_head, put_table, run_token, value_token,
};
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

const W: u32 = 8;
const H: u32 = 4;

/// Residual 5 (-3), then a run to the end of the 8 x 4 lane: every pixel
/// is 253.
fn syms() -> Vec<(u32, u32, u32)> {
    vec![value_token(5), run_token(31)]
}

fn weights() -> Vec<u32> {
    let mut w = vec![0; K4_ALPHABET];
    for s in syms() {
        w[s.0 as usize] = 1;
    }
    w
}

fn stripe() -> Vec<u8> {
    gray8_stripe(H, &one_lane_stream(&normalize(&weights()), &syms()))
}

/// The 8 x 4 Gray file with head present bits `present` and `models` as its
/// models bytes.
fn file(present: u64, models: &[u8]) -> Vec<u8> {
    gray8_file(W, H, present, models, &stripe())
}

/// The slot model the stripe was coded with (k = 4, m = 0).
fn model(b: &mut Bits, cross: bool) {
    put_model_head(b, 4, 0, cross, None);
    put_table(b, &weights());
}

fn models(f: impl FnOnce(&mut Bits)) -> Vec<u8> {
    let mut b = Bits::default();
    f(&mut b);
    b.out
}

fn decode(file: &[u8]) -> Result<Vec<u8>, Error> {
    imij::decode(file).map(|d| d.1)
}

#[track_caller]
fn corrupt(file: &[u8], why: &str) {
    match decode(file) {
        Err(Error::Corrupt(m)) => assert_eq!(m, why),
        other => panic!("{:?}, expected Corrupt({why})", other.map(|p| p.len())),
    }
}

/// The cross bit is written in every model, a Gray slot's included (where
/// it must be 0): a writer that left it out of a non-cross slot would shift
/// everything after it by one bit.
#[test]
fn every_model_carries_the_cross_bit() {
    assert_eq!(
        decode(&file(0, &models(|b| model(b, false)))),
        Ok(vec![253; 32])
    );
    let without = models(|b| {
        b.put(4, 5);
        b.put(0, 3);
        b.put(0, 32); // ctx_map, no cross bit before it
        put_table(b, &weights());
    });
    assert_ne!(decode(&file(0, &without)), Ok(vec![253; 32]));
}

/// `cross = 1` is valid only in slots 1 and 2 of RGB/RGBA: not in a Gray
/// slot, and never in the palette index model.
#[test]
fn cross_outside_the_color_slots_is_corrupt() {
    corrupt(&file(0, &models(|b| model(b, true))), "bad cross flag");
    // Present bit 0: the palette index model follows the slot models.
    let palette = |cross| {
        file(
            1,
            &models(|b| {
                model(b, false);
                model(b, cross);
            }),
        )
    };
    assert_eq!(decode(&palette(false)), Ok(vec![253; 32]));
    corrupt(&palette(true), "bad cross flag");
}

/// `k` must be the clamped `kv = min(max(k, m), n)`, at most 16; `m` at
/// most 4; the scheme's alphabet at most 256 symbols (n = 8 here).
#[test]
fn token_parameters_are_canonical() {
    for (k, m) in [(0, 1), (3, 4), (9, 0), (17, 0), (4, 5), (7, 0)] {
        let f = file(
            0,
            &models(|b| {
                put_model_head(b, k, m, false, None);
                put_table(b, &weights());
            }),
        );
        corrupt(&f, "bad token params");
    }
}

/// Table fields (SPEC 3.3): `last` inside the alphabet, gamma codes of at
/// most 24 leading zeros, exponents 0..=32 (32 included), some weight.
#[test]
fn tables_are_bounded() {
    let head = |b: &mut Bits| put_model_head(b, 4, 0, false, None);
    let table_of = |f: &dyn Fn(&mut Bits)| {
        file(
            0,
            &models(|b| {
                head(b);
                f(b);
            }),
        )
    };
    corrupt(
        &table_of(&|b| put_table(b, &[0; K4_ALPHABET])),
        "empty table",
    );
    corrupt(
        &table_of(&|b| {
            b.put(K4_ALPHABET as u64, 8);
            b.put(7, 3);
        }),
        "table too long",
    );
    // e = 33: zigzag 66, gamma(67).
    corrupt(
        &table_of(&|b| {
            b.put(0, 8);
            b.put(7, 3);
            b.gamma(67);
        }),
        "bad weight exponent",
    );
    // e = -1: zigzag 1, gamma(2).
    corrupt(
        &table_of(&|b| {
            b.put(0, 8);
            b.put(7, 3);
            b.gamma(2);
        }),
        "bad weight exponent",
    );
    corrupt(
        &table_of(&|b| {
            b.put(0, 8);
            b.put(7, 3);
            b.put(0, 25);
            b.put(1, 1);
        }),
        "bad gamma",
    );
    // Exponent 32 (weights of 2^31) normalizes like weights of 1.
    let big: Vec<u32> = weights().iter().map(|&w| w << 31).collect();
    assert_eq!(
        decode(&table_of(&|b| put_table(b, &big))),
        Ok(vec![253; 32])
    );
}

/// Bits after the last model, and bytes after them inside `models_len`, are
/// ignored (SPEC 3.3); a stream longer than `models_len` is corrupt.
#[test]
fn models_padding_is_ignored() {
    let good = models(|b| model(b, false));
    // 95 bits: one padding bit in the last byte.
    let mut ones = good.clone();
    *ones.last_mut().unwrap() |= 0x80;
    let mut longer = good.clone();
    longer.extend_from_slice(&[0xA5, 0xFF, 0x00]);
    for m in [&good, &ones, &longer] {
        assert_eq!(decode(&file(0, m)), Ok(vec![253; 32]));
    }
    let short = &good[..good.len() - 1];
    assert!(matches!(decode(&file(0, short)), Err(Error::Corrupt(_))));
}

/// A Const map's value is an order-domain value (SPEC 2.1): f16 alpha whose
/// samples are all the half 0xFFFF is written as value 0x8000 and decodes
/// back to 0xFFFF.
#[test]
fn const_values_are_order_domain() {
    let info = ImageInfo::new(8, 4, Channels::GrayAlpha, SampleType::F16);
    let mut px = Vec::new();
    for i in 0..32u16 {
        px.extend_from_slice(&(0x3C00 + i).to_ne_bytes());
        px.extend_from_slice(&0xFFFFu16.to_ne_bytes());
    }
    let f = imij::ImiEncoderConfig::default()
        .encode_legacy_samples(
            imij::ImageRef::new(&px, info.row_bytes(), &info),
            &Metadata::default(),
        )
        .unwrap();
    let d = &f[data_payload(&f)];
    let p = payload_head(d, &info, false);
    // present, lanes, stripe rows, column groups, then one map per channel.
    let mut at = 0;
    common::read_varint(d, &mut at);
    at += 1;
    common::read_varint(d, &mut at);
    common::read_varint(d, &mut at);
    let alpha = &d[at + 4..at + 8];
    assert!(at + 8 <= p.models.start);
    assert_eq!(alpha, [4, 0, 0x00, 0x80], "Const, 0 bits, value 0x8000");
    assert_eq!(decode(&f), Ok(px));
}
