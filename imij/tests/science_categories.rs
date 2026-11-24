//! Categorical records (`cATG`, SPEC.md 1.5): label maps, class maps and
//! palette images keep their index values as integer samples, and the record
//! maps each value to a color, name and description.

mod science_util;

use imij::planes::*;
use imij::science::*;
use imij::{
    Channels, Chunk, Error, FrameConfig, FrameDecoder, FrameEncoder, ImageInfo, Metadata,
    SampleType,
};
use science_util::*;

fn cat(
    value: i64,
    rgba8: Option<[u8; 4]>,
    name: Option<&str>,
    description: Option<&str>,
) -> Category {
    let mut c = Category::new(value);
    c.color = rgba8.map(|v| v.map(|x| u16::from(x) * 257));
    c.name = name.map(String::from);
    c.description = description.map(String::from);
    c
}

/// Five land-cover classes (NLCD-style codes) plus a nodata value.
fn land_cover() -> ScienceRecords {
    let mut r = ScienceRecords::default();
    r.categories.push(vec![
        cat(
            11,
            Some([70, 107, 159, 255]),
            Some("open_water"),
            Some("areas of open water, generally with less than 25% cover of vegetation or soil"),
        ),
        cat(
            21,
            Some([222, 197, 197, 255]),
            Some("developed_open_space"),
            None,
        ),
        cat(
            41,
            Some([104, 171, 95, 255]),
            Some("deciduous_forest"),
            None,
        ),
        cat(
            82,
            Some([171, 108, 40, 255]),
            Some("cultivated_crops"),
            None,
        ),
        cat(90, None, Some("woody_wetlands"), None),
    ]);
    let mut c = ChannelCalibration::default();
    c.nodata = vec![0.0];
    r.calibration.push(c);
    r
}

fn class_map(w: u32, h: u32) -> Vec<u8> {
    let codes = [11u8, 21, 41, 82, 90, 0];
    (0..w * h)
        .map(|i| codes[((i / 7 + i / w) % 6) as usize])
        .collect()
}

#[test]
fn class_map_round_trips_with_nodata_and_keeps_values() {
    let info = ImageInfo::new(40, 30, Channels::Gray, SampleType::U8);
    let px = class_map(40, 30);
    let records = land_cover();
    let meta = with_records(&info, &records);
    assert_eq!(tags(&meta), [*b"cALB", *b"cATG"]);
    let plain = lossless()
        .encode_samples(imij::ImageRef::new(&px, 40, &info), &Metadata::default())
        .unwrap();
    let with = lossless()
        .encode_samples(imij::ImageRef::new(&px, 40, &info), &meta)
        .unwrap();
    assert_only_inserted(&plain, &with);
    // The decoded samples are the class codes themselves.
    assert_eq!(imij::decode(&with).unwrap().1, px);
    let (pinfo, pmeta) = imij::probe(&with).unwrap();
    let back = ScienceRecords::from_metadata(&pinfo, &pmeta).unwrap();
    assert_eq!(back, records);
    // Every sample is a listed class or the nodata value.
    let cal = &back.calibration[0];
    for &v in &px {
        let listed = back.categories[0].iter().any(|c| c.value == i64::from(v));
        assert!(listed != cal.is_missing(f64::from(v)), "{v}");
    }
    // Pages: the outer record describes every page.
    let mut e = FrameEncoder::pages(info.clone(), meta.clone(), FrameConfig::new(lossless()));
    for _ in 0..2 {
        e.push_page(&px, 40, &info).unwrap();
    }
    let file = e.finish().unwrap();
    let d = FrameDecoder::new(&file).unwrap();
    assert_eq!(
        ScienceRecords::from_metadata(d.info(), d.metadata()).unwrap(),
        records
    );
}

#[test]
fn gray_alpha_marks_only_the_categorical_channel() {
    let info = ImageInfo::new(8, 8, Channels::GrayAlpha, SampleType::U16);
    let mut r = ScienceRecords::default();
    r.categories = vec![
        vec![
            cat(1, None, Some("cell"), None),
            cat(2, None, Some("nucleus"), None),
        ],
        vec![],
    ];
    let meta = with_records(&info, &r);
    let px: Vec<u8> = (0..64u16)
        .flat_map(|i| [i % 3, 65535])
        .flat_map(u16::to_ne_bytes)
        .collect();
    let file = lossless()
        .encode_samples(imij::ImageRef::new(&px, 32, &info), &meta)
        .unwrap();
    let (pi, pm) = imij::probe(&file).unwrap();
    assert_eq!(ScienceRecords::from_metadata(&pi, &pm).unwrap(), r);
    assert_eq!(imij::decode(&file).unwrap().1, px);
}

