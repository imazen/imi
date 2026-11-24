use super::*;
use crate::{Channels, SampleType};

#[test]
fn transfer_table_is_shared_and_smallest_layout_cannot_evade_its_budget() {
    let cfg = LossyConfig::new(80.0);
    let full = ImageInfo::new(1, 1, Channels::Rgba, SampleType::U8);
    let alpha_only = full.clone().with_significant_bits(&[8, 8, 8, 1]).unwrap();
    let color = full.clone().with_significant_bits(&[7, 8, 8, 8]).unwrap();
    let table = crate::lossy::decoder_transfer_table_bytes(&color, cfg.color);
    assert!(table > 0);
    for threads in [1, 4] {
        let baseline = lossy_bytes(&full, &cfg, threads);
        assert_eq!(lossy_bytes(&alpha_only, &cfg, threads), baseline);
        assert_eq!(lossy_bytes(&color, &cfg, threads), baseline + table);
    }
    let baseline = cfg.estimated_decoder_memory(&full).unwrap();
    let limited = cfg
        .clone()
        .with_decoder_memory_limit(DecoderMemoryLimit::new(baseline));
    assert!(plan_lossy(&full, &limited).is_ok());
    assert!(plan_lossy(&color, &limited).is_err());
    let limited = cfg
        .clone()
        .with_decoder_memory_limit(DecoderMemoryLimit::new(baseline + table as u64));
    assert_eq!(
        plan_lossy(&color, &limited).unwrap().1,
        baseline + table as u64
    );
    for cicp in [
        crate::Cicp::SRGB,
        crate::Cicp::BT2100_PQ,
        crate::Cicp::BT2100_HLG,
    ] {
        let mut info = ImageInfo::new(1, 1, Channels::Rgb, SampleType::U16);
        info.cicp = cicp;
        let ycc = cfg.clone().with_color(crate::LossyColor::YCoCg);
        assert_eq!(
            lossy_bytes(&info, &cfg, 1) - lossy_bytes(&info, &ycc, 1),
            table
        );
    }
}

#[test]
fn reduces_stripes_then_groups_and_accounts_for_threads() {
    let info = ImageInfo::new(8193, 1025, Channels::Rgba, SampleType::U16);
    let mut short = LossyConfig::new(20.0);
    short.column_groups = Some(0);
    let bytes = short.estimated_decoder_memory(&info).unwrap();
    let cfg = short
        .with_stripe_block_rows(8)
        .with_decoder_memory_limit(DecoderMemoryLimit::new(bytes));
    let (plan, used) = plan_lossy(&info, &cfg).unwrap();
    assert_eq!(plan.stripe_block_rows, 1);
    assert_eq!(plan.column_groups, Some(0));
    assert!(used <= bytes);
    // More target threads consume more scratch regardless of the encoding host.
    let cfg = cfg.with_decoder_memory_limit(DecoderMemoryLimit::new(bytes).with_threads(4));
    let (plan, used) = plan_lossy(&info, &cfg).unwrap();
    assert!(plan.column_groups.is_some_and(|g| g > 0 && g < info.width));
    assert!(used <= bytes);
}

#[test]
fn restores_lanes_before_disabling_palette() {
    let info = ImageInfo::new(2049, 257, Channels::Rgba, SampleType::U8);
    let ordinary = EncodeConfig::default();
    let bytes = ordinary.estimated_decoder_memory(&info).unwrap();
    let cfg = EncodeConfig {
        lanes: Some(1),
        ..ordinary
    }
    .with_decoder_memory_limit(DecoderMemoryLimit::new(bytes));
    let (plan, used) = plan_lossless(&info, &cfg).unwrap();
    assert_eq!(plan.lanes, Some(16));
    assert!(plan.palette);
    assert!(used <= bytes);
}

#[test]
fn can_disable_palette_but_cannot_remove_the_streaming_row_floor() {
    let info = ImageInfo::new(2049, 257, Channels::Rgba, SampleType::U8);
    let without = EncodeConfig {
        palette: false,
        ..Default::default()
    };
    let bytes = without.estimated_decoder_memory(&info).unwrap();
    let cfg = EncodeConfig::default().with_decoder_memory_limit(DecoderMemoryLimit::new(bytes));
    let (plan, _) = plan_lossless(&info, &cfg).unwrap();
    assert!(!plan.palette);
    let cfg = cfg.with_decoder_memory_limit(DecoderMemoryLimit::new(1024));
    assert!(plan_lossless(&info, &cfg).is_err());
}

