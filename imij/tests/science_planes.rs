//! Science records in plane sets (SPEC.md 1.5): defaults and per-plane
//! records in the science attachment, malformed attachments, builder rules.

mod science_util;

use imij::planes::*;
use imij::science::*;
use imij::{Channels, Error, ImageInfo, Metadata, SampleType};

use science_util::*;

struct Stack {
    file: Vec<u8>,
    ids: Vec<PlaneId>,
    mask: PlaneId,
    levels: Vec<Vec<Vec<u8>>>,
}

/// Six (c, z) planes of a 2-channel, 3-slice stack with two levels each,
/// plus a mask plane; with or without science records.
fn stack(science: bool) -> (Stack, ScienceRecords, Vec<ScienceRecords>) {
    let pinfo = ImageInfo::new(8, 6, Channels::Gray, SampleType::U16);
    let mut primary_records = ScienceRecords::default();
    let mut pc = ChannelCalibration::default();
    pc.unit = Some("count".into());
    pc.mask_plane = Some(6);
    primary_records.calibration.push(pc);
    let pmeta = if science {
        with_records(&pinfo, &primary_records)
    } else {
        Metadata::default()
    };
    let primary = lossless()
        .encode_samples(imij::ImageRef::new(&gray16(8, 6, 9), 16, &pinfo), &pmeta)
        .unwrap();
    let mut b = PlaneSetBuilder::new(&primary).unwrap();
    let mut ids = Vec::new();
    let mut levels = Vec::new();
    for c in 0..2 {
        for z in 0..3 {
            let id = b
                .add_plane(PlaneDescriptor::new(format!("c{c}z{z}"), SampleType::U16))
                .unwrap();
            let mut lv = Vec::new();
            for (k, (w, h)) in [(32u32, 24u32), (16, 12)].into_iter().enumerate() {
                let px = gray16(w, h, (c * 10 + z * 3 + k) as u16);
                b.add_level(
                    id,
                    LevelDescriptor::new(w, h, 16, 16),
                    &px,
                    w as usize * 2,
                    lossless(),
                )
                .unwrap();
                lv.push(px);
            }
            ids.push(id);
            levels.push(lv);
        }
    }
    let mask = b
        .add_plane(PlaneDescriptor::new("valid", SampleType::U8).with_role(PlaneRole::Segmentation))
        .unwrap();
    let mask_px: Vec<u8> = (0..32 * 24).map(|i| (i % 7 != 0) as u8 * 255).collect();
    b.add_level(
        mask,
        LevelDescriptor::new(32, 24, 32, 24),
        &mask_px,
        32,
        lossless(),
    )
    .unwrap();
    levels.push(vec![mask_px]);
    // Defaults: NGFF-style c, z, y, x axes with two levels.
    let mut defaults = ScienceRecords::default();
    let mut axes = Axes::new(vec![
        axis("c", AxisKind::Channel, AxisBinding::Coordinate, None),
        axis(
            "z",
            AxisKind::Space,
            AxisBinding::Coordinate,
            Some("micrometer"),
        ),
        axis("y", AxisKind::Space, AxisBinding::Rows, Some("micrometer")),
        axis(
            "x",
            AxisKind::Space,
            AxisBinding::Columns,
            Some("micrometer"),
        ),
    ]);
    axes.levels.push(AxisLevel::new(
        vec![1.0, 2.0, 0.5, 0.5],
        vec![0.0, 0.0, 0.0, 0.0],
    ));
    axes.levels.push(AxisLevel::new(
        vec![1.0, 2.0, 1.0, 1.0],
        vec![0.0, 0.0, 0.25, 0.25],
    ));
    axes.downsampling = Some("mean".into());
    defaults.axes = Some(axes);
    let mut dc = ChannelCalibration::default();
    dc.unit = Some("count".into());
    dc.mask_plane = Some(mask.index());
    defaults.calibration.push(dc);
    defaults.structured.push(StructuredMetadata::new(
        "note",
        StructuredFormat::Text,
        b"shared".to_vec(),
    ));
    let mut expected = Vec::new();
    if science {
        b.set_science(None, defaults.clone()).unwrap();
    }
    for (i, &id) in ids.iter().enumerate() {
        let mut own = ScienceRecords::default();
        own.coordinates = vec![(i / 3) as i64, (i % 3) as i64];
        if i == 4 {
            let mut c = ChannelCalibration::default();
            c.slope = 2.0;
            c.nodata = vec![0.0, 4095.0];
            c.nan_is_missing = true;
            own.calibration.push(c);
            own.structured.push(StructuredMetadata::new(
                "ome-xml",
                StructuredFormat::Text,
                b"<OME/>".to_vec(),
            ));
        }
        let mut effective = own.clone();
        if effective.calibration.is_empty() {
            effective.calibration = defaults.calibration.clone();
        }
        if effective.structured.is_empty() {
            effective.structured = defaults.structured.clone();
        }
        effective.axes = defaults.axes.clone();
        if science {
            b.set_science(Some(id), own).unwrap();
        }
        expected.push(effective);
    }
    // The mask plane: its own identity calibration replaces the default.
    let mut own = ScienceRecords::default();
    own.calibration.push(ChannelCalibration::default());
    let mut effective = own.clone();
    effective.axes = defaults.axes.clone();
    effective.structured = defaults.structured.clone();
    if science {
        b.set_science(Some(mask), own).unwrap();
    }
    expected.push(effective);
    let file = b.finish().unwrap();
    (
        Stack {
            file,
            ids,
            mask,
            levels,
        },
        primary_records,
        expected,
    )
}

