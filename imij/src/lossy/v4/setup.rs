//! The explicit V4 lossy setup. No legacy payload is synthesized or probed.

use alloc::{boxed::Box, vec::Vec};

use super::super::*;
use crate::decode_resources::DecodeSession;
use crate::parse::limits::Admission;
use crate::v4::{LossySettings, NumericGeometry, ObjectIntegrity, TileLayout, TileOrder};

pub(super) struct Parameters {
    pub xyb_power: bool,
    pub scale: [f32; 3],
    pub dq: [[f32; 64]; 3],
    pub steps: [[f32; 64]; 3],
    pub bias: Option<Box<[[[f32; 64]; 2]; 3]>>,
    pub transform: Option<Box<[[f32; 4]; 3]>>,
    pub xfer16: Option<(Vec<f32>, f32)>,
    pub merged: Option<Box<superblock::Tables>>,
    pub small: Option<Box<[[f32; 64]; 3]>>,
}

/// Checked immutable interpretation/geometry of a V4 lossy DATA payload.
/// Definitions and entropy are separate objects. Reading setup alone does not
/// validate a file, reconstruct pixels or prove indexed definition binding.
pub struct LossySetup {
    pub(super) info: ImageInfo,
    pub(super) color: LossyColor,
    pub(super) declared: crate::Features,
    pub(super) rows: u32,
    pub(super) columns: u32,
    pub(super) layout: TileLayout,
    pub(super) integrity: ObjectIntegrity,
    pub(super) coder: LossyEntropy,
    pub(super) control: u8,
    pub(super) settings: LossySettings,
    pub(super) coverage: crate::v4::coverage::DcCoverage,
    pub(super) banks: Vec<u64>,
    pub(super) numeric: [Option<NumericGeometry>; 4],
    pub(super) exact: Option<(u32, u32)>,
    pub(super) parameters: Parameters,
    pub(crate) checks: Option<alloc::sync::Arc<crate::v4::OwnedChecks>>,
    _charge: crate::decode_resources::V4Retained,
}

fn blob<'a>(
    r: &mut ByteReader<'a>,
    spec: &'static str,
    label: &'static str,
) -> Result<&'a [u8], Error> {
    let n = crate::trace::tf!("setup section bytes", r, r.var_usize()?);
    crate::trace::trace_enter_addr("payload", spec, label, r.mark());
    let bytes = r.bytes(n)?;
    crate::trace::exit_addr(r.mark());
    Ok(bytes)
}