#[test]
fn colors_use_sixteen_bits_only_when_needed() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U16);
    let mut r = ScienceRecords::default();
    r.categories
        .push(vec![cat(0, Some([0, 0, 128, 128]), None, None)]);
    let narrow = with_records(&info, &r).chunks[0].data.clone();
    // version, flags 0, one channel, one entry, value 0, entry flags, RGBA.
    assert_eq!(narrow, [1, 0, 1, 1, 0, 1, 0, 0, 128, 128]);
    // A TIFF ColorMap component that is not a multiple of 257.
    r.categories[0][0].color = Some([0, 1, 32896, 65535]);
    let wide = with_records(&info, &r);
    assert_eq!(wide.chunks[0].data[1], 1);
    assert_eq!(wide.chunks[0].data.len(), narrow.len() + 4);
    assert_eq!(ScienceRecords::from_metadata(&info, &wide).unwrap(), r);
}

/// docs/SCIENCE_METADATA.md: the OME-NGFF 0.5 `image-label` example.
#[test]
fn ngff_image_label_example_bytes() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
    let mut r = ScienceRecords::default();
    r.categories.push(vec![
        cat(0, Some([0, 0, 128, 128]), Some("intercellular space"), None),
        cat(1, Some([0, 128, 0, 128]), Some("cell"), None),
    ]);
    let meta = with_records(&info, &r);
    let mut want = vec![1u8, 0, 1, 2];
    want.extend_from_slice(&[0, 3, 0, 0, 128, 128, 19]);
    want.extend_from_slice(b"intercellular space");
    want.extend_from_slice(&[2, 3, 0, 128, 0, 128, 4]);
    want.extend_from_slice(b"cell");
    assert_eq!(meta.chunks[0].tag, *b"cATG");
    assert_eq!(meta.chunks[0].data, want);
    assert_eq!(want.len(), 41);
    assert_eq!(ScienceRecords::from_metadata(&info, &meta).unwrap(), r);
}

#[test]
fn plane_set_defaults_and_overrides() {
    let gray = ImageInfo::new(4, 4, Channels::Gray, SampleType::U8);
    let primary = imij::ImiEncoderConfig::default()
        .encode_samples(
            imij::ImageRef::new(&[0; 16], 4, &gray),
            &Metadata::default(),
        )
        .unwrap();
    let build = |science: bool| {
        let mut b = PlaneSetBuilder::new(&primary).unwrap();
        let mut ids = Vec::new();
        for (i, name) in ["labels_t0", "labels_t1", "classes"].iter().enumerate() {
            let id = b
                .add_plane(
                    PlaneDescriptor::new(*name, SampleType::U16).with_role(PlaneRole::Segmentation),
                )
                .unwrap();
            let px = gray16(24, 16, i as u16);
            b.add_level(
                id,
                LevelDescriptor::new(24, 16, 16, 16),
                &px,
                48,
                lossless(),
            )
            .unwrap();
            ids.push((id, px));
        }
        if science {
            let mut d = ScienceRecords::default();
            d.categories.push(vec![
                cat(0, Some([0, 0, 0, 0]), Some("background"), None),
                cat(1, None, Some("cell"), None),
            ]);
            b.set_science(None, d).unwrap();
            let mut own = ScienceRecords::default();
            own.categories
                .push(vec![cat(5, None, Some("water"), Some("open water"))]);
            b.set_science(Some(ids[2].0), own).unwrap();
        }
        (b.finish().unwrap(), ids)
    };
    let (plain, _) = build(false);
    let (with, ids) = build(true);
    let mut r = PlaneSetReader::open(SliceReader::new(&with), PlaneSetLimits::default()).unwrap();
    let mut old =
        PlaneSetReader::open(SliceReader::new(&plain), PlaneSetLimits::default()).unwrap();
    for (i, (id, px)) in ids.iter().enumerate() {
        let got = r.science(*id).unwrap();
        let want = if i == 2 { "water" } else { "background" };
        assert_eq!(got.categories[0][0].name.as_deref(), Some(want));
        let mut a = vec![0u8; px.len()];
        let mut b = a.clone();
        r.decode_region(*id, 0, Rect::new(0, 0, 24, 16), &mut a, 48)
            .unwrap();
        old.decode_region(*id, 0, Rect::new(0, 0, 24, 16), &mut b, 48)
            .unwrap();
        assert_eq!(&a, px);
        assert_eq!(a, b);
    }
}

