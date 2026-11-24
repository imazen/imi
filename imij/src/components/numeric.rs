//! Shared numeric-head admission for forward and indexed component consumers.
use super::ComponentHeader;
use crate::decode_resources::feed::Budget;
use crate::parse::header::Features;
use crate::parse::limits::Admission;
use crate::parse::records::UnitMap;
use crate::{ByteReader, Error, ImageInfo, SampleType};
use alloc::{boxed::Box, vec::Vec};
use zenpixels::ChannelType;
use zenpixels::plane::PlaneEncoding;

pub(super) enum Coding {
    Lossless(Box<crate::v1::Parsed<'static>>),
    Lossy(Box<crate::lossy::LossyParsed<'static>>),
}

pub(super) struct Head {
    pub info: ImageInfo,
    pub coding: Coding,
    pub units: UnitMap,
    pub rows: usize,
    pub columns: usize,
    pub stripes: usize,
    pub groups: usize,
    pub hint: Option<Vec<u64>>,
    pub ordinals: bool,
}

impl Head {
    pub(super) fn cursor(&self) -> Cursor {
        Cursor {
            width: self.info.width as usize,
            height: self.info.height as usize,
            units: self.units.clone(),
            rows: self.rows,
            columns: self.columns,
            stripes: self.stripes,
            groups: self.groups,
            stripe: 0,
            group: 0,
            kind: 0,
            ordinal: 0,
        }
    }
    pub(super) fn parse(
        header: &ComponentHeader<'_>,
        role: usize,
        features: Features,
        head: &[u8],
        budget: &Budget,
    ) -> Result<Self, Error> {
        let (width, height) = header
            .layout
            .plane_size(role, header.width, header.height)
            .ok_or(Error::Corrupt("component grid"))?;
        let PlaneEncoding::Code(encoding) = header.layout.planes()[role].encoding() else {
            unreachable!()
        };
        let mut info = ImageInfo::new(
            width,
            height,
            crate::Channels::Gray,
            if encoding.storage() == ChannelType::U8 {
                SampleType::U8
            } else {
                SampleType::U16
            },
        );
        info.significant_bits[0] = encoding.code_bits();
        // This is one actual numeric plane, never the composed canvas.
        info.cicp = crate::Cicp {
            color_primaries: 2,
            transfer_characteristics: 2,
            matrix_coefficients: 0,
            full_range: true,
        };
        budget.header(&info, head.len())?;
        let mut input = ByteReader::new(head);
        let (coding, units, rows, columns, stripes, groups, hint, ordinals) =
            if let Some(color) = header.plane_lossy_color(role) {
                let mut parsed = crate::lossy::parse_lossy_head_checked(
                    info.clone(),
                    color,
                    features,
                    &mut input,
                    budget,
                )?;
                if parsed.component_requires_reference() {
                    return Err(Error::Unsupported("component inter reference"));
                }
                let geometry = parsed.region_geometry();
                let units = parsed.unit_map();
                let hint = parsed.hint.take();
                let ordinals = parsed.unit_ordinals;
                (
                    Coding::Lossy(Box::new(parsed)),
                    units,
                    geometry.rows,
                    if geometry.gcols == 0 {
                        width as usize
                    } else {
                        geometry.gcols
                    },
                    geometry.stripes,
                    geometry.groups,
                    hint,
                    ordinals,
                )
            } else {
                let mut parsed =
                    crate::v1::parse_head_checked(info.clone(), &mut input, budget, features)?;
                if parsed.inter {
                    return Err(Error::Unsupported("component inter reference"));
                }
                if role == 3 && parsed.layout.is_near() {
                    return Err(Error::Corrupt("component alpha must be exact"));
                }
                let (rows, groups, stripes) = (parsed.hs, parsed.ngroups(), parsed.stripe_count());
                let columns = if parsed.gcols == 0 {
                    width as usize
                } else {
                    parsed.gcols
                };
                let hint = parsed.hint.take();
                let ordinals = parsed.unit_ordinals;
                (
                    Coding::Lossless(Box::new(parsed)),
                    UnitMap::lossless(groups),
                    rows,
                    columns,
                    stripes,
                    groups,
                    hint,
                    ordinals,
                )
            };
        if input.remaining() != 0 {
            return Err(Error::Corrupt("trailing component numeric head"));
        }
        Ok(Self {
            info,
            coding,
            units,
            rows,
            columns,
            stripes,
            groups,
            hint,
            ordinals,
        })
    }
}

/// The same numeric record order/coverage for publication and consumption.
pub(super) struct Cursor {
    width: usize,
    height: usize,
    units: UnitMap,
    pub rows: usize,
    columns: usize,
    pub stripes: usize,
    pub groups: usize,
    pub stripe: usize,
    group: usize,
    kind: usize,
    pub ordinal: u64,
}
impl Cursor {
    pub fn count(&self) -> Result<usize, Error> {
        self.units.count(self.stripes)
    }
    pub fn expected(&self) -> Result<[u64; 5], Error> {
        if self.stripe >= self.stripes {
            return Err(Error::Corrupt("too many component records"));
        }
        let (kinds, count) = self.units.kinds_at(self.stripe, self.group);
        if self.kind >= count {
            return Err(Error::Corrupt("component record position"));
        }
        let (x, y) = (self.group * self.columns, self.stripe * self.rows);
        Ok([
            kinds[self.kind],
            x as u64,
            y as u64,
            self.columns.min(self.width - x) as u64,
            self.rows.min(self.height - y) as u64,
        ])
    }
    pub fn advance(&mut self) -> bool {
        self.ordinal += 1;
        self.kind += 1;
        if self.kind == self.units.per_unit_at(self.stripe, self.group) {
            self.kind = 0;
            self.group += 1;
            if self.group == self.groups {
                self.group = 0;
                self.stripe += 1;
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_resources::DecodePolicy;
    use alloc::{borrow::Cow, vec};
    use zenpixels::plane::{PlanarLayout, PlaneRole, PlaneSpec};
    use zenpixels::sample::{ChromaSiting, ChromaSubsampling, SampleEncoding};

    #[test]
    fn actual_near_heads_are_color_only_and_feature_overdeclaration_keeps_alpha_exact() {
        let info = ImageInfo::new(17, 9, crate::Channels::Gray, SampleType::U16);
        let source: Vec<_> = (0..153u16)
            .map(|i| i.wrapping_mul(4517))
            .flat_map(u16::to_ne_bytes)
            .collect();
        for bound in [0, 1] {
            let cfg = crate::v1::EncodeConfig {
                max_error: [bound; 4],
                palette: false,
                value_tables: false,
                effort: 0.0,
                stripe_rows: Some(8),
                ..Default::default()
            };
            let encoded = crate::v1::encode(
                &source,
                info.row_bytes(),
                &info,
                &crate::Metadata::default(),
                &cfg,
            )
            .unwrap();
            let file = crate::v1::parse_file(
                &encoded,
                &crate::parse::limits::Unrestricted,
                crate::checksum::Verify::Now(1),
            )
            .unwrap();
            let mut reader = ByteReader::new(file.data);
            let parsed = crate::v1::parse_head_checked(
                info.clone(),
                &mut reader,
                &crate::parse::limits::Unrestricted,
                file.declared,
            )
            .unwrap();
            assert_eq!(parsed.layout.is_near(), bound != 0);
            let (_, pixels) = crate::decode(&encoded).unwrap();
            for (original, reconstructed) in source
                .as_chunks::<2>()
                .0
                .iter()
                .zip(pixels.as_chunks::<2>().0)
            {
                assert!(
                    u16::from_ne_bytes(*original).abs_diff(u16::from_ne_bytes(*reconstructed))
                        <= bound as u16
                );
            }
            let encoding =
                PlaneEncoding::Code(SampleEncoding::new(ChannelType::U16, 16, 0).unwrap());
            let layout = PlanarLayout::new(
                vec![
                    PlaneSpec::new(PlaneRole::Luma, encoding),
                    PlaneSpec::new(PlaneRole::Cb, encoding),
                    PlaneSpec::new(PlaneRole::Cr, encoding),
                    PlaneSpec::new(PlaneRole::Alpha, encoding),
                ],
                ChromaSubsampling::Yuv444,
                ChromaSiting::Unknown,
            )
            .unwrap();
            let features = file
                .declared
                .with(crate::parse::header::Feature::NearLossless);
            let header = ComponentHeader {
                width: 17,
                height: 9,
                orientation: 1,
                decode_speed: crate::DecodeSpeed::Balanced,
                numeric_features: features,
                layout,
                cicp: zenpixels::Cicp::new(2, 2, 0, true),
                alpha_mode: zenpixels::AlphaMode::Straight,
                color_authority: zenpixels::ColorAuthority::Cicp,
                icc: None::<Cow<'_, [u8]>>,
                data_offset: 0,
                decoded_bytes: 17 * 9 * 2 * 4,
                codings: [0; 4],
                checksum: None,
            };
            let budget = Budget::new(
                DecodePolicy::default(),
                None,
                zencodec::ResourceLimits::none(),
            )
            .unwrap();
            let color =
                Head::parse(&header, 0, features, &file.data[..reader.pos], &budget).unwrap();
            assert!(matches!(color.coding, Coding::Lossless(_)));
            let alpha = Head::parse(&header, 3, features, &file.data[..reader.pos], &budget);
            if bound == 0 {
                assert!(alpha.is_ok());
            } else {
                assert!(matches!(
                    alpha,
                    Err(Error::Corrupt("component alpha must be exact"))
                ));
            }
        }
    }
}