#[test]
fn arithmetic_and_impossible_limits_do_not_wrap_or_allocate() {
    let info = ImageInfo::new(u32::MAX, u32::MAX, Channels::Rgba, SampleType::F32);
    let cfg = EncodeConfig {
        stripe_rows: Some(1),
        column_groups: Some(1),
        ..Default::default()
    };
    assert_eq!(cfg.estimated_decoder_memory(&info), Err(Error::TooLarge));
    assert!(
        cfg.with_decoder_memory_limit(DecoderMemoryLimit::new(1))
            .estimated_decoder_memory(&info)
            .is_err()
    );
    assert!(
        LossyConfig::default()
            .with_decoder_memory_limit(DecoderMemoryLimit::new(1))
            .estimated_decoder_memory(&info)
            .is_err()
    );
}

#[test]
fn row_admission_covers_observed_simultaneous_lossless_groups() {
    use crate::decode_resources::{DecodePolicy, DecodeSession};
    let info = ImageInfo::new(257, 7, Channels::Rgba, SampleType::U16);
    let pixels: alloc::vec::Vec<u8> = (0..257 * 7 * 4)
        .flat_map(|i| ((i * 997) as u16).to_ne_bytes())
        .collect();
    let cfg = EncodeConfig {
        lanes: Some(1),
        column_groups: Some(8),
        stripe_rows: Some(3),
        palette: false,
        ..Default::default()
    };
    let file = crate::encode(
        &pixels,
        info.row_bytes(),
        &info,
        &crate::Metadata::default(),
        &cfg,
    )
    .unwrap();
    let parsed = crate::v1::parse(&file).unwrap();
    let mut stream = crate::v1::LosslessStream::from_parsed(parsed);
    assert_eq!(stream.retained_group_capacity(), (0, 0));
    let mut row = alloc::vec![0; info.row_bytes()];
    stream.next_row(&mut row).unwrap();
    let (groups, capacity) = stream.retained_group_capacity();
    assert_eq!(groups, 33);
    assert_eq!(&row, &pixels[..info.row_bytes()]);
    let mut serial = DecodeSession::new(DecodePolicy::default());
    serial.decode(&file).unwrap();
    let mut rows = DecodeSession::new(DecodePolicy::default());
    rows.decode_rows(&file, &mut row, |_, _, _| Ok(())).unwrap();
    assert!(capacity as u64 <= rows.usage().peak_planned_working_bytes);
    assert!(rows.usage().peak_planned_working_bytes > serial.usage().peak_planned_working_bytes);
}