#[test]
fn plane_set_records_resolve_defaults_and_overrides() {
    let (plain, _, _) = stack(false);
    let (with, primary_records, expected) = stack(true);
    let mut r =
        PlaneSetReader::open(SliceReader::new(&with.file), PlaneSetLimits::default()).unwrap();
    let mut old =
        PlaneSetReader::open(SliceReader::new(&plain.file), PlaneSetLimits::default()).unwrap();
    assert_eq!(r.planes(), old.planes());
    let all: Vec<PlaneId> = with.ids.iter().copied().chain([with.mask]).collect();
    for (i, &id) in all.iter().enumerate() {
        assert_eq!(r.science(id).unwrap(), expected[i], "plane {i}");
        assert!(old.science(id).unwrap().is_empty());
        for (k, want) in with.levels[i].iter().enumerate() {
            let l = &r.planes()[i].levels()[k];
            let (w, h) = (l.width(), l.height());
            let bpp = r.planes()[i].sample_type().bytes();
            let mut a = vec![0u8; (w * h) as usize * bpp];
            let mut b = a.clone();
            r.decode_region(
                id,
                k as u32,
                Rect::new(0, 0, w, h),
                &mut a,
                w as usize * bpp,
            )
            .unwrap();
            old.decode_region(
                id,
                k as u32,
                Rect::new(0, 0, w, h),
                &mut b,
                w as usize * bpp,
            )
            .unwrap();
            assert_eq!(&a, want);
            assert_eq!(a, b);
        }
    }
    // The records live in one attachment; nothing else changed.
    let sci: Vec<_> = r
        .attachments()
        .iter()
        .filter(|a| a.mime_type() == "application/x.imij-science")
        .collect();
    assert_eq!(sci.len(), 1);
    assert_eq!(old.attachments().len(), 0);
    // The primary's own records (and the mask reference) and pixels.
    let primary = &with.file[..r.primary_len() as usize];
    let (info, meta) = imij::probe(primary).unwrap();
    assert_eq!(
        ScienceRecords::from_metadata(&info, &meta).unwrap(),
        primary_records
    );
    assert_eq!(
        imij::decode(&with.file).unwrap(),
        imij::decode(&plain.file).unwrap()
    );
    assert_eq!(r.plane_id(7), None);
}