fn float(r: &mut ByteReader<'_>) -> Result<f32, Error> {
    Ok(f32::from_le_bytes(r.bytes(4)?.try_into().unwrap()))
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::Cicp;
    use crate::decode_resources::DecodePolicy;

    #[test]
    fn v4_srgb_power_transfer_needs_matching_output_declaration() {
        let session = DecodeSession::new(DecodePolicy::default());
        let mut quantizer = vec![xyb_power::QUANTIZER_BIT as u8];
        quantizer.extend_from_slice(&1.0f32.to_le_bytes());
        for (kind, cicp, color, accepted) in [
            (SampleType::U8, Cicp::SRGB, LossyColor::Xyb, true),
            (SampleType::U16, Cicp::SRGB, LossyColor::Xyb, true),
            (SampleType::U8, Cicp::SRGB, LossyColor::YCoCg, false),
            (SampleType::U16, Cicp::SRGB, LossyColor::YCoCg, false),
            (SampleType::F16, Cicp::SRGB, LossyColor::Xyb, false),
            (SampleType::F32, Cicp::SRGB, LossyColor::Xyb, false),
            (SampleType::U8, Cicp::BT2100_PQ, LossyColor::Xyb, false),
            (SampleType::U16, Cicp::BT2100_PQ, LossyColor::Xyb, false),
        ] {
            let mut info = ImageInfo::new(17, 19, Channels::Rgb, kind);
            info.cicp = cicp;
            let actual = parameters(
                &info,
                color,
                &quantizer,
                &[],
                crate::Features::EMPTY,
                &session,
            );
            if accepted {
                let p = actual.unwrap();
                assert!(p.xyb_power);
                assert!(p.xfer16.is_none());
            } else {
                assert!(matches!(actual, Err(Error::Corrupt(_))));
            }
        }
        // Custom precision must also be rejected for bit 5.
        let custom = ImageInfo::new(17, 19, Channels::Rgb, SampleType::U16)
            .with_significant_bits(&[12, 12, 12])
            .unwrap();
        let actual = parameters(
            &custom,
            LossyColor::Xyb,
            &quantizer,
            &[],
            crate::Features::EMPTY,
            &session,
        );
        assert!(matches!(actual, Err(Error::Corrupt(_))));
    }

    #[test]
    fn quantizer_defaults_and_explicit_records_retain_v3_steps_and_gates() {
        let session = DecodeSession::new(DecodePolicy::default());
        for (color, sample) in [
            (LossyColor::YCoCg, SampleType::U8),
            (LossyColor::Xyb, SampleType::U8),
            (LossyColor::YCoCg, SampleType::U16),
            (LossyColor::Xyb, SampleType::U16),
            (LossyColor::Xyb, SampleType::F16),
            (LossyColor::Xyb, SampleType::F32),
            (LossyColor::YCoCg, SampleType::U32),
        ] {
            let info = ImageInfo::new(17, 19, Channels::Rgb, sample);
            for matrix in 0..=5 {
                let mut bytes = Vec::new();
                crate::put_varint(
                    &mut bytes,
                    u64::from(matrix != envelope::default_matrix(color)),
                );
                bytes.extend_from_slice(&1.25f32.to_le_bytes());
                if matrix != envelope::default_matrix(color) {
                    crate::put_varint(&mut bytes, u64::from(matrix));
                }
                let declared = crate::Features::EMPTY.with(crate::Feature::DequantPresets);
                let actual = parameters(&info, color, &bytes, &[], declared, &session).unwrap();
                let mut expected = QParams::from_quality(90.0, color, matrix);
                expected.scale = 1.25;
                assert_eq!(
                    actual.steps.map(|p| p.map(f32::to_bits)),
                    expected.steps().map(|p| p.map(f32::to_bits))
                );
                assert_eq!(
                    actual.scale,
                    envelope::defaults(color, InputKind::of(&info).unwrap())
                );
                if matrix >= 3 {
                    assert!(
                        parameters(&info, color, &bytes, &[], crate::Features::EMPTY, &session)
                            .is_err()
                    );
                }
            }
        }
        let info = ImageInfo::new(17, 19, Channels::Rgb, SampleType::U8);
        let mut quantizer = Vec::new();
        crate::put_varint(&mut quantizer, 1);
        quantizer.extend_from_slice(&1.25f32.to_le_bytes());
        crate::put_varint(&mut quantizer, 63);
        let tables = core::array::from_fn(|c| core::array::from_fn(|i| 1.0 + (c * 7 + i) as f32));
        let mut table_bytes = Vec::new();
        envelope::write_explicit(&mut table_bytes, &tables);
        let mut records = Vec::new();
        crate::parse::records::put_record(&mut records, H_EXPLICIT, &table_bytes);
        let mut transform = Vec::new();
        envelope::write_transform(&mut transform, ColorTransform::Bt709);
        crate::parse::records::put_record(&mut records, H_TRANSFORM, &transform);
        let declared = crate::Features::EMPTY
            .with(crate::Feature::ExplicitTables)
            .with(crate::Feature::ColorTransform);
        let actual = parameters(
            &info,
            LossyColor::YCoCg,
            &quantizer,
            &records,
            declared,
            &session,
        )
        .unwrap();
        let mut expected = QParams::from_quality(90.0, LossyColor::YCoCg, 63);
        expected.scale = 1.25;
        expected.explicit = Some(tables);
        assert_eq!(
            actual.steps.map(|p| p.map(f32::to_bits)),
            expected.steps().map(|p| p.map(f32::to_bits))
        );
        assert_eq!(actual.transform.as_deref(), Some(&color::TRANSFORM_BT709));
        assert!(
            parameters(
                &info,
                LossyColor::YCoCg,
                &quantizer,
                &[],
                declared,
                &session
            )
            .is_err()
        );
        assert!(
            parameters(
                &info,
                LossyColor::Xyb,
                &quantizer,
                &records,
                declared,
                &session
            )
            .is_err()
        );
        let mut ordinary = Vec::new();
        ordinary.push(0);
        ordinary.extend_from_slice(&1f32.to_le_bytes());
        assert!(matches!(
            parameters(
                &info,
                LossyColor::YCoCg,
                &ordinary,
                &records,
                declared,
                &session
            ),
            Err(Error::Corrupt("explicit tables without matrix 63"))
        ));
        for value in [f32::NAN, f32::INFINITY, -1.0, 65536.0] {
            let mut bytes = Vec::new();
            bytes.push(0);
            bytes.extend_from_slice(&value.to_le_bytes());
            assert!(
                parameters(
                    &info,
                    LossyColor::YCoCg,
                    &bytes,
                    &[],
                    crate::Features::EMPTY,
                    &session
                )
                .is_err()
            );
        }
    }
}

