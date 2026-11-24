//! Science records (SPEC.md 1.5): round trips through every write and read
//! path, decoders skipping them, malformed records, byte-identical output
//! without records, and the worked mappings in docs/SCIENCE_METADATA.md.
//! Plane-set records are in science_planes.rs.

mod science_util;

use imij::planes::*;
use imij::science::*;
use imij::{
    Channels, Chunk, Error, FrameConfig, FrameDecoder, FrameDuration, FrameEncoder, FrameKind,
    ImageInfo, Metadata, SampleType,
};

use science_util::*;

#[test]
fn single_image_round_trips_lossless_and_lossy() {
    let info = ImageInfo::new(37, 23, Channels::Gray, SampleType::U16);
    let px = gray16(37, 23, 5);
    let records = ct_records();
    let meta = with_records(&info, &records);
    assert_eq!(
        tags(&meta),
        [*b"cALB", *b"aXES", *b"cORD", *b"sMDT", *b"sMDT"]
    );
    for lossy in [false, true] {
        let enc = |m: &Metadata| {
            if lossy {
                imij::ImiEncoderConfig::lossy(90.0)
                    .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), m)
            } else {
                lossless().encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), m)
            }
            .unwrap()
        };
        let plain = enc(&Metadata::default());
        let with = enc(&meta);
        assert_only_inserted(&plain, &with);
        // Pixels: identical with and without records.
        assert_eq!(imij::decode(&with).unwrap(), imij::decode(&plain).unwrap());
        // probe: the container returns the records as ordinary ancillary
        // chunks, exactly as a decoder that does not know them would.
        let (pinfo, pmeta) = imij::probe(&with).unwrap();
        assert_eq!(pmeta.chunks, meta.chunks);
        assert_eq!(
            ScienceRecords::from_metadata(&pinfo, &pmeta).unwrap(),
            records
        );
        let (_, none) = imij::probe(&plain).unwrap();
        assert!(
            ScienceRecords::from_metadata(&pinfo, &none)
                .unwrap()
                .is_empty()
        );
        // Streaming reader.
        let mut rows = Vec::new();
        let (rinfo, rmeta) = imij::decode_reader(&with[..], &imij::Limits::DEFAULT, |_, _, row| {
            rows.extend_from_slice(row);
            Ok(())
        })
        .unwrap();
        assert_eq!(rows, imij::decode(&plain).unwrap().1);
        assert_eq!(
            ScienceRecords::from_metadata(&rinfo, &rmeta).unwrap(),
            records
        );
        // Decode sessions (cumulative budgets) keep them too.
        let mut session = imij::decode_resources::DecodeSession::new(
            imij::decode_resources::DecodePolicy::default(),
        );
        let (sinfo, smeta) = session.decode_reader(&with[..], |_, _, _| Ok(())).unwrap();
        assert_eq!(
            ScienceRecords::from_metadata(&sinfo, &smeta).unwrap(),
            records
        );
        // Row streaming decoder.
        let mut s = imij::StreamDecoder::new(&with).unwrap();
        let mut row = vec![0u8; info.row_bytes()];
        let mut all = Vec::new();
        while s.next_row(&mut row).unwrap() {
            all.extend_from_slice(&row);
        }
        assert_eq!(all, imij::decode(&plain).unwrap().1);
    }
}

#[test]
fn rgb_channels_axis_and_three_calibrations() {
    let info = ImageInfo::new(16, 9, Channels::Rgb, SampleType::U8);
    let mut records = ScienceRecords::default();
    for (i, unit) in ["count", "count", "count"].iter().enumerate() {
        let mut c = ChannelCalibration::default();
        c.slope = 0.25 * (i + 1) as f64;
        c.unit = Some((*unit).into());
        records.calibration.push(c);
    }
    let mut axes = Axes::new(vec![
        axis("c", AxisKind::Channel, AxisBinding::Channels, None),
        axis("y", AxisKind::Space, AxisBinding::Rows, Some("micrometer")),
        axis(
            "x",
            AxisKind::Space,
            AxisBinding::Columns,
            Some("micrometer"),
        ),
    ]);
    axes.levels
        .push(AxisLevel::new(vec![1.0, 0.65, 0.65], vec![0.0, 0.0, 0.0]));
    records.axes = Some(axes);
    let meta = with_records(&info, &records);
    let px = rgb8(16, 9, 3);
    let file = lossless()
        .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
        .unwrap();
    let (pinfo, pmeta) = imij::probe(&file).unwrap();
    assert_eq!(
        ScienceRecords::from_metadata(&pinfo, &pmeta).unwrap(),
        records
    );
    assert_eq!(imij::decode(&file).unwrap().1, px);
    // The same chunks read against a Gray image: wrong channel count.
    let gray = ImageInfo::new(16, 9, Channels::Gray, SampleType::U8);
    assert_eq!(
        ScienceRecords::from_metadata(&gray, &pmeta),
        Err(Error::Corrupt("calibration channel count"))
    );
}

