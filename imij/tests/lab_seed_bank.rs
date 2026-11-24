//! Lab seed-bank registration through the process-wide registry. It runs in
//! a test process of its own: once bank 13 is registered, every later
//! decode in the process sees it (library tests expect it unknown).
#![cfg(all(feature = "lab", feature = "std"))]

use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

#[test]
fn registered_bank_registers_once_and_seeds_standalone_lossless() {
    const ID: u8 = 13;
    let mut blob = imij::seeds::bank_blob(1).unwrap().to_vec();
    blob[5] = ID;
    let mut wrong = blob.clone();
    wrong[0] = b'X';
    assert_eq!(
        imij::seeds::register_bank(wrong),
        Err(Error::InvalidInput("bad seed bank blob"))
    );
    assert_eq!(imij::seeds::register_bank(blob.clone()), Ok(u64::from(ID)));
    assert_eq!(
        imij::seeds::register_bank(blob),
        Err(Error::InvalidInput("seed bank id already registered"))
    );
    assert_eq!(imij::seeds::bank_blob(u64::from(ID)).unwrap()[5], ID);
    // Three flat colors and a gradient: standalone lossless encodes with and
    // without the bank decode to the source; seeds never make a file larger.
    let (w, h) = (48u32, 40u32);
    let px: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            match (x / 16 + y / 10) % 4 {
                0 => [250, 250, 250],
                1 => [20, 40, 200],
                2 => [220, 30, 30],
                _ => [x as u8 * 5, y as u8 * 6, 128],
            }
        })
        .collect();
    let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
    let off = imij::ImiEncoderConfig::default();
    let mut on = off.clone();
    on = on.tune_lossless(|t| {
        t.seed_bank = u64::from(ID);
    });
    let a = off
        .encode_samples(
            imij::ImageRef::new(&px, w as usize * 3, &info),
            &Metadata::default(),
        )
        .unwrap();
    let b = on
        .encode_samples(
            imij::ImageRef::new(&px, w as usize * 3, &info),
            &Metadata::default(),
        )
        .unwrap();
    assert!(b.len() <= a.len());
    assert_eq!(imij::decode(&a).unwrap().1, px);
    assert_eq!(imij::decode(&b).unwrap().1, px);
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 128 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

fn bank_envelope(id: u64) -> Vec<u8> {
    let inner = imij::seeds::bank_blob(1).unwrap();
    let mut out = b"IMSB\x02".to_vec();
    varint(&mut out, id);
    varint(&mut out, inner.len() as u64);
    out.extend_from_slice(inner);
    out
}

#[test]
fn full_width_bank_ids_roundtrip_without_masking_or_policy_bits() {
    let ids = [
        16,
        127,
        128,
        16_383,
        16_384,
        u64::from(u32::MAX) + 1,
        u64::MAX,
    ];
    let info = ImageInfo::new(96, 64, Channels::Rgb, SampleType::U8);
    let pixels: Vec<u8> = (0..info.row_bytes() * info.height as usize)
        .map(|i| ((i * 37 + i / 17) ^ (i / 193)) as u8)
        .collect();
    for id in ids {
        let blob = bank_envelope(id);
        assert_eq!(imij::seeds::register_bank(blob.clone()), Ok(id));
        assert_eq!(imij::seeds::bank_blob(id).unwrap(), blob);
        assert_eq!(
            imij::seeds::register_bank(blob),
            Err(Error::InvalidInput("seed bank id already registered"))
        );
        for entropy in [imij::LossyEntropy::RunLevel, imij::LossyEntropy::SplitHuf] {
            let cfg = imij::ImiEncoderConfig::lossy(5.0)
                .with_effort(0.0)
                .tune_lossy(|t| {
                    t.entropy = entropy;
                    t.aq_gamma = 0.0;
                    t.exact_blocks = Some(0.0);
                    t.lab.seed_bank = id;
                });
            let file = cfg
                .encode_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let (_, used, scope) = imij::seeds::lossy_entropy_seed(&file).unwrap();
            assert_eq!(used, id, "positive seeded witness, {entropy:?}");
            assert_ne!(scope, 0);
            let unseeded = cfg
                .clone()
                .tune_lossy(|t| t.lab.seed_bank = 0)
                .encode_samples(
                    imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            assert!(
                file.len() <= unseeded.len(),
                "complete-file fallback still counts the longer identifier"
            );
            let expected = imij::decode(&unseeded).unwrap().1;
            for threads in [1, 2, 4, 8] {
                let stride = info.row_bytes() + 13;
                let mut out = vec![0xa7; stride * info.height as usize];
                imij::decode_into_threads(&file, &mut out, stride, threads).unwrap();
                for y in 0..info.height as usize {
                    assert_eq!(
                        &out[y * stride..y * stride + info.row_bytes()],
                        &expected[y * info.row_bytes()..(y + 1) * info.row_bytes()]
                    );
                    assert!(
                        out[y * stride + info.row_bytes()..(y + 1) * stride]
                            .iter()
                            .all(|&b| b == 0xa7)
                    );
                }
            }
        }
        let off = imij::ImiEncoderConfig::lossless();
        let on = off.clone().tune_lossless(|t| t.seed_bank = id);
        let a = off
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        let b = on
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                &Metadata::default(),
            )
            .unwrap();
        assert!(b.len() <= a.len());
        assert_eq!(imij::decode(&b).unwrap().1, pixels);
    }
}

#[test]
fn malformed_wide_bank_envelopes_do_not_register() {
    let id = 1u64 << 40;
    let valid = bank_envelope(id);
    let mut trailing = valid.clone();
    trailing.push(0);
    let mut bad_inner = valid.clone();
    let inner = bad_inner
        .windows(5)
        .rposition(|w| w == b"IMSB\x01")
        .unwrap();
    bad_inner[inner] = b'X';
    for blob in [
        b"IMSB\x02\x80".to_vec(),
        b"IMSB\x02\xff\xff\xff\xff\xff\xff\xff\xff\xff\x02".to_vec(),
        trailing,
        bad_inner,
    ] {
        assert_eq!(
            imij::seeds::register_bank(blob),
            Err(Error::InvalidInput("bad seed bank blob"))
        );
        assert!(imij::seeds::bank_blob(id).is_none());
    }
}