fn parameters<C: Fn() -> bool + Sync>(
    info: &ImageInfo,
    color: LossyColor,
    quantizer: &[u8],
    records: &[u8],
    declared: crate::Features,
    session: &DecodeSession<C>,
) -> Result<Parameters, Error> {
    use crate::Feature;
    let kind = InputKind::of(info).ok_or(Error::Corrupt("V4 lossy sample format"))?;
    if matches!(kind, InputKind::U32 | InputKind::F32)
        && !cfg!(feature = "extended")
        && session.v4_decodes()
    {
        return Err(Error::Unsupported("32-bit samples (extended tier)"));
    }
    let transfer = transfer::Transfer::from_cicp(info.cicp.transfer_characteristics);
    let valid_color = match kind {
        InputKind::U8 => true,
        InputKind::U16 | InputKind::U32 => color == LossyColor::YCoCg || transfer.is_some(),
        InputKind::F16 | InputKind::F32 => color == LossyColor::Xyb,
    };
    if !valid_color
        || ((info.has_custom_precision() || kind == InputKind::U32)
            && (!info.cicp.full_range || info.cicp.matrix_coefficients != 0))
        || (precision::custom_color(info) && color == LossyColor::Xyb && transfer.is_none())
    {
        return Err(Error::Corrupt(
            "lossy color does not match the sample type or range",
        ));
    }
    let mut r = ByteReader::new(quantizer);
    let present = r.varint()?;
    if present & !(31 | xyb_power::QUANTIZER_BIT) != 0 {
        return Err(Error::Unsupported("V4 quantizer field"));
    }
    let xyb_power = present & xyb_power::QUANTIZER_BIT != 0;
    if xyb_power && !xyb_power::supports(info, color) {
        return Err(Error::Corrupt(
            "V4 power transfer requires full-precision sRGB U8 or U16 XYB",
        ));
    }
    let mut qp = QParams::from_quality(90.0, color, envelope::default_matrix(color));
    qp.scale = float(&mut r)?;
    qp.floor = 1.0;
    qp.dc = default_dc_step(color);
    if present & 1 != 0 {
        qp.matrix =
            u8::try_from(r.varint()?).map_err(|_| Error::Unsupported("quantization matrix"))?;
    }
    let gate = match qp.matrix {
        0..=2 => None,
        m if envelope::MATRIX_PRESETS.contains(&m) => Some(Feature::DequantPresets),
        envelope::MATRIX_EXPLICIT => Some(Feature::ExplicitTables),
        _ => return Err(Error::Unsupported("unknown quantization matrix")),
    };
    if gate.is_some_and(|f| !declared.contains(f)) {
        return Err(Error::Corrupt("quantization matrix without its feature"));
    }
    // Channel-multiplier defaults depend on the resolved matrix, as in V3.
    qp.mul = QParams::from_quality(90.0, color, qp.matrix).mul;
    if present & 2 != 0 {
        for d in &mut qp.dc {
            *d = u8::try_from(r.varint()?)
                .ok()
                .filter(|&v| v != 0)
                .ok_or(Error::Corrupt("V4 DC step multiplier"))?;
        }
    }
    if present & 4 != 0 {
        qp.floor = float(&mut r)?;
    }
    let mut scale = envelope::defaults(color, kind);
    if present & 8 != 0 {
        for s in &mut scale {
            *s = float(&mut r)?;
        }
    }
    if present & 16 != 0 {
        for m in &mut qp.mul {
            *m = float(&mut r)?;
        }
    }
    if r.remaining() != 0 {
        return Err(Error::Corrupt("V4 quantizer trailing bytes"));
    }
    crate::parse::records::head_records(records, |id, bytes| {
        session.adapter_work(bytes.len() as u128 + 1)?;
        let mut r = ByteReader::new(bytes);
        let feature = match id {
            H_EXPLICIT => Some(crate::Feature::ExplicitTables),
            H_BIAS => Some(crate::Feature::ReconBias),
            H_TRANSFORM => Some(crate::Feature::ColorTransform),
            // These meanings were replaced by mandatory V4 setup fields.
            1 | 3 | 9 | 10 | 11 | 12 | 21 | 23 | 27 | 37 | 39 | 41 | 43 | 45 => {
                return Err(Error::Corrupt("replaced V3 record in V4 lossy setup"));
            }
            _ if id & 1 != 0 => return Err(Error::Unsupported("unknown V4 lossy setup record")),
            _ => None,
        };
        if feature.is_some_and(|f| !declared.contains(f)) {
            return Err(Error::Corrupt("lossy setup record without its feature"));
        }
        match id {
            H_EXPLICIT => {
                if qp.matrix != envelope::MATRIX_EXPLICIT {
                    return Err(Error::Corrupt("explicit tables without matrix 63"));
                }
                qp.explicit = Some(envelope::read_explicit(&mut r)?);
            }
            H_BIAS => qp.bias = Some(envelope::read_bias(&mut r)?),
            H_TRANSFORM => qp.transform = Some(envelope::read_transform(&mut r)?),
            _ => return Ok(()),
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("V4 lossy setup record trailing bytes"));
        }
        Ok(())
    })?;
    if qp.matrix == envelope::MATRIX_EXPLICIT && qp.explicit.is_none() {
        return Err(Error::Corrupt("explicit tables without their record"));
    }
    if qp.transform.is_some()
        && (color != LossyColor::YCoCg || kind != InputKind::U8 || info.has_custom_precision())
    {
        return Err(Error::Corrupt("color transform needs YCoCg U8 samples"));
    }
    if !scale.iter().all(|s| s.is_finite() && *s > 1e-6 && *s < 1e9) || !qp.valid() {
        return Err(Error::Corrupt("bad V4 quantization parameters"));
    }
    let transfer_bytes = if xyb_power {
        0
    } else {
        decoder_transfer_table_bytes(info, color)
    };
    session
        .v4_models()
        .hint(usize::try_from(transfer_bytes + 64 * 1024).map_err(|_| Error::TooLarge)?)?;
    let xfer16 = if xyb_power {
        None
    } else {
        match (kind, color, transfer) {
            (InputKind::U16 | InputKind::U32, LossyColor::Xyb, Some(t)) => {
                Some((t.table(), t.linear_max_f32()))
            }
            (InputKind::U8, LossyColor::Xyb, Some(t)) if precision::custom_color(info) => {
                Some((t.table(), t.linear_max_f32()))
            }
            _ => None,
        }
    };
    let steps = qp.steps();
    Ok(Parameters {
        xyb_power,
        scale,
        dq: steps.map(|s| dct::dequant_factors(&s)),
        steps,
        bias: qp
            .bias
            .filter(|b| !b.is_zero())
            .map(|b| Box::new(b.tables())),
        transform: qp.transform.and_then(|t| t.decode()).map(Box::new),
        xfer16,
        merged: declared
            .contains(Feature::Superblocks)
            .then(|| Box::new(superblock::Tables::new(&steps))),
        small: declared
            .contains(Feature::SmallTransforms)
            .then(|| Box::new(steps.map(|s| small::dequant4(&s)))),
    })
}