// Re-encode only the nested DC lossless image of a real lossy payload
// (encoded with full restart states, so AC records are stored as-is, and
// without the unit table hint, so no span list needs rewriting). The
// AC/alpha records and outer quantization/color settings remain unchanged;
// the new DC head carries explicit tables, so the seed byte loses its DC
// scope. Walks the payload by SPEC 2.8/2.7.
fn with_dc_stripe_height(file: &[u8], hs: u32) -> alloc::vec::Vec<u8> {
    use crate::parse::records::{R_AC, R_ALPHA, R_DC, put_record, read_record};
    fn nested<'a>(
        info: ImageInfo,
        bytes: &[u8],
        bank: Option<&'static crate::seeds::Bank>,
    ) -> crate::v1::Parsed<'a> {
        crate::v1::parse_head_nested(
            info,
            &mut crate::ByteReader::new(bytes),
            &crate::parse::limits::Unrestricted,
            bank,
            crate::parse::header::Features::ALL,
        )
        .unwrap()
    }
    let legacy = crate::lossy::legacy_seed_fixture(file);
    let file = &legacy[..];
    let header = crate::v1::header(file).unwrap();
    let (info, meta, body) = crate::v1::parse_header(file).unwrap();
    let mut r = crate::ByteReader::new(body);
    let sb = r.u8().unwrap() as usize;
    let gcols = r.var_usize().unwrap();
    // The parameters: `present`, the quantizer scale, then the fields in
    // presence-bit order; the seed byte is the first.
    let present_at = body.len() - r.remaining();
    let present = r.varint().unwrap();
    r.bytes(4).unwrap();
    let seed = if present & 1 != 0 { r.u8().unwrap() } else { 0 };
    for (bit, n) in [(1, 1), (2, 1), (3, 3), (6, 12), (7, 4), (8, 12)] {
        if present >> bit & 1 != 0 {
            r.bytes(n).unwrap();
        }
    }
    let tb = r.var_usize().unwrap();
    r.bytes(tb).unwrap();
    // Head records: the DC image's options.
    let (mut four, mut dc32) = (false, false);
    if present & 16 != 0 {
        let n = r.var_usize().unwrap();
        crate::parse::records::head_records(r.bytes(n).unwrap(), |id, _| {
            assert_ne!(id, 2, "no unit table hint");
            four |= matches!(id, 5 | 7 | 9);
            dc32 |= id == 11;
            Ok(())
        })
        .unwrap();
    }
    let head_start = body.len() - r.remaining();
    let dc_bank = (seed & crate::seeds::SCOPE_DC != 0)
        .then(|| crate::seeds::bank(u64::from(seed & 15)).unwrap());
    // The parameters with the DC scope dropped from the seed byte (and the
    // byte itself when no scope is left; `present` keeps its length).
    let mut params = body[present_at..head_start].to_vec();
    let mut new_seed = seed;
    if dc_bank.is_some() {
        new_seed = seed & !crate::seeds::SCOPE_DC;
        let seed_at = crate::varint_len(present) + 4;
        if new_seed & 0x70 == 0 {
            new_seed = 0;
            params[0] &= !1;
            params.remove(seed_at);
        } else {
            params[seed_at] = new_seed;
        }
    }
    let dc_len = r.var_usize().unwrap();
    let dc_head = r.bytes(dc_len).unwrap();
    let di = ImageInfo::new(
        info.width.div_ceil(8),
        info.height.div_ceil(8),
        if four { Channels::Rgba } else { Channels::Rgb },
        if dc32 {
            SampleType::I32
        } else {
            SampleType::U16
        },
    );
    let mut dc = nested(di.clone(), dc_head, dc_bank);
    let alpha_head = if info.channels.has_alpha() {
        let n = r.var_usize().unwrap();
        Some(r.bytes(n).unwrap())
    } else {
        None
    };
    let groups = if gcols == 0 {
        1
    } else {
        (info.width as usize).div_ceil(gcols)
    };
    let ns = (info.height as usize).div_ceil(8).div_ceil(sb);
    let k = dc.hs / sb;
    let mut alpha_records = alloc::vec::Vec::new();
    let mut ac_records = alloc::vec::Vec::new();
    let mut next = |kind: u64| {
        let (id, rec) = read_record(&mut r).unwrap();
        assert_eq!(id, kind);
        rec
    };
    for u in 0..ns * groups {
        if (u / groups).is_multiple_of(k) {
            dc.stripes.push(next(R_DC));
        }
        if alpha_head.is_some() {
            alpha_records.push(next(R_ALPHA));
        }
        ac_records.push(next(R_AC));
    }
    assert_eq!(r.remaining(), 0);
    let mut stream = crate::v1::LosslessStream::from_parsed(dc);
    let mut px = alloc::vec![0;di.checked_buffer_len().unwrap()];
    for row in px.chunks_exact_mut(di.row_bytes()) {
        assert!(stream.next_row(row).unwrap());
    }
    let cfg = EncodeConfig {
        stripe_rows: Some(hs),
        column_groups: Some((gcols / 8) as u32),
        palette: false,
        ..Default::default()
    };
    let (head, records, _) = crate::v1::encode_parts(&px, di.row_bytes(), &di, &cfg).unwrap();
    let mut data = body[..present_at].to_vec();
    data.extend_from_slice(&params);
    crate::put_varint(&mut data, head.len() as u64);
    data.extend(head);
    if let Some(a) = alpha_head {
        crate::put_varint(&mut data, a.len() as u64);
        data.extend(a);
    }
    let nk = hs as usize / sb;
    for (u, ac) in ac_records.iter().enumerate() {
        let si = u / groups;
        let gi = u % groups;
        if si.is_multiple_of(nk) {
            put_record(&mut data, R_DC, &records[si / nk * groups + gi]);
        }
        if alpha_head.is_some() {
            put_record(&mut data, R_ALPHA, alpha_records[u]);
        }
        put_record(&mut data, R_AC, ac);
    }
    // As the encoder: a file whose parameters keep seed tables declares
    // `seed_bank_1`.
    let features = if new_seed != 0 {
        crate::parse::header::Features::EMPTY.with(crate::parse::header::Feature::SeedBank1)
    } else {
        crate::parse::header::Features::EMPTY
    };
    let mut out = alloc::vec::Vec::new();
    crate::container::write_head_features(
        &mut out,
        &info,
        &meta,
        crate::Coding::Lossy,
        header.lossy_color,
        crate::DecodeSpeed::Balanced,
        features,
    )
    .unwrap();
    crate::container::put_data(&mut out, &data);
    out
}

