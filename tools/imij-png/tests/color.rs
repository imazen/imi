use enough::Unstoppable;
use imij_png::color_metadata;

fn parsed(config: zenpng::EncodeConfig) -> zenpng::PngInfo {
    let pixels = [rgb::Rgb::new(7u8, 19, 43); 4];
    let bytes = zenpng::encode_rgb8(
        imgref::ImgRef::new(&pixels, 2, 2),
        None,
        &config,
        &Unstoppable,
        &Unstoppable,
    )
    .unwrap();
    zenpng::probe(&bytes).unwrap()
}

#[test]
fn legacy_policy_and_precedence() {
    for (gamma, transfer) in [(45454, 13), (45455, 13), (100000, 8)] {
        let p = parsed(zenpng::EncodeConfig::default().with_source_gamma(Some(gamma)));
        assert_eq!(
            color_metadata(&p).unwrap().0.transfer_characteristics,
            transfer
        );
    }
    let mut p = parsed(zenpng::EncodeConfig::default().with_source_gamma(Some(50000)));
    assert!(matches!(
        color_metadata(&p),
        Err(imij::Error::Unsupported(_))
    ));
    p.srgb_intent = Some(0);
    assert_eq!(color_metadata(&p).unwrap().0, imij::Cicp::SRGB);
    p.icc_profile = Some(vec![1, 2, 3]);
    assert_eq!(color_metadata(&p).unwrap().1.icc, p.icc_profile);
    p.cicp = Some(zencodec::Cicp::new(9, 16, 0, true));
    let (c, m) = color_metadata(&p).unwrap();
    assert_eq!((c.color_primaries, c.transfer_characteristics), (9, 16));
    assert!(m.icc.is_none());
    let origin = m.color_origin().unwrap().unwrap();
    assert_eq!(origin.icc.as_deref(), Some(&[1, 2, 3][..]));
    assert_eq!(origin.cicp, p.cicp);
    assert_eq!(origin.color_authority, zenpixels::ColorAuthority::Cicp);
    assert_eq!(origin.provenance, zenpixels::ColorProvenance::Cicp);
    p.cicp.as_mut().unwrap().full_range = false;
    assert!(color_metadata(&p).is_err());
}

#[test]
fn raw_hdr_survives_imij() {
    let p =
        parsed(zenpng::EncodeConfig::default().with_content_light_level_raw(Some([u32::MAX, 1])));
    let (c, m) = color_metadata(&p).unwrap();
    assert_eq!(
        m.content_light_level,
        Some([255, 255, 255, 255, 0, 0, 0, 1])
    );
    let mut info = imij::ImageInfo::new(2, 2, imij::Channels::Rgb, imij::SampleType::U8);
    info.cicp = c;
    let pixels = [7, 19, 43].repeat(4);
    let file = imij::ImiEncoderConfig::default()
        .encode_samples(imij::ImageRef::new(&pixels, 6, &info), &m)
        .unwrap();
    assert_eq!(
        imij::probe(&file).unwrap().1.content_light_level,
        m.content_light_level
    );
    assert_eq!(imij::decode(&file).unwrap().1, pixels);
}

#[test]
fn chromaticity_mapping_is_bounded() {
    let mut p = parsed(zenpng::EncodeConfig::default());
    p.srgb_intent = None;
    p.cicp = None;
    for (code, v) in [
        (1, [31270, 32900, 64000, 33000, 30000, 60000, 15000, 6000]),
        (12, [31270, 32900, 68000, 32000, 26500, 69000, 15000, 6000]),
        (9, [31270, 32900, 70800, 29200, 17000, 79700, 13100, 4600]),
    ] {
        p.chromaticities = Some(zenpng::PngChromaticities {
            white_x: v[0],
            white_y: v[1],
            red_x: v[2],
            red_y: v[3],
            green_x: v[4],
            green_y: v[5],
            blue_x: v[6],
            blue_y: v[7],
        });
        assert_eq!(color_metadata(&p).unwrap().0.color_primaries, code);
        p.chromaticities.as_mut().unwrap().red_x += 1;
        assert_eq!(color_metadata(&p).unwrap().0.color_primaries, code);
        p.chromaticities.as_mut().unwrap().red_x += 1;
        assert!(color_metadata(&p).is_err());
    }
}

#[test]
fn legacy_and_assumed_origins_are_distinct_from_active_cicp() {
    let mut p = parsed(zenpng::EncodeConfig::default());
    p.cicp = None;
    p.icc_profile = None;
    p.srgb_intent = None;
    p.source_gamma = None;
    p.chromaticities = None;
    let (active, meta) = color_metadata(&p).unwrap();
    assert_eq!(active, imij::Cicp::SRGB);
    assert_eq!(
        meta.color_origin().unwrap(),
        Some(zenpixels::ColorOrigin::assumed())
    );
    p.source_gamma = Some(45455);
    let (active, meta) = color_metadata(&p).unwrap();
    assert_eq!(active, imij::Cicp::SRGB);
    assert_eq!(
        meta.color_origin().unwrap(),
        Some(zenpixels::ColorOrigin::from_gama_chrm())
    );
}