impl LossySetup {
    /// Read the immutable setup body (excluding its outer length and directory
    /// placement). Framed input belongs to the owning file/feed driver.
    pub fn read<C: Fn() -> bool + Sync>(
        info: ImageInfo,
        color: LossyColor,
        declared: crate::Features,
        body: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        let scope = session.v4_model_scope();
        session.v4_setup_admitted(body.len())?;
        info.validate_precision()?;
        if info.width == 0 || info.height == 0 {
            return Err(Error::Corrupt("V4 lossy canvas"));
        }
        let mut r = ByteReader::new(body);
        let rows = crate::trace::tf!("group block rows", r, r.var_u32()?);
        let columns = crate::trace::tf!("group columns", r, r.var_u32()?);
        if rows == 0 || (columns != 0 && (columns % 8 != 0 || columns >= info.width)) {
            return Err(Error::Corrupt("V4 lossy group geometry"));
        }
        let groups = if columns == 0 {
            1
        } else {
            info.width.div_ceil(columns)
        };
        let stripes = info.height.div_ceil(8).div_ceil(rows);
        let tile_columns = crate::trace::tf!("tile columns", r, r.var_u32()?);
        let tile_rows = crate::trace::tf!("tile rows", r, r.var_u32()?);
        let order = TileOrder::try_from(crate::trace::tf!("tile order", r, r.varint()?))?;
        let layout = TileLayout::new(groups, stripes, tile_columns, tile_rows, order)?;
        let integrity =
            ObjectIntegrity::try_from(crate::trace::tf!("object integrity", r, r.varint()?))?;
        let coder = match crate::trace::tf!("AC coder", r, r.varint()?) {
            0 => LossyEntropy::RunLevel,
            3 => LossyEntropy::SplitHuf,
            _ => return Err(Error::Unsupported("V4 AC coder")),
        };
        let quantizer = blob(&mut r, "V4:2.2", "quantizer parameters")?;
        let control = u8::try_from(crate::trace::tf!("block control", r, r.varint()?))
            .ok()
            .filter(|&v| v <= 4)
            .ok_or(Error::Unsupported("V4 block control"))?;
        if matches!(control, 3 | 4) && info.channels != Channels::Rgb {
            return Err(Error::Corrupt("keep blocks outside RGB"));
        }
        if control == 2
            && (!matches!(info.channels, Channels::Rgb | Channels::Rgba)
                || info.sample != SampleType::U8
                || !declared.contains(crate::Feature::ExactBlocks))
        {
            return Err(Error::Corrupt("V4 exact blocks need declared RGB(A)8"));
        }
        let storage = crate::trace::tf!("DC storage", r, r.varint()?);
        if storage > 1 {
            return Err(Error::Unsupported("V4 DC storage"));
        }
        if storage == 1 && !declared.contains(crate::Feature::Samples32) {
            return Err(Error::Corrupt("I32 DC without sample capability"));
        }
        if storage == 1 && !cfg!(feature = "extended") && session.v4_decodes() {
            return Err(Error::Unsupported("32-bit samples (extended tier)"));
        }
        let settings = LossySettings::read(
            blob(&mut r, "V4:3", "image defaults and reusable settings")?,
            session,
        )?;
        let coverage = crate::v4::coverage::DcCoverage::read(
            blob(&mut r, "V4:5", "DC coverage")?,
            stripes,
            groups,
            tile_rows,
            session,
        )?;
        let count = crate::trace::tf!("declared seed banks", r, r.var_usize()?);
        if count > r.remaining() {
            return Err(Error::Corrupt("V4 declared bank count"));
        }
        session
            .v4_models()
            .hint(count.checked_mul(8).ok_or(Error::TooLarge)?)?;
        let mut banks = Vec::new();
        banks
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        let mut previous = 0;
        for _ in 0..count {
            session.adapter_work(1)?;
            let id = crate::trace::tf!("declared bank ID", r, r.varint()?);
            if id <= previous {
                return Err(Error::Corrupt("V4 declared bank IDs"));
            }
            if crate::seeds::bank(id).is_none() {
                return Err(Error::Unsupported("unknown V4 declared bank"));
            }
            previous = id;
            banks.push(id);
        }
        let expected = 1 + usize::from(info.channels.has_alpha()) + usize::from(control == 2);
        if crate::trace::tf!("numeric plane descriptors", r, r.var_usize()?) != expected {
            return Err(Error::Corrupt("V4 numeric plane descriptors"));
        }
        let mut numeric = [None, None, None, None];
        let mut exact = None;
        for role in [0, 2, 3] {
            if role == 2 && !info.channels.has_alpha() || role == 3 && control != 2 {
                continue;
            }
            if crate::trace::tf!("numeric plane role", r, r.varint()?) != role {
                return Err(Error::Corrupt("V4 numeric plane role order"));
            }
            let lanes = u8::try_from(crate::trace::tf!("numeric plane lanes", r, r.varint()?))
                .ok()
                .filter(|&v| (1..=16).contains(&v))
                .ok_or(Error::Corrupt("V4 numeric plane lanes"))?;
            let (plane, hs, cols) = match role {
                0 => {
                    let height = info.height.div_ceil(8);
                    let hs = (u64::from(coverage.max_span(stripes, tile_rows)) * u64::from(rows))
                        .min(u64::from(height)) as u32;
                    (
                        ImageInfo::new(
                            info.width.div_ceil(8),
                            height,
                            if control == 0 {
                                Channels::Rgb
                            } else {
                                Channels::Rgba
                            },
                            if storage == 0 {
                                SampleType::U16
                            } else {
                                SampleType::I32
                            },
                        ),
                        hs,
                        columns / 8,
                    )
                }
                2 => {
                    let plane =
                        ImageInfo::new(info.width, info.height, Channels::Gray, info.sample)
                            .with_significant_bits(&[info
                                .channel_bits(info.channels.count() - 1)
                                .unwrap()])?;
                    (
                        plane,
                        (u64::from(rows) * 8).min(u64::from(info.height)) as u32,
                        columns,
                    )
                }
                _ => {
                    let blocks = r.var_u32()?;
                    let exact_rows = r.var_u32()?;
                    if blocks == 0 {
                        return Err(Error::Corrupt("zero V4 exact packed width"));
                    }
                    let width = blocks
                        .checked_mul(8)
                        .filter(|&v| v != 0)
                        .ok_or(Error::TooLarge)?;
                    if exact_rows == 0 || exact_rows % 8 != 0 {
                        return Err(Error::Corrupt("V4 exact stripe geometry"));
                    }
                    // Admission uses the largest possible tile packing. Actual
                    // counts/height belong to each tile's begin declaration.
                    let bw = if columns == 0 {
                        info.width.div_ceil(8)
                    } else {
                        columns / 8
                    };
                    let tile_bw = (u64::from(tile_columns.min(groups)) * u64::from(bw))
                        .min(u64::from(info.width.div_ceil(8)));
                    let parents = tile_bw
                        * (u64::from(tile_rows.min(stripes)) * u64::from(rows))
                            .min(u64::from(info.height.div_ceil(8)));
                    let height = u32::try_from(
                        parents
                            .div_ceil(u64::from(blocks))
                            .checked_mul(8)
                            .ok_or(Error::TooLarge)?,
                    )
                    .map_err(|_| Error::TooLarge)?;
                    exact = Some((blocks, exact_rows));
                    (
                        ImageInfo::new(width, height, Channels::Rgb, SampleType::U8),
                        exact_rows.min(height),
                        0,
                    )
                }
            };
            numeric[role as usize] = Some(NumericGeometry::new(plane, lanes, hs, cols)?);
        }
        let records = blob(&mut r, "V4:2.2", "remaining parameter records")?;
        let remaining = r.remaining();
        let checks = crate::v4::OwnedChecks::read_tail(r.bytes(remaining)?, session)?;
        let parameters = parameters(&info, color, quantizer, records, declared, session)?;
        if let Some(profile) = session.v4_profile() {
            profile.check_image(&info, crate::Coding::Lossy)?;
            profile.check_lossy_control(control, storage)?;
        }
        Ok(Self {
            info,
            color,
            declared,
            rows,
            columns,
            layout,
            integrity,
            coder,
            control,
            settings,
            coverage,
            banks,
            numeric,
            exact,
            parameters,
            checks,
            _charge: scope.commit()?,
        })
    }