#[test]
fn row_admission_observes_nested_groups_and_actual_legal_dc_stripe_height() {
    use crate::decode_resources::{DecodePolicy, DecodeSession};
    let info = ImageInfo::new(65, 145, Channels::Rgba, SampleType::U16);
    let pixels: alloc::vec::Vec<u8> = (0..65 * 145 * 4)
        .flat_map(|i| ((i * 997) as u16).to_ne_bytes())
        .collect();
    // One-block-row lossy stripes, so DC stripe heights 1 and 32 are legal.
    let cfg = LossyConfig {
        column_groups: Some(8),
        compact_streams: false,
        aq_gamma: 0.5,
        stripe_block_rows: 1,
        ..LossyConfig::new(55.0)
    };
    // `with_dc_stripe_height` rewrites payloads without the unit table hint.
    let original = crate::v1::without_hint(|| {
        crate::encode_lossy(
            &pixels,
            info.row_bytes(),
            &info,
            &crate::Metadata::default(),
            &cfg,
        )
        .unwrap()
    });
    let expected = crate::decode(&original).unwrap().1;
    let (mut plans, mut caps) = (alloc::vec::Vec::new(), alloc::vec::Vec::new());
    for hs in [1, 32] {
        let file = with_dc_stripe_height(&original, hs);
        assert_eq!(crate::decode(&file).unwrap().1, expected);
        let (_, _, body, lossy) = crate::v1::parse_header_lossy(&file).unwrap();
        let color = crate::v1::header(&file).unwrap().lossy_color.unwrap();
        let mut stream = crate::lossy::LossyStream::new(
            info.clone(),
            crate::parse::header::HEADER_VERSION,
            color,
            lossy,
            body,
        )
        .unwrap();
        let mut row = alloc::vec![0;info.row_bytes()];
        stream.next_row(&mut row).unwrap();
        let (live, capacity) = stream.retained_nested_capacity();
        // Every group's first DC record decoded with stripe 0 (its whole DC
        // stripe: one DC row with hs1, the whole DC image with hs32), and
        // the stripe's alpha rows.
        assert_eq!(live, 9);
        caps.push(capacity);
        let mut session = DecodeSession::new(DecodePolicy::default());
        let mut out = alloc::vec::Vec::new();
        session
            .decode_rows(&file, &mut row, |_, _, r| {
                out.extend_from_slice(r);
                Ok(())
            })
            .unwrap();
        assert_eq!(out, expected);
        assert!(capacity as u64 <= session.usage().peak_planned_working_bytes);
        plans.push(session.usage().peak_planned_working_bytes);
    }
    assert!(plans[1] > plans[0]);
    assert!(caps[1] > caps[0]);
}

