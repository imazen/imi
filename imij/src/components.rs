//! Review-only native component interpretation. No identifiers are allocated.
//!
//! Inspection admits the critical declaration and metadata before DATA. With
//! `std`, a candidate forward reader decodes native component rows through the
//! shared numeric engines and validates its tail. Neither path turns the canvas
//! into an ordinary packed [`crate::Header`]/[`crate::ImageInfo`]. Existing packed
//! APIs keep refusing this candidate before rows. All syntax remains unallocated.
use alloc::{borrow::Cow, vec::Vec};

use crate::decode_resources::DecodePolicy;
use crate::parse::chunks::{self, Kind, Rules};
use crate::parse::header::{self, Coding, DecodeSpeed, Feature, Features};
use crate::{ByteReader, Error, LossyColor};
use zenpixels::plane::{PlanarLayout, PlaneEncoding, PlaneRole, PlaneSpec};
use zenpixels::sample::{ChromaSiting, ChromaSubsampling, SampleEncoding};
use zenpixels::{AlphaMode, ChannelType, Cicp, ColorAuthority};

#[cfg(feature = "std")]
mod decode;
#[cfg(feature = "std")]
mod encode;
#[cfg(feature = "std")]
mod feed;
#[cfg(feature = "std")]
mod numeric;
#[cfg(feature = "std")]
pub use decode::{ComponentEvent, decode_reader};
#[cfg(feature = "std")]
pub use encode::{ComponentEncodeConfig, ComponentEncodeLimits, ComponentSource, encode_groups};
#[cfg(feature = "std")]
pub use feed::{ComponentFeedDecoder, ComponentFeedStatus};

// Sentinels shared by the private wire witnesses, not registry allocations.
const COMPONENT_FEATURE: u64 = 1 << 63;
const COMPONENT_DESCRIPTOR: u64 = u64::MAX - 2;
const ALL_LOSSLESS: u8 = 254;
const POTENTIALLY_LOSSY: u8 = 255;

/// Declared native component interpretation, independent of packed pixels.
///
/// ICC is opaque metadata: this inspector neither validates an ICC profile nor
/// applies a color transform. Chroma matrix/range/siting stay explicit even
/// when ICC is authoritative for subsequent CMS operations. The declaration
/// remains provisional until a complete native component decoder validates it.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ComponentHeader<'a> {
    pub width: u32,
    pub height: u32,
    pub orientation: u8,
    pub decode_speed: DecodeSpeed,
    /// Assigned numeric capabilities, excluding the unallocated component-mode
    /// sentinel; lossy coding is included when implied by this declaration.
    pub numeric_features: Features,
    /// Shared roles, sampling, siting and per-plane integer code interpretation.
    pub layout: PlanarLayout,
    pub cicp: Cicp,
    pub alpha_mode: AlphaMode,
    /// Active authority, separate from historical `cORg` metadata.
    pub color_authority: ColorAuthority,
    /// A raw `iCCP` is borrowed; a packed `mCMP` ICC is expanded under policy.
    pub icc: Option<Cow<'a, [u8]>>,
    /// Byte position immediately after the DATA chunk framing, before its head.
    pub data_offset: usize,
    /// Decoded bytes across the visible component grids, excluding padding.
    pub decoded_bytes: u64,
    codings: [u8; 4],
    #[cfg(feature = "std")]
    checksum: Option<crate::checksum::Announced>,
}

impl ComponentHeader<'_> {
    /// Whether any plane selects lossy transform coding. The existing lossless
    /// numeric grammar can carry near-lossless color bounds; this classification
    /// alone is not a source-exactness guarantee. Alpha bounds are checked at
    /// numeric-head admission and must remain zero.
    pub fn is_lossy(&self) -> bool {
        self.codings[..self.layout.planes().len()]
            .iter()
            .any(|&c| c != 0)
    }

    /// Numeric plane coding, or `None` for an index outside this layout.
    pub fn plane_coding(&self, index: usize) -> Option<Coding> {
        (index < self.layout.planes().len()).then(|| {
            if self.codings[index] == 0 {
                Coding::Lossless
            } else {
                Coding::Lossy
            }
        })
    }

    /// Lossy plane's existing numeric color convention; `None` for lossless
    /// planes or an index outside the layout. This is not composed RGB output.
    pub fn plane_lossy_color(&self, index: usize) -> Option<LossyColor> {
        if index >= self.layout.planes().len() {
            return None;
        }
        match self.codings[index] {
            1 => Some(LossyColor::Xyb),
            2 => Some(LossyColor::YCoCg),
            _ => None,
        }
    }
}