#[test]
fn frames_and_pages_carry_the_outer_records() {
    // Pages as a z-stack: the page index runs along z.
    let info = ImageInfo::new(12, 10, Channels::Gray, SampleType::U16);
    let mut records = ct_records();
    records.coordinates.clear();
    let a = records.axes.as_mut().unwrap();
    a.axes[0].binding = AxisBinding::Frames;
    let pages: Vec<Vec<u8>> = (0..3).map(|i| gray16(12, 10, i * 101)).collect();
    let build = |meta: Metadata| {
        let mut e = FrameEncoder::pages(info.clone(), meta, FrameConfig::new(lossless()));
        for p in &pages {
            e.push_page(p, info.row_bytes(), &info).unwrap();
        }
        e.finish().unwrap()
    };
    let plain = build(Metadata::default());
    let with = build(with_records(&info, &records));
    assert_only_inserted(&plain, &with);
    let mut d = FrameDecoder::new(&with).unwrap();
    assert_eq!(d.kind(), FrameKind::Pages);
    assert_eq!(
        ScienceRecords::from_metadata(d.info(), d.metadata()).unwrap(),
        records
    );
    let mut i = 0;
    while let Some(h) = d.next_frame().unwrap() {
        let mut out = vec![0u8; info.row_bytes() * h.height as usize];
        d.decode_into(&mut out, info.row_bytes(), 1).unwrap();
        assert_eq!(out, pages[i]);
        i += 1;
    }
    assert_eq!(i, 3);

    // An animation: time runs along the frames, channels interleaved.
    let info = ImageInfo::new(20, 8, Channels::Rgb, SampleType::U8);
    let mut records = ScienceRecords::default();
    let mut axes = Axes::new(vec![
        axis("t", AxisKind::Time, AxisBinding::Frames, Some("second")),
        axis("c", AxisKind::Channel, AxisBinding::Channels, None),
        axis("y", AxisKind::Space, AxisBinding::Rows, None),
        axis("x", AxisKind::Space, AxisBinding::Columns, None),
    ]);
    axes.levels.push(AxisLevel::new(
        vec![0.04, 1.0, 1.0, 1.0],
        vec![0.0, 0.0, 0.0, 0.0],
    ));
    records.axes = Some(axes);
    let frames: Vec<Vec<u8>> = (0..4).map(|i| rgb8(20, 8, i * 50)).collect();
    let build = |meta: Metadata| {
        let mut e = FrameEncoder::animation(info.clone(), meta, FrameConfig::new(lossless()));
        for f in &frames {
            e.push_frame(f, info.row_bytes(), FrameDuration::from_millis(40))
                .unwrap();
        }
        e.finish().unwrap()
    };
    let plain = build(Metadata::default());
    let with = build(with_records(&info, &records));
    assert_only_inserted(&plain, &with);
    let mut d = FrameDecoder::new(&with).unwrap();
    assert_eq!(
        ScienceRecords::from_metadata(d.info(), d.metadata()).unwrap(),
        records
    );
    let mut canvas = vec![0u8; info.row_bytes() * 8];
    let mut i = 0;
    while d.next_frame().unwrap().is_some() {
        d.decode_into(&mut canvas, info.row_bytes(), 1).unwrap();
        assert_eq!(canvas, frames[i]);
        i += 1;
    }
    assert_eq!(i, 4);
}