    /// Declared scopes and suites; unknown suites remain unsupported.
    pub fn checks(&self) -> crate::v4::Checks<'_> {
        self.checks
            .as_ref()
            .map_or_else(crate::v4::Checks::default, |c| c.get())
    }

    pub fn info(&self) -> &ImageInfo {
        &self.info
    }
    pub const fn layout(&self) -> TileLayout {
        self.layout
    }
    pub const fn integrity(&self) -> ObjectIntegrity {
        self.integrity
    }
    pub const fn coder(&self) -> LossyEntropy {
        self.coder
    }
    pub fn settings(&self) -> &LossySettings {
        &self.settings
    }
    pub fn numeric_geometry(&self, role: u64) -> Option<&NumericGeometry> {
        self.numeric.get(usize::try_from(role).ok()?)?.as_ref()
    }

    pub fn group_rect(&self, id: u64) -> Result<crate::planes::Rect, Error> {
        let (groups, stripes) = self.layout.group_grid();
        if id >= u64::from(groups) * u64::from(stripes) {
            return Err(Error::Corrupt("V4 lossy group identity"));
        }
        let x = (id % u64::from(groups)) * u64::from(self.columns);
        let y = (id / u64::from(groups)) * u64::from(self.rows) * 8;
        Ok(crate::planes::Rect::new(
            x as u32,
            y as u32,
            if self.columns == 0 {
                self.info.width
            } else {
                self.columns.min(self.info.width - x as u32)
            },
            (u64::from(self.rows) * 8).min(u64::from(self.info.height) - y) as u32,
        ))
    }

    /// DC plane rectangle and its plane-local identity for the requested AC group.
    pub fn dc_for_group(&self, id: u64) -> Result<(u64, crate::planes::Rect), Error> {
        let rect = self.group_rect(id)?;
        let (groups, stripes) = self.layout.group_grid();
        let span = self.coverage.span(
            (id / u64::from(groups)) as u32,
            (id % u64::from(groups)) as u32,
            stripes,
            groups,
            self.layout.tile_shape().1,
        )?;
        let y = u64::from(span.start) * u64::from(self.rows);
        let end = (u64::from(span.end) * u64::from(self.rows))
            .min(u64::from(self.info.height.div_ceil(8)));
        Ok((
            span.id,
            crate::planes::Rect::new(
                rect.x() / 8,
                y as u32,
                rect.width().div_ceil(8),
                (end - y) as u32,
            ),
        ))
    }
}