/// A plane set whose science attachment holds `bytes`.
fn with_attachment(bytes: &[u8], twice: bool) -> Vec<u8> {
    let gray = ImageInfo::new(4, 4, Channels::Gray, SampleType::U8);
    let primary = lossless()
        .encode_samples(
            imij::ImageRef::new(&[0; 16], 4, &gray),
            &Metadata::default(),
        )
        .unwrap();
    let mut b = PlaneSetBuilder::new(&primary).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("p", SampleType::U8))
        .unwrap();
    b.add_level(
        id,
        LevelDescriptor::new(4, 4, 4, 4),
        &[7; 16],
        4,
        lossless(),
    )
    .unwrap();
    for i in 0..1 + twice as usize {
        b.add_attachment(
            AttachmentDescriptor::new(format!("s{i}"), "application/x.imij-science"),
            bytes,
        )
        .unwrap();
    }
    b.finish().unwrap()
}

#[test]
fn malformed_plane_set_records_are_corrupt_but_tiles_decode() {
    let entry = |target: u8, tag: &[u8; 4], payload: &[u8]| {
        let mut v = vec![target];
        v.extend_from_slice(tag);
        v.push(payload.len() as u8 + 1);
        v.extend_from_slice(payload);
        v
    };
    let cal = [1u8, 1, 0, 0];
    let good = entry(1, b"cALB", &cal);
    let cases: Vec<(Vec<u8>, bool)> = vec![
        (good.clone(), true),
        (entry(0, b"cALB", &cal), true),
        // Unknown ancillary entry tags are skipped.
        ([entry(1, b"zZZZ", &[9, 9]), good.clone()].concat(), true),
        (entry(2, b"cALB", &cal), false),
        (entry(1, b"c1LB", &cal), false),
        ([good.clone(), good.clone()].concat(), false),
        (good[..good.len() - 1].to_vec(), false),
        (vec![1, b'c', b'A', b'L', b'B', 0], false),
        (entry(1, b"cALB", &[1, 1, 0x80, 0x02, 0]), false),
        (entry(0, b"cORD", &[1, 1, 0]), false),
        // A malformed default is corrupt even where the plane overrides it.
        ([entry(0, b"cALB", &[1, 2]), good.clone()].concat(), false),
    ];
    for (bytes, ok) in cases {
        let file = with_attachment(&bytes, false);
        let mut r =
            PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
        let id = r.plane_id(0).unwrap();
        let got = r.science(id);
        assert_eq!(got.is_ok(), ok, "{bytes:?}: {got:?}");
        if ok {
            assert_eq!(
                got.unwrap().calibration,
                vec![ChannelCalibration::default()]
            );
        } else {
            assert!(matches!(got, Err(Error::Corrupt(_))), "{got:?}");
        }
        let mut out = [0u8; 16];
        r.decode_region(id, 0, Rect::new(0, 0, 4, 4), &mut out, 4)
            .unwrap();
        assert_eq!(out, [7; 16]);
    }
    let file = with_attachment(&good, true);
    let mut r = PlaneSetReader::open(SliceReader::new(&file), PlaneSetLimits::default()).unwrap();
    let id = r.plane_id(0).unwrap();
    assert_eq!(
        r.science(id),
        Err(Error::Corrupt("repeated science attachment"))
    );
}

#[test]
fn builder_refuses_records_next_to_a_raw_science_attachment() {
    let gray = ImageInfo::new(4, 4, Channels::Gray, SampleType::U8);
    let primary = lossless()
        .encode_samples(
            imij::ImageRef::new(&[0; 16], 4, &gray),
            &Metadata::default(),
        )
        .unwrap();
    let mut b = PlaneSetBuilder::new(&primary).unwrap();
    let id = b
        .add_plane(PlaneDescriptor::new("p", SampleType::U8))
        .unwrap();
    b.add_level(
        id,
        LevelDescriptor::new(4, 4, 4, 4),
        &[7; 16],
        4,
        lossless(),
    )
    .unwrap();
    b.add_attachment(
        AttachmentDescriptor::new("raw", "application/x.imij-science"),
        &[],
    )
    .unwrap();
    let mut r = ScienceRecords::default();
    r.calibration.push(ChannelCalibration::default());
    b.set_science(Some(id), r).unwrap();
    assert!(matches!(b.finish(), Err(Error::InvalidInput(_))));
}