#[test]
fn roomy_plans_preserve_requested_geometry_and_serialized_bytes() {
    let info = ImageInfo::new(65, 25, Channels::Rgba, SampleType::U16);
    let pixels: alloc::vec::Vec<u8> = (0..65 * 25 * 4)
        .flat_map(|i| ((i * 997) as u16).to_ne_bytes())
        .collect();
    let cfg = EncodeConfig {
        stripe_rows: Some(3),
        column_groups: Some(7),
        lanes: Some(1),
        ..Default::default()
    };
    let estimate = cfg.estimated_decoder_memory(&info).unwrap();
    let budgeted = cfg
        .clone()
        .with_decoder_memory_limit(DecoderMemoryLimit::new(estimate));
    let (plan, used) = plan_lossless(&info, &budgeted).unwrap();
    assert_eq!(used, estimate);
    assert_eq!(plan.column_groups, cfg.column_groups);
    assert_eq!(plan.stripe_rows, cfg.stripe_rows);
    assert_eq!(plan.lanes, cfg.lanes);
    assert_eq!(plan.palette, cfg.palette);
    let original = crate::encode(
        &pixels,
        info.row_bytes(),
        &info,
        &crate::Metadata::default(),
        &cfg,
    )
    .unwrap();
    let limited = crate::encode(
        &pixels,
        info.row_bytes(),
        &info,
        &crate::Metadata::default(),
        &budgeted,
    )
    .unwrap();
    assert_eq!(original, limited);
    let cfg = LossyConfig {
        column_groups: Some(8),
        stripe_block_rows: 3,
        ..LossyConfig::new(55.0)
    };
    let estimate = cfg.estimated_decoder_memory(&info).unwrap();
    let budgeted = cfg
        .clone()
        .with_decoder_memory_limit(DecoderMemoryLimit::new(estimate));
    let (plan, used) = plan_lossy(&info, &budgeted).unwrap();
    assert_eq!(used, estimate);
    assert_eq!(plan.column_groups, cfg.column_groups);
    assert_eq!(plan.stripe_block_rows, cfg.stripe_block_rows);
    let original = crate::encode_lossy(
        &pixels,
        info.row_bytes(),
        &info,
        &crate::Metadata::default(),
        &cfg,
    )
    .unwrap();
    let limited = crate::encode_lossy(
        &pixels,
        info.row_bytes(),
        &info,
        &crate::Metadata::default(),
        &budgeted,
    )
    .unwrap();
    assert_eq!(original, limited);
}

#[cfg(all(feature = "lab", feature = "std"))]
#[test]
fn checked_rows_ignore_lab_reconstruction_chunk_capacity_override() {
    let _override_guard = crate::lossy::scoped_recon_chunk_override();
    let info = ImageInfo::new(1025, 17, Channels::Rgb, SampleType::U16);
    let pixels: alloc::vec::Vec<u8> = (0..1025 * 17 * 3)
        .flat_map(|i| ((i * 997) as u16).to_ne_bytes())
        .collect();
    let cfg = LossyConfig::new(55.0);
    let file = crate::encode_lossy(
        &pixels,
        info.row_bytes(),
        &info,
        &crate::Metadata::default(),
        &cfg,
    )
    .unwrap();
    let expected = crate::decode(&file).unwrap().1;
    let (_, _, body, lossy) = crate::v1::parse_header_lossy(&file).unwrap();
    crate::lossy::set_recon_chunk(usize::MAX);
    let color = crate::v1::header(&file).unwrap().lossy_color.unwrap();
    let mut checked = crate::lossy::LossyStream::new(
        info.clone(),
        crate::parse::header::HEADER_VERSION,
        color,
        lossy,
        body,
    )
    .unwrap();
    let mut ordinary = crate::lossy::LossyStream::new(
        info.clone(),
        crate::parse::header::HEADER_VERSION,
        color,
        lossy,
        body,
    )
    .unwrap();
    let mut row = alloc::vec![0;info.row_bytes()];
    checked.next_row_checked(&mut row, &|| Ok(())).unwrap();
    assert_eq!(row, &expected[..info.row_bytes()]);
    assert!(checked.reconstruction_capacity_bytes() as u128 <= crate::lossy::decoder_recon_bytes());
    ordinary.next_row(&mut row).unwrap();
    assert!(ordinary.reconstruction_capacity_bytes() > checked.reconstruction_capacity_bytes());
    let mut session = crate::decode_resources::DecodeSession::new(
        crate::decode_resources::DecodePolicy::default(),
    );
    let mut out = alloc::vec::Vec::new();
    session
        .decode_rows(&file, &mut row, |_, _, r| {
            out.extend_from_slice(r);
            Ok(())
        })
        .unwrap();
    assert_eq!(out, expected);
}