#[test]
fn malformed_categories_are_corrupt_but_pixels_decode() {
    let info = ImageInfo::new(40, 30, Channels::Gray, SampleType::U8);
    let px = class_map(40, 30);
    let good = with_records(&info, &land_cover());
    let catg = good.chunks.iter().position(|c| c.tag == *b"cATG").unwrap();
    let parse = |chunks: Vec<Chunk>| {
        ScienceRecords::from_metadata(
            &info,
            &Metadata {
                chunks,
                ..Default::default()
            },
        )
    };
    let data = &good.chunks[catg].data;
    for n in 0..data.len() {
        let mut chunks = good.chunks.clone();
        chunks[catg].data.truncate(n);
        assert!(
            matches!(parse(chunks.clone()), Err(Error::Corrupt(_))),
            "prefix {n}"
        );
        if n % 9 == 0 {
            let meta = Metadata {
                chunks,
                ..Default::default()
            };
            let file = lossless()
                .encode_samples(imij::ImageRef::new(&px, 40, &info), &meta)
                .unwrap();
            assert_eq!(imij::decode(&file).unwrap().1, px);
        }
    }
    for at in 1..data.len() {
        for bit in 0..8 {
            let mut chunks = good.chunks.clone();
            chunks[catg].data[at] ^= 1 << bit;
            let _ = parse(chunks);
        }
    }
    let one = |d: &[u8]| {
        parse(vec![Chunk {
            tag: *b"cATG",
            data: d.to_vec(),
        }])
    };
    assert!(one(&[2, 9, 9]).unwrap().is_empty()); // unknown version: skipped
    assert!(one(&[1, 0, 1, 1, 0, 0]).unwrap().categories.len() == 1);
    for bad in [
        &[1, 2, 1, 1, 0, 0][..],            // reserved record flag
        &[1, 0, 1, 1, 0, 8],                // reserved entry flag
        &[1, 0, 0],                         // no channels
        &[1, 0, 2, 1, 0, 0, 0],             // two channels on a Gray image
        &[1, 0, 1, 0],                      // no categories at all
        &[1, 0, 1, 2, 4, 0, 2, 0],          // values not increasing (2, 1)
        &[1, 0, 1, 2, 2, 0, 2, 0],          // repeated value
        &[1, 0, 1, 0xff, 0xff, 0x7f, 0, 0], // count beyond the payload
        &[1, 0, 1, 1, 0, 2, 0],             // empty name
        &[1, 0, 1, 1, 0, 0, 0],             // trailing byte
    ] {
        assert!(matches!(one(bad), Err(Error::Corrupt(_))), "{bad:?}");
    }
    // A categorical channel cannot also carry a slope or offset.
    let mut r = land_cover();
    r.calibration[0].slope = 2.0;
    let mut chunks = good.chunks.clone();
    chunks[0].data = {
        let mut m = Metadata::default();
        let mut c = ScienceRecords::default();
        c.calibration = r.calibration.clone();
        c.write_metadata(&info, &mut m).unwrap();
        m.chunks[0].data.clone()
    };
    assert_eq!(
        parse(chunks),
        Err(Error::Corrupt("categorical channel with slope or offset"))
    );
}

#[test]
fn writer_rejects_invalid_categories() {
    let info = ImageInfo::new(4, 4, Channels::Gray, SampleType::U8);
    let bad = |f: &dyn Fn(&mut ScienceRecords)| {
        let mut r = land_cover();
        f(&mut r);
        let mut meta = Metadata::default();
        assert!(matches!(
            r.write_metadata(&info, &mut meta),
            Err(Error::InvalidInput(_))
        ));
        assert!(meta.chunks.is_empty());
    };
    bad(&|r| r.categories[0].swap(0, 1));
    bad(&|r| r.categories[0][1].value = 11);
    bad(&|r| r.categories.push(vec![]));
    bad(&|r| r.categories[0].clear());
    bad(&|r| r.categories[0][0].name = Some(String::new()));
    bad(&|r| r.categories[0][0].description = Some("x".repeat(1025)));
    bad(&|r| r.categories[0] = (0..65_537).map(Category::new).collect());
    bad(&|r| r.calibration[0].offset = 1.0);
    let mut ok = land_cover();
    ok.categories[0] = (0..65_536).map(Category::new).collect();
    ok.write_metadata(&info, &mut Metadata::default()).unwrap();
}