#[test]
fn byte_identical_without_records() {
    let info = ImageInfo::new(9, 7, Channels::Gray, SampleType::U16);
    let px = gray16(9, 7, 1);
    let mut meta = Metadata::default();
    ScienceRecords::default()
        .write_metadata(&info, &mut meta)
        .unwrap();
    assert_eq!(meta, Metadata::default());
    // Writing records and then empty records removes them again.
    ct_records().write_metadata(&info, &mut meta).unwrap();
    ScienceRecords::default()
        .write_metadata(&info, &mut meta)
        .unwrap();
    assert_eq!(meta, Metadata::default());
    assert_eq!(
        lossless()
            .encode_samples(imij::ImageRef::new(&px, 18, &info), &meta)
            .unwrap(),
        lossless()
            .encode_samples(imij::ImageRef::new(&px, 18, &info), &Metadata::default())
            .unwrap()
    );
    let primary = lossless()
        .encode_samples(imij::ImageRef::new(&px, 18, &info), &meta)
        .unwrap();
    let build = |set: bool| {
        let mut b = PlaneSetBuilder::new(&primary).unwrap();
        let id = b
            .add_plane(PlaneDescriptor::new("p", SampleType::U16))
            .unwrap();
        b.add_level(id, LevelDescriptor::new(9, 7, 4, 4), &px, 18, lossless())
            .unwrap();
        if set {
            b.set_science(None, ct_records()).unwrap_err();
            b.set_science(Some(id), ScienceRecords::default()).unwrap();
            b.set_science(None, ScienceRecords::default()).unwrap();
        }
        b.finish().unwrap()
    };
    assert_eq!(build(true), build(false));
}