/// Inspect a component declaration and admit its complete pre-DATA metadata.
///
/// This review-only API recognizes unallocated sentinel syntax. It does not
/// accept ordinary packed files or promise DATA decoding/final validation.
/// Limits apply to the supplied bytes, fixed header, pre-DATA metadata/chunks,
/// dynamic declaration capacity, canvas pixels and total visible plane bytes.
/// No raster is allocated. `iCCP` must precede DATA when declared authoritative;
/// late metadata cannot retroactively change this early interpretation.
pub fn inspect<'a>(data: &'a [u8], policy: DecodePolicy) -> Result<ComponentHeader<'a>, Error> {
    inspect_impl(data, policy, true)
}

#[cfg(feature = "std")]
fn inspect_declaration(data: &[u8], policy: DecodePolicy) -> Result<ComponentHeader<'_>, Error> {
    inspect_impl(data, policy, false)
}

fn inspect_impl(
    data: &[u8],
    policy: DecodePolicy,
    scan_metadata: bool,
) -> Result<ComponentHeader<'_>, Error> {
    let (output, max_input, max_header, max_metadata, max_chunks, max_working) =
        policy.component_probe_limits();
    if data.len() as u64 > max_input {
        return Err(Error::TooLarge);
    }
    let mut input = ByteReader::new(data);
    let prefix = header::read_header_prefix(&mut input, max_header)?;
    if prefix.version != header::HEADER_VERSION
        || prefix.high
        || prefix.low & COMPONENT_FEATURE == 0
    {
        return Err(Error::Unsupported("not a native component candidate"));
    }
    let known = Features::from_bits(prefix.low & !COMPONENT_FEATURE)
        .ok_or(Error::Unsupported("unknown component numeric feature"))?;
    if known.contains(Feature::NativeComponents) {
        return Err(Error::Unsupported("native components retired in v3"));
    }
    let allowed = header::F_CODING | header::F_FEATURES | header::F_ORIENT | header::F_SPEED;
    if prefix.present & !allowed != 0 {
        return Err(Error::Corrupt("packed fields forbidden in component mode"));
    }
    if prefix.present & (header::F_CODING | header::F_FEATURES)
        != (header::F_CODING | header::F_FEATURES)
    {
        return Err(Error::Corrupt("component feature and coding required"));
    }
    let mut head = prefix.body;
    let mode = head.u8()?;
    if !matches!(mode, ALL_LOSSLESS | POTENTIALLY_LOSSY) {
        return Err(Error::Unsupported("unknown component coding mode"));
    }
    let numeric_features = if mode == POTENTIALLY_LOSSY {
        known.with(Feature::Lossy)
    } else {
        known
    };
    let orientation = if prefix.present & header::F_ORIENT != 0 {
        head.u8()?
    } else {
        1
    };
    if !(1..=8).contains(&orientation) {
        return Err(Error::Corrupt("bad component orientation"));
    }
    let decode_speed = if prefix.present & header::F_SPEED != 0 {
        DecodeSpeed::from_u8(head.u8()?).unwrap_or(DecodeSpeed::Fastest)
    } else {
        DecodeSpeed::Balanced
    };
    let (width, height) = (head.varint()?, head.varint()?);
    if width == 0 || height == 0 || width > u32::MAX as u64 || height > u32::MAX as u64 {
        return Err(Error::Corrupt("bad component dimensions"));
    }
    if width * height > output.max_pixels {
        return Err(Error::TooLarge);
    }
    let (width, height) = (width as u32, height as u32);
    let mut last = None;
    let mut descriptor = None;
    #[cfg(feature = "std")]
    let mut checksum = None;
    while head.remaining() != 0 {
        let (id, len) = (head.varint()?, head.varint()?);
        if last.is_some_and(|old| id <= old) {
            return Err(Error::Corrupt("component header records out of order"));
        }
        last = Some(id);
        let payload = head.bytes_len(len)?;
        match id {
            COMPONENT_DESCRIPTOR => descriptor = Some(payload),
            crate::checksum::EXT_CHECKSUM => {
                let announced = crate::checksum::Announced::parse(payload, prefix.version);
                announced.log2()?;
                #[cfg(feature = "std")]
                {
                    checksum = Some(announced);
                }
            }
            header::EXT_LOCATOR => {
                return Err(Error::Unsupported("component DATA locator not defined"));
            }
            _ if id & 1 != 0 => {
                return Err(Error::Unsupported(
                    "unknown critical component header record",
                ));
            }
            _ => {}
        }
    }
    let mut descriptor =
        ByteReader::new(descriptor.ok_or(Error::Corrupt("missing component descriptor"))?);
    let (version, count, sampling, siting) = (
        descriptor.u8()?,
        descriptor.u8()?,
        descriptor.u8()?,
        descriptor.u8()?,
    );
    if !matches!(version, 1 | 2) || !(3..=4).contains(&count) || (version == 1 && count != 3) {
        return Err(Error::Unsupported("component descriptor version/count"));
    }
    let (authority, alpha) = if version == 2 {
        (descriptor.u8()?, descriptor.u8()?)
    } else {
        (0, 0)
    };
    let color_authority = match authority {
        0 => ColorAuthority::Cicp,
        1 => ColorAuthority::Icc,
        _ => return Err(Error::Unsupported("component color authority")),
    };
    let alpha_mode = match alpha {
        0 => AlphaMode::Straight,
        1 => AlphaMode::Premultiplied,
        2 => AlphaMode::Opaque,
        3 => AlphaMode::Undefined,
        _ => return Err(Error::Unsupported("component alpha interpretation")),
    };
    if count == 3 && alpha != 0 {
        return Err(Error::Corrupt("alpha interpretation without alpha plane"));
    }
    let sampling = match sampling {
        0 => ChromaSubsampling::Yuv444,
        1 => ChromaSubsampling::Yuv422,
        2 => ChromaSubsampling::Yuv420,
        _ => return Err(Error::Unsupported("component sampling")),
    };
    let siting = match siting {
        0 => ChromaSiting::Unknown,
        1 => ChromaSiting::Center,
        2 => ChromaSiting::Left,
        3 => ChromaSiting::TopLeft,
        _ => return Err(Error::Unsupported("component siting")),
    };
    let color = descriptor.bytes(4)?;
    if color[3] > 1 {
        return Err(Error::Corrupt("component range bits"));
    }
    let cicp = Cicp::new(color[0], color[1], color[2], color[3] != 0);
    let declaration_capacity = u64::from(count) * core::mem::size_of::<PlaneSpec>() as u64;
    if declaration_capacity > max_working {
        return Err(Error::TooLarge);
    }
    let mut planes = Vec::new();
    planes
        .try_reserve_exact(count as usize)
        .map_err(|_| Error::TooLarge)?;
    let mut grids = [(0u64, 0u64); 4];
    let mut codings = [0; 4];
    let mut decoded_bytes = 0u64;
    for index in 0..count as usize {
        if descriptor.u8()? != index as u8 {
            return Err(Error::Corrupt("component role/order"));
        }
        let (pw, ph) = (descriptor.varint()?, descriptor.varint()?);
        let (storage, bits, coding) = (descriptor.u8()?, descriptor.u8()?, descriptor.u8()?);
        if bits < 8 || coding > 2 {
            return Err(Error::Unsupported("component precision/coding"));
        }
        let storage = match storage {
            8 => ChannelType::U8,
            16 => ChannelType::U16,
            _ => return Err(Error::Unsupported("component sample storage")),
        };
        let encoding = SampleEncoding::new(storage, bits, 0)
            .map_err(|_| Error::Corrupt("component sample encoding"))?;
        let role = [
            PlaneRole::Luma,
            PlaneRole::Cb,
            PlaneRole::Cr,
            PlaneRole::Alpha,
        ][index];
        if role == PlaneRole::Alpha && coding != 0 {
            return Err(Error::Corrupt("component alpha must be exact"));
        }
        planes.push(PlaneSpec::new(role, PlaneEncoding::Code(encoding)));
        grids[index] = (pw, ph);
        codings[index] = coding;
    }
    if descriptor.remaining() != 0 {
        return Err(Error::Corrupt("trailing component descriptor"));
    }
    if (mode == POTENTIALLY_LOSSY) != codings[..count as usize].iter().any(|&c| c != 0) {
        return Err(Error::Corrupt("component losslessness disagreement"));
    }
    let layout = PlanarLayout::new(planes, sampling, siting)
        .map_err(|_| Error::Corrupt("component planar layout"))?;
    for (index, &(pw, ph)) in grids[..count as usize].iter().enumerate() {
        let expected = layout
            .plane_size(index, width, height)
            .ok_or(Error::Corrupt("component plane count"))?;
        if (pw, ph) != (u64::from(expected.0), u64::from(expected.1)) {
            return Err(Error::Corrupt("component visible grid"));
        }
        let PlaneEncoding::Code(encoding) = layout.planes()[index].encoding() else {
            unreachable!()
        };
        let sample_bytes = if encoding.storage() == ChannelType::U8 {
            1
        } else {
            2
        };
        decoded_bytes = pw
            .checked_mul(ph)
            .and_then(|n| n.checked_mul(sample_bytes))
            .and_then(|n| decoded_bytes.checked_add(n))
            .ok_or(Error::TooLarge)?;
    }
    if decoded_bytes > output.max_bytes {
        return Err(Error::TooLarge);
    }
    let mut icc = None;
    let mut metadata = 0u64;
    let mut chunk_count = 0u64;
    let mut rules = Rules::new(false, true);
    if scan_metadata {
        loop {
            chunk_count = chunk_count.checked_add(1).ok_or(Error::TooLarge)?;
            if chunk_count > max_chunks {
                return Err(Error::TooLarge);
            }
            let (tag, len) = chunks::read_head(&mut input)?;
            let kind = rules.head(tag, len)?;
            if tag == chunks::DATA {
                break;
            }
            if tag == crate::pixelsum::PSUM {
                return Err(Error::Unsupported(
                    "packed bitmap table cannot attest component image",
                ));
            }
            if kind == Kind::Checksum {
                let payload = chunks::payload(
                    &mut input,
                    len.ok_or(Error::Corrupt("open component checksum"))?,
                )?;
                crate::checksum::parse_payload(payload)?;
                continue;
            }
            if kind != Kind::Meta {
                return Err(Error::Unsupported("non-metadata before component DATA"));
            }
            let len = len.ok_or(Error::Corrupt("open component metadata"))?;
            metadata = metadata
                .checked_add(len)
                .filter(|n| *n <= max_metadata)
                .ok_or(Error::TooLarge)?;
            let payload = chunks::payload(&mut input, len)?;
            rules.body(tag, payload)?;
            if tag == chunks::ICCP {
                icc = Some(Cow::Borrowed(payload));
            }
            if tag == chunks::MCMP {
                let expanded = crate::metadata_pack::unpacked_len(&payload[4..])?;
                metadata = metadata
                    .checked_add(expanded as u64)
                    .filter(|n| *n <= max_metadata)
                    .ok_or(Error::TooLarge)?;
                if payload[..4] == chunks::ICCP {
                    if expanded as u64 > max_working.saturating_sub(declaration_capacity) {
                        return Err(Error::TooLarge);
                    }
                    icc = Some(Cow::Owned(crate::metadata_pack::expand(
                        &payload[4..],
                        expanded,
                    )?));
                }
            }
        }
    }
    if scan_metadata
        && color_authority == ColorAuthority::Icc
        && icc.as_deref().is_none_or(<[u8]>::is_empty)
    {
        return Err(Error::Corrupt(
            "component ICC authority requires pre-DATA ICC",
        ));
    }
    Ok(ComponentHeader {
        width,
        height,
        orientation,
        decode_speed,
        numeric_features,
        layout,
        cicp,
        alpha_mode,
        color_authority,
        icc,
        data_offset: input.pos,
        decoded_bytes,
        codings,
        #[cfg(feature = "std")]
        checksum,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[cfg(feature = "std")]
    #[test]
    fn reader_refuses_bad_declaration_without_requesting_the_next_chunk() {
        struct HeaderOnly {
            data: Vec<u8>,
            at: usize,
        }
        impl std::io::Read for HeaderOnly {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                assert!(
                    self.at < self.data.len(),
                    "requested future input after invalid declaration"
                );
                let n = out.len().min(self.data.len() - self.at);
                out[..n].copy_from_slice(&self.data[self.at..self.at + n]);
                self.at += n;
                Ok(n)
            }
        }
        let mut data = header(2, Some((0, 2)), 0, &[]);
        data.truncate(data.len() - 5); // The producer has not supplied DATA.
        let mut published = false;
        let error = decode_reader(HeaderOnly { data, at: 0 }, DecodePolicy::default(), |_| {
            published = true;
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(
            error,
            Error::Corrupt("component alpha must be exact")
        ));
        assert!(!published);
    }

    fn header(
        version: u8,
        alpha: Option<(u8, u8)>,
        authority: u8,
        metadata: &[([u8; 4], Vec<u8>)],
    ) -> Vec<u8> {
        let count = if alpha.is_some() { 4 } else { 3 };
        let mut description = vec![version, count, 2, 0];
        if version == 2 {
            description.extend_from_slice(&[authority, alpha.map_or(0, |a| a.0)]);
        }
        description.extend_from_slice(&[9, 16, 9, 0]);
        for role in 0..count {
            description.push(role);
            crate::put_varint(
                &mut description,
                if role == 1 || role == 2 { 33 } else { 65 },
            );
            crate::put_varint(
                &mut description,
                if role == 1 || role == 2 { 17 } else { 33 },
            );
            description.extend_from_slice(&[16, 10, if role == 3 { alpha.unwrap().1 } else { 2 }]);
        }
        let mut body = vec![3, 5];
        crate::put_varint(&mut body, COMPONENT_FEATURE);
        body.extend_from_slice(&[POTENTIALLY_LOSSY, 65, 33]);
        crate::put_varint(&mut body, COMPONENT_DESCRIPTOR);
        crate::put_varint(&mut body, description.len() as u64);
        body.extend_from_slice(&description);
        let mut out = header::SIGNATURE.to_vec();
        crate::put_varint(&mut out, body.len() as u64);
        out.extend_from_slice(&body);
        for (tag, payload) in metadata {
            out.extend_from_slice(tag);
            crate::put_varint(&mut out, payload.len() as u64 + 1);
            out.extend_from_slice(payload);
        }
        out.extend_from_slice(b"DATA\0");
        out
    }

    #[test]
    fn native_declaration_uses_shared_layout_and_packed_apis_keep_refusing() {
        for version in [1, 2] {
            for alpha in [None, Some((0, 0)), Some((1, 0)), Some((2, 0)), Some((3, 0))] {
                if version == 1 && alpha.is_some() {
                    continue;
                }
                let bytes = header(version, alpha, 0, &[]);
                let got = inspect(&bytes, DecodePolicy::default()).unwrap();
                assert_eq!((got.width, got.height), (65, 33));
                assert_eq!(got.layout.subsampling(), ChromaSubsampling::Yuv420);
                assert_eq!(got.layout.siting(), ChromaSiting::Unknown);
                assert_eq!(got.layout.plane_size(1, 65, 33), Some((33, 17)));
                assert_eq!(
                    got.layout.planes().len(),
                    if alpha.is_some() { 4 } else { 3 }
                );
                assert_eq!(
                    got.decoded_bytes,
                    65 * 33 * 2 + 2 * 33 * 17 * 2 + if alpha.is_some() { 65 * 33 * 2 } else { 0 }
                );
                assert_eq!(got.data_offset, bytes.len());
                assert!(got.is_lossy());
                assert_eq!(got.plane_coding(0), Some(Coding::Lossy));
                assert_eq!(got.plane_lossy_color(0), Some(LossyColor::YCoCg));
                assert_eq!(got.plane_coding(4), None);
                if alpha.is_some() {
                    assert_eq!(got.plane_coding(3), Some(Coding::Lossless));
                    assert_eq!(got.layout.plane_size(3, 65, 33), Some((65, 33)));
                }
                assert_eq!(
                    crate::header(&bytes),
                    Err(Error::Unsupported("unknown feature"))
                );
            }
        }
        let bad = header(2, Some((0, 2)), 0, &[]);
        assert!(matches!(
            inspect(&bad, DecodePolicy::default()),
            Err(Error::Corrupt("component alpha must be exact"))
        ));
    }

    #[test]
    fn icc_authority_requires_active_pre_data_bytes_and_history_does_not_override() {
        let missing = header(2, None, 1, &[]);
        assert!(matches!(
            inspect(&missing, DecodePolicy::default()),
            Err(Error::Corrupt(
                "component ICC authority requires pre-DATA ICC"
            ))
        ));
        let bytes = b"opaque ICC preservation witness".to_vec();
        let raw = header(2, None, 1, &[(*b"iCCP", bytes.clone())]);
        let got = inspect(&raw, DecodePolicy::default()).unwrap();
        assert_eq!(got.color_authority, ColorAuthority::Icc);
        assert!(matches!(got.icc, Some(Cow::Borrowed(_))));
        assert_eq!(got.icc.as_deref(), Some(bytes.as_slice()));
        let mut packed = b"iCCP".to_vec();
        packed.extend_from_slice(&crate::metadata_pack::pack_stored(&bytes).unwrap());
        let compressed = header(2, None, 1, &[(*b"mCMP", packed)]);
        let got = inspect(&compressed, DecodePolicy::default()).unwrap();
        assert!(matches!(got.icc, Some(Cow::Owned(_))));
        assert_eq!(got.icc.as_deref(), Some(bytes.as_slice()));
        let duplicate = header(
            2,
            None,
            0,
            &[(*b"iCCP", bytes.clone()), (*b"iCCP", bytes.clone())],
        );
        assert!(matches!(
            inspect(&duplicate, DecodePolicy::default()),
            Err(Error::Corrupt("repeated chunk"))
        ));
        let history = header(2, None, 1, &[(*b"cORg", bytes)]);
        assert!(matches!(
            inspect(&history, DecodePolicy::default()),
            Err(Error::Corrupt(
                "component ICC authority requires pre-DATA ICC"
            ))
        ));
        let metadata_only = header(2, None, 0, &[(*b"iCCP", b"retained metadata".to_vec())]);
        assert_eq!(
            inspect(&metadata_only, DecodePolicy::default())
                .unwrap()
                .color_authority,
            ColorAuthority::Cicp
        );
    }

    #[test]
    fn declaration_admission_does_not_allocate_rasters_or_bypass_policy() {
        let bytes = header(2, Some((0, 0)), 0, &[]);
        for policy in [
            DecodePolicy::default().with_max_header_bytes(0),
            DecodePolicy::default().with_max_input_bytes(0),
            DecodePolicy::default().with_max_working_bytes(0),
            DecodePolicy::default().with_max_chunks(0),
            DecodePolicy::default().with_output_limits(crate::Limits {
                max_pixels: 1,
                ..crate::Limits::DEFAULT
            }),
            DecodePolicy::default().with_output_limits(crate::Limits {
                max_bytes: 1,
                ..crate::Limits::DEFAULT
            }),
        ] {
            assert!(matches!(inspect(&bytes, policy), Err(Error::TooLarge)));
        }
        let metadata = header(2, None, 1, &[(*b"iCCP", b"opaque profile".to_vec())]);
        assert!(matches!(
            inspect(
                &metadata,
                DecodePolicy::default().with_max_metadata_bytes(1)
            ),
            Err(Error::TooLarge)
        ));
        let mut truncated = header(1, None, 0, &[]);
        truncated.truncate(truncated.len() - 1);
        assert!(inspect(&truncated, DecodePolicy::default()).is_err());
    }
}