/// docs/SCIENCE_METADATA.md: CF 1.12 Example 3.5, a byte quality flag with
/// flag_values 0, 1, 2, _FillValue -128 and valid_range 0..2.
#[cfg(feature = "extended")]
#[test]
fn cf_flag_values_example() {
    let info = ImageInfo::new(6, 1, Channels::Gray, SampleType::I32);
    let px: Vec<u8> = [0i32, 1, 2, -128, 1, 0]
        .iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect();
    let mut r = ScienceRecords::default();
    let mut c = ChannelCalibration::default();
    c.quantity = Some("status_flag".into());
    c.quantity_is_standard_name = true;
    c.nodata = vec![-128.0];
    c.valid_min = Some(0.0);
    c.valid_max = Some(2.0);
    r.calibration.push(c);
    r.categories.push(vec![
        cat(0, None, Some("quality_good"), None),
        cat(1, None, Some("sensor_nonfunctional"), None),
        cat(2, None, Some("outside_valid_range"), None),
    ]);
    let meta = with_records(&info, &r);
    let file = lossless()
        .encode_samples(imij::ImageRef::new(&px, 24, &info), &meta)
        .unwrap();
    let (pi, pm) = imij::probe(&file).unwrap();
    assert_eq!(ScienceRecords::from_metadata(&pi, &pm).unwrap(), r);
    assert_eq!(imij::decode(&file).unwrap().1, px);
}

/// Audit witness (SPEC 1.5 `cATG`): float samples compare in binary64, so
/// each category value is first rounded to binary64 (to nearest, ties to
/// even). Up to 2^53 every integer converts exactly. Beyond it, distinct
/// categories can round to the same binary64 and then share its samples,
/// and a category can match a sample that differs from its value. The
/// record is valid (values strictly increase as integers); integer samples
/// compare as integers and keep the categories apart. This documents the
/// current rule; it is not a semantic change.
#[cfg(feature = "extended")]
#[test]
fn float_channel_categories_beyond_2_53_share_samples() {
    let big = 1i64 << 53;
    let values = [big - 1, big, big + 1, big + 3, big + 5];
    // The binary64 each category value is compared as.
    let as_f64: Vec<f64> = values.iter().map(|&v| v as f64).collect();
    assert_eq!(as_f64[0] as i64, big - 1); // exact up to 2^53
    assert_eq!(as_f64[1], as_f64[2]); // 2^53 + 1 rounds to 2^53
    assert_eq!(as_f64[3], as_f64[4]); // 2^53 + 3 and + 5 round to 2^53 + 4
    assert_eq!(as_f64[3] as i64, big + 4);
    let matching = |sample: f64| -> Vec<i64> {
        values
            .iter()
            .zip(&as_f64)
            .filter(|&(_, &f)| f == sample)
            .map(|(&v, _)| v)
            .collect()
    };

    // An F32 channel: 2^53 is exact in f32, so a stored sample can equal it.
    let info = ImageInfo::new(2, 1, Channels::Gray, SampleType::F32);
    let mut r = ScienceRecords::default();
    r.categories
        .push(values.iter().map(|&v| cat(v, None, None, None)).collect());
    let meta = with_records(&info, &r); // accepted by the writer
    let px: Vec<u8> = [big as f32, 1.0f32]
        .iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect();
    let file = lossless()
        .encode_samples(imij::ImageRef::new(&px, 8, &info), &meta)
        .unwrap();
    let (pi, pm) = imij::probe(&file).unwrap();
    assert_eq!(ScienceRecords::from_metadata(&pi, &pm).unwrap(), r); // and by the reader
    let stored = f64::from(f32::from_ne_bytes(px[..4].try_into().unwrap()));
    assert_eq!(matching(stored), [big, big + 1]); // one sample, two categories
    assert!(matching(1.0).is_empty());

    // A binary64 sample (f64 samples are added by the samples branch):
    // 2^53 + 4 matches categories 2^53 + 3 and 2^53 + 5, neither equal to it,
    // and no category is 2^53 + 4 itself.
    assert_eq!(matching((big + 4) as f64), [big + 3, big + 5]);

    // Integer samples (i64 on that branch) compare as integers: exact.
    let integer_matching = |sample: i64| values.iter().filter(|&&v| v == sample).count();
    assert_eq!(integer_matching(big), 1);
    assert_eq!(integer_matching(big + 1), 1);
    assert_eq!(integer_matching(big + 4), 0);
}