#[test]
fn malformed_records_are_corrupt_but_pixels_decode() {
    let info = ImageInfo::new(5, 4, Channels::Gray, SampleType::U16);
    let px = gray16(5, 4, 2);
    let good = with_records(&info, &ct_records());
    let plain = lossless()
        .encode_samples(imij::ImageRef::new(&px, 10, &info), &Metadata::default())
        .unwrap();
    let pixels = imij::decode(&plain).unwrap();
    let parse = |chunks: Vec<Chunk>| {
        let meta = Metadata {
            chunks,
            ..Default::default()
        };
        ScienceRecords::from_metadata(&info, &meta)
    };
    // Every strict prefix of every record is malformed; flips never panic.
    for (i, c) in good.chunks.iter().enumerate() {
        for n in 0..c.data.len() {
            let mut chunks = good.chunks.clone();
            chunks[i].data.truncate(n);
            let got = parse(chunks.clone());
            assert!(
                matches!(got, Err(Error::Corrupt(_))),
                "{:?} prefix {n}: {got:?}",
                c.tag
            );
            // A decoder still decodes the image carrying it.
            if n % 7 == 0 {
                let meta = Metadata {
                    chunks,
                    ..Default::default()
                };
                let file = lossless()
                    .encode_samples(imij::ImageRef::new(&px, 10, &info), &meta)
                    .unwrap();
                assert_eq!(imij::decode(&file).unwrap(), pixels);
            }
        }
        for at in 1..c.data.len() {
            for bit in 0..8 {
                let mut chunks = good.chunks.clone();
                chunks[i].data[at] ^= 1 << bit;
                let _ = parse(chunks);
            }
        }
        let mut chunks = good.chunks.clone();
        chunks[i].data.push(0);
        assert!(parse(chunks).is_err(), "{:?} trailing byte", c.tag);
    }
    let one = |tag: &[u8; 4], data: &[u8]| {
        parse(vec![Chunk {
            tag: *tag,
            data: data.to_vec(),
        }])
    };
    // Unknown versions and structured formats are skipped.
    assert!(one(b"cALB", &[3, 0xff]).unwrap().is_empty());
    assert!(one(b"aXES", &[9]).unwrap().is_empty());
    assert!(one(b"sMDT", &[1, 7, 1, b'a', 0, 0]).unwrap().is_empty());
    // Reserved flag bits, counts, kinds and bindings.
    assert!(one(b"cALB", &[1, 1, 0x80, 0x02, 0]).is_err());
    assert!(one(b"cALB", &[1, 0]).is_err());
    assert!(one(b"cALB", &[1, 5]).is_err());
    assert!(one(b"cALB", &[1, 1, 0, 17]).is_err());
    assert!(one(b"aXES", &[1, 8, 2]).is_err());
    assert!(one(b"aXES", &[1, 0, 1, 1, b'y', 0, 2, 0]).is_err());
    let yx = [1, 0, 2, 1, b'y', 0, 2, 0, 1, b'x', 0, 1, 0, 0];
    assert!(one(b"aXES", &yx).unwrap().axes.is_some());
    for (at, v) in [(6, 5), (5, 4), (11, 2), (2, 1), (2, 33)] {
        let mut bad = yx;
        bad[at] = v;
        assert!(one(b"aXES", &bad).is_err(), "{at} {v}");
    }
    // Coordinates need axes with as many coordinate-bound axes.
    assert!(one(b"cORD", &[1, 1, 2]).is_err());
    let mut chunks = vec![Chunk {
        tag: *b"aXES",
        data: yx.to_vec(),
    }];
    chunks.push(Chunk {
        tag: *b"cORD",
        data: vec![1, 1, 2],
    });
    assert!(parse(chunks.clone()).is_err());
    // A newer axes version: its coordinates are skipped with it.
    chunks[0].data[0] = 2;
    assert!(parse(chunks).unwrap().is_empty());
    // Repeated records, repeated schemas, bad UTF-8, bad packing.
    let mut twice = good.chunks.clone();
    twice.push(good.chunks[0].clone());
    assert_eq!(parse(twice), Err(Error::Corrupt("repeated science record")));
    let mut twice = good.chunks.clone();
    twice.push(good.chunks[3].clone());
    assert_eq!(
        parse(twice),
        Err(Error::Corrupt("structured metadata schema"))
    );
    assert!(one(b"sMDT", &[1, 1, 1, b'j', 0, 1, 0xff]).is_err());
    assert!(one(b"sMDT", &[1, 2, 1, b'c', 0, 1, 0xff]).is_ok());
    assert!(one(b"sMDT", &[1, 0, 1, b'j', 9, 1, 0xff]).is_err());
    assert!(one(b"sMDT", &[1, 0, 1, b' ', 0, 0]).is_err());
    // Non-finite or zero slope, NaN nodata, inverted range.
    let mut c = vec![1, 1, 1];
    c.extend_from_slice(&0f64.to_le_bytes());
    c.extend_from_slice(&0f64.to_le_bytes());
    c.push(0);
    assert!(one(b"cALB", &c).is_err());
    let mut c = vec![1, 1, 0, 1];
    c.extend_from_slice(&f64::NAN.to_le_bytes());
    assert!(one(b"cALB", &c).is_err());
    let mut c = vec![1, 1, 0x30];
    c.extend_from_slice(&2f64.to_le_bytes());
    c.extend_from_slice(&1f64.to_le_bytes());
    c.push(0);
    assert!(one(b"cALB", &c).is_err());
}