#[cfg(all(feature = "lab", feature = "std"))]
impl LossySetup {
    // Compact semantic runs come from the production resolver. They are display
    // annotations, not invented byte fields or a second settings interpretation.
    pub(crate) fn trace_settings<C: Fn() -> bool + Sync>(
        &self,
        id: u64,
        bytes: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        use core::fmt::Write;
        let rect = self.group_rect(id)?;
        let cols = rect.width().div_ceil(8);
        let rows = rect.height().div_ceil(8);
        let mut out = alloc::format!(
            "{{\"columns\":{},\"rows\":{},\"runs\":[",
            cols.div_ceil(2),
            rows.div_ceil(2)
        );
        let mut last = None;
        let mut count = 0u64;
        let mut first = true;
        let flush = |out: &mut alloc::string::String,
                     count: u64,
                     state: crate::v4::CellSettings,
                     first: &mut bool|
         -> Result<(), Error> {
            out.try_reserve(128).map_err(|_| Error::TooLarge)?;
            session.adapter_work(128)?;
            write!(
                out,
                "{}[{}, {}, {:?}, {:?}]",
                if *first { "" } else { "," },
                count,
                state.partition(),
                state.plane_plans(),
                state.ac_codes()
            )
            .map_err(|_| Error::TooLarge)?;
            *first = false;
            Ok(())
        };
        self.settings
            .visit_unit(bytes, rows, cols, self.declared, session, |_, _, state| {
                if let Some(previous) = last
                    && previous != state
                {
                    flush(&mut out, count, previous, &mut first)?;
                    count = 0;
                }
                last = Some(state);
                count += 1;
                Ok(())
            })?;
        if let Some(state) = last {
            flush(&mut out, count, state, &mut first)?;
        }
        out.push_str("]}");
        crate::trace::tnote!("V4 color", alloc::format!("{:?}", self.color));
        crate::trace::tnote!("V4 effective cells", out);
        Ok(())
    }
}