#[test]
fn writer_rejects_invalid_records() {
    let gray = ImageInfo::new(4, 4, Channels::Gray, SampleType::U8);
    let bad = |f: &dyn Fn(&mut ScienceRecords)| {
        let mut r = ct_records();
        f(&mut r);
        let mut meta = Metadata::default();
        let e = r.write_metadata(&gray, &mut meta).unwrap_err();
        assert!(
            matches!(e, Error::InvalidInput(_) | Error::TooLarge),
            "{e:?}"
        );
        assert!(meta.chunks.is_empty());
    };
    bad(&|r| r.calibration.push(ChannelCalibration::default()));
    bad(&|r| r.calibration[0].slope = 0.0);
    bad(&|r| r.calibration[0].offset = f64::INFINITY);
    bad(&|r| r.calibration[0].unit = Some(String::new()));
    bad(&|r| r.calibration[0].quantity = Some("a\0b".into()));
    bad(&|r| {
        r.calibration[0].quantity = None;
        r.calibration[0].quantity_is_standard_name = true;
    });
    bad(&|r| r.calibration[0].nodata = vec![f64::NAN]);
    bad(&|r| r.calibration[0].nodata = vec![0.0; 17]);
    bad(&|r| r.calibration[0].valid_min = Some(5000.0));
    bad(&|r| r.axes.as_mut().unwrap().axes[0].name = "y".into());
    bad(&|r| r.axes.as_mut().unwrap().axes[0].binding = AxisBinding::Rows);
    bad(&|r| r.axes.as_mut().unwrap().axes.truncate(1));
    bad(&|r| r.axes.as_mut().unwrap().levels[0].scale[1] = 0.0);
    bad(&|r| {
        r.axes.as_mut().unwrap().levels[0].translation.pop();
    });
    bad(&|r| {
        r.axes.as_mut().unwrap().affine.as_mut().unwrap().pop();
    });
    bad(&|r| r.axes.as_mut().unwrap().downsampling = Some(String::new()));
    bad(&|r| r.coordinates = vec![1, 2]);
    bad(&|r| r.axes = None);
    bad(&|r| r.structured[1].schema = "dicom-json".into());
    bad(&|r| r.structured[0].schema = "has space".into());
    bad(&|r| r.structured[0].data = vec![0xff]);
    // Planes are scalar and have no frames; defaults have no coordinates.
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
    let mut r = ct_records();
    r.axes.as_mut().unwrap().axes[0].binding = AxisBinding::Frames;
    r.coordinates.clear();
    assert!(b.set_science(Some(id), r).is_err());
    assert!(b.set_science(None, ct_records()).is_err());
    let mut r = ct_records();
    r.calibration.push(ChannelCalibration::default());
    assert!(b.set_science(Some(id), r).is_err());
    assert!(b.set_science(Some(id), ct_records()).is_ok());
    // A mask reference past the last plane is caught when finishing.
    let mut r = ct_records();
    r.calibration[0].mask_plane = Some(1);
    b.set_science(Some(id), r).unwrap();
    b.add_level(
        id,
        LevelDescriptor::new(4, 4, 4, 4),
        &[0; 16],
        4,
        lossless(),
    )
    .unwrap();
    assert!(matches!(b.finish(), Err(Error::InvalidInput(_))));
}

#[test]
fn calibration_semantics() {
    let mut c = ChannelCalibration::default();
    c.slope = 0.01;
    c.offset = 273.15;
    c.nodata = vec![-32767.0];
    c.valid_min = Some(-30000.0);
    c.valid_max = Some(30000.0);
    assert_eq!(c.physical(100.0), 0.01 * 100.0 + 273.15);
    assert!(c.is_missing(-32767.0));
    assert!(c.is_missing(-30001.0) && c.is_missing(30001.0));
    assert!(!c.is_missing(30000.0) && !c.is_missing(-30000.0));
    assert!(!c.is_missing(f64::NAN));
    c.nan_is_missing = true;
    assert!(c.is_missing(f64::NAN));
    let mut z = ChannelCalibration::default();
    z.nodata = vec![0.0];
    assert!(z.is_missing(-0.0));
}

/// docs/SCIENCE_METADATA.md: the CF example's exact `cALB` bytes.
#[test]
fn cf_calibration_bytes() {
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U16);
    let mut c = ChannelCalibration::default();
    c.slope = 0.01;
    c.offset = 273.15;
    c.unit = Some("K".into());
    c.quantity = Some("air_temperature".into());
    c.quantity_is_standard_name = true;
    c.nodata = vec![65535.0];
    let mut r = ScienceRecords::default();
    r.calibration.push(c);
    let meta = with_records(&info, &r);
    let mut want = vec![1u8, 1, 0x0f];
    want.extend_from_slice(&0.01f64.to_le_bytes());
    want.extend_from_slice(&273.15f64.to_le_bytes());
    want.extend_from_slice(b"\x01K\x0fair_temperature\x01");
    want.extend_from_slice(&65535f64.to_le_bytes());
    assert_eq!(meta.chunks.len(), 1);
    assert_eq!(meta.chunks[0].tag, *b"cALB");
    assert_eq!(meta.chunks[0].data, want);
    assert_eq!(want.len(), 46);
}

/// docs/SCIENCE_METADATA.md: the OME-NGFF 0.5 specification's example
/// (t, c, z, y, x; three levels; a multiscale-level time scale).
#[test]
fn ngff_example_maps_one_to_one() {
    let mut axes = Axes::new(vec![
        axis(
            "t",
            AxisKind::Time,
            AxisBinding::Coordinate,
            Some("millisecond"),
        ),
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
    for s in [0.5, 1.0, 2.0] {
        axes.levels
            .push(AxisLevel::new(vec![1.0, 1.0, s, s, s], vec![0.0; 5]));
    }
    // The multiscale-level scale [0.1, 1, 1, 1, 1] is a diagonal affine.
    let mut a = vec![0.0; 30];
    for (i, s) in [0.1, 1.0, 1.0, 1.0, 1.0].into_iter().enumerate() {
        a[i * 6 + i] = s;
    }
    axes.affine = Some(a);
    axes.downsampling = Some("gaussian".into());
    // Level 1, t = 3, c = 1, z = 2, y = 10, x = 20.
    let p = axes.to_physical(1, &[3.0, 1.0, 2.0, 10.0, 20.0]).unwrap();
    assert!((p[0] - 0.3).abs() < 1e-12);
    assert_eq!(&p[1..], &[1.0, 2.0, 10.0, 20.0]);
    assert_eq!(axes.to_physical(3, &[0.0; 5]), None);
    let mut r = ScienceRecords::default();
    r.axes = Some(axes);
    r.coordinates = vec![3, 1, 2];
    let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U16);
    let meta = with_records(&info, &r);
    assert_eq!(ScienceRecords::from_metadata(&info, &meta).unwrap(), r);
}

/// docs/SCIENCE_METADATA.md: a GDAL GeoTransform and a DICOM slice.
#[test]
fn geotiff_and_dicom_affines() {
    // GT = (440720, 60, 0, 3751320, 0, -60), PixelIsArea; axes (y, x).
    let gt = [440720.0, 60.0, 0.0, 3751320.0, 0.0, -60.0];
    let mut g = Axes::new(vec![
        axis("northing", AxisKind::Space, AxisBinding::Rows, Some("m")),
        axis("easting", AxisKind::Space, AxisBinding::Columns, Some("m")),
    ]);
    g.levels
        .push(AxisLevel::new(vec![1.0, 1.0], vec![0.0, 0.0]));
    g.affine = Some(vec![gt[5], gt[4], gt[3], gt[2], gt[1], gt[0]]);
    g.pixel_is_area = true;
    assert_eq!(
        g.to_physical(0, &[0.0, 0.0]).unwrap(),
        [3751320.0, 440720.0]
    );
    // Center of pixel (row 2, column 3): index + 0.5 under PixelIsArea.
    let (row, col) = (2.5, 3.5);
    assert_eq!(
        g.to_physical(0, &[row, col]).unwrap(),
        [
            gt[3] + col * gt[4] + row * gt[5],
            gt[0] + col * gt[1] + row * gt[2]
        ]
    );
    // DICOM: P = S + i * di * X + j * dj * Y (+ k * dk * N for the slice),
    // S = (-125, -110, 50) mm at the first voxel's center; rows run along
    // Y = (0, 1, 0), columns along X = (1, 0, 0); spacing 0.5 x 0.6, slices
    // 2.5 apart along N = (0, 0, 1). Axes (k, j, i) = (slice, row, column),
    // physical outputs named (z, y, x) in patient coordinates.
    let mut d = Axes::new(vec![
        axis(
            "z",
            AxisKind::Space,
            AxisBinding::Coordinate,
            Some("millimeter"),
        ),
        axis("y", AxisKind::Space, AxisBinding::Rows, Some("millimeter")),
        axis(
            "x",
            AxisKind::Space,
            AxisBinding::Columns,
            Some("millimeter"),
        ),
    ]);
    d.levels
        .push(AxisLevel::new(vec![2.5, 0.6, 0.5], vec![0.0, 0.0, 0.0]));
    d.affine = Some(vec![
        1.0, 0.0, 0.0, 50.0, //
        0.0, 1.0, 0.0, -110.0, //
        0.0, 0.0, 1.0, -125.0,
    ]);
    let (k, j, i) = (4.0, 10.0, 20.0);
    assert_eq!(
        d.to_physical(0, &[k, j, i]).unwrap(),
        [50.0 + k * 2.5, -110.0 + j * 0.6, -125.0 + i * 0.5]
    );
}

#[cfg(feature = "extended")]
#[test]
fn fits_int16_through_i32() {
    // FITS BITPIX = 16, BZERO = 32768 (unsigned data), BLANK = -32768,
    // BUNIT = 'adu', stored losslessly as I32 with the original header.
    let info = ImageInfo::new(6, 5, Channels::Gray, SampleType::I32);
    let px: Vec<u8> = (0..30i32)
        .flat_map(|i| (if i == 7 { -32768 } else { i * 997 - 15000 }).to_ne_bytes())
        .collect();
    let mut c = ChannelCalibration::default();
    c.offset = 32768.0;
    c.unit = Some("count".into());
    c.nodata = vec![-32768.0];
    let mut r = ScienceRecords::default();
    r.calibration.push(c);
    let cards: String = [
        "SIMPLE  =                    T",
        "BITPIX  =                   16",
        "NAXIS   =                    2",
        "BZERO   =                32768",
        "BLANK   =               -32768",
        "BUNIT   = 'adu     '",
        "END",
    ]
    .iter()
    .map(|c| format!("{c:<80}"))
    .collect();
    r.structured.push(StructuredMetadata::new(
        "fits-header",
        StructuredFormat::Text,
        cards.into_bytes(),
    ));
    let meta = with_records(&info, &r);
    let file = lossless()
        .encode_samples(imij::ImageRef::new(&px, 24, &info), &meta)
        .unwrap();
    assert_eq!(imij::decode(&file).unwrap().1, px);
    let (pi, pm) = imij::probe(&file).unwrap();
    let got = ScienceRecords::from_metadata(&pi, &pm).unwrap();
    assert_eq!(got, r);
    let cal = &got.calibration[0];
    assert!(cal.is_missing(-32768.0));
    assert_eq!(cal.physical(-32767.0), 1.0);
}

#[cfg(feature = "lab")]
#[test]
fn science_records_coexist_with_actual_lossy_seed_tables() {
    let mut seeded = 0;
    for size in [16, 64] {
        let info = ImageInfo::new(size, size, Channels::Gray, SampleType::U16);
        let px = gray16(size, size, 5);
        let records = ct_records();
        let meta = with_records(&info, &records);
        for quality in [5.0, 30.0, 70.0, 90.0] {
            let cfg = imij::ImiEncoderConfig::lossy(quality);
            let plain = cfg
                .encode_samples(
                    imij::ImageRef::new(&px, info.row_bytes(), &info),
                    &Metadata::default(),
                )
                .unwrap();
            let file = cfg
                .encode_samples(imij::ImageRef::new(&px, info.row_bytes(), &info), &meta)
                .unwrap();
            let seed = imij::seeds::lossy_entropy_seed(&file).unwrap();
            seeded += usize::from(seed.2 != 0);
            assert_eq!(imij::seeds::lossy_entropy_seed(&plain).unwrap(), seed);
            assert_only_inserted(&plain, &file);
            let expected = imij::decode(&plain).unwrap().1;
            assert_eq!(imij::decode(&file).unwrap().1, expected);
            let (pinfo, pmeta) = imij::probe(&file).unwrap();
            assert_eq!(
                ScienceRecords::from_metadata(&pinfo, &pmeta).unwrap(),
                records
            );
            let mut rows = Vec::new();
            imij::decode_reader(file.as_slice(), &imij::Limits::DEFAULT, |_, _, row| {
                rows.extend_from_slice(row);
                Ok(())
            })
            .unwrap();
            assert_eq!(rows, expected);
            let mut strided = vec![0xA5; (info.row_bytes() + 7) * size as usize];
            imij::decode_into_threads(&file, &mut strided, info.row_bytes() + 7, 4).unwrap();
            for (y, row) in strided.chunks_exact(info.row_bytes() + 7).enumerate() {
                assert_eq!(
                    &row[..info.row_bytes()],
                    &expected[y * info.row_bytes()..][..info.row_bytes()]
                );
                assert!(row[info.row_bytes()..].iter().all(|&x| x == 0xA5));
            }
        }
    }
    assert!(seeded > 0, "matrix did not exercise Lossy 0.2 seed tables");
}
