//! Exact sample-word packet units. The container supplies geometry and sample meaning.
pub(crate) mod pixels;
mod planes;
#[cfg(feature = "encode")]
use crate::Metadata;
use crate::{Error, ImageInfo};
#[cfg(feature = "encode")]
use alloc::vec::Vec;
#[cfg(feature = "encode")]
use how_far::{Complete, PhaseSpec, ProgressExt, Pulse, Stages, Total};

/// Exact preprocessing policy for quick-pixel units.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum QuickBitSmarts {
    /// Preserve the original sample-word packet/raw choices without bit analysis.
    #[default]
    Off,
    /// Remove constant byte/bit positions and densely pack varying bits.
    Packed,
    /// Also try pixel packets on byte planes and their compacted values.
    Packets,
}

/// Low analysis-cost lossless encoding with independent raw or pixel-packet units.
/// The sample bits, channel identity and alpha interpretation are unchanged.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
#[cfg(feature = "encode")]
pub(crate) struct QuickConfig {
    /// Rows per independent unit (default 64).
    pub stripe_rows: u32,
    /// Columns per unit; zero means the full width.
    pub column_groups: u32,
    /// Maximum encoder workers. Without a thread host, encoding is serial.
    pub threads: usize,
    /// Add the normal IMI encoded-file checksum.
    pub checksum: bool,
    /// The checksum's digest algorithm.
    pub hash: crate::HashAlgorithm,
    /// Optional exact bit analysis; defaults to [`QuickBitSmarts::Off`].
    pub bit_smarts: QuickBitSmarts,
}
#[cfg(feature = "encode")]
impl Default for QuickConfig {
    fn default() -> Self {
        Self {
            stripe_rows: 64,
            column_groups: 0,
            threads: 1,
            checksum: false,
            hash: crate::HashAlgorithm::DEFAULT,
            bit_smarts: QuickBitSmarts::Off,
        }
    }
}

pub(crate) fn shape(info: &ImageInfo, width: usize, height: usize) -> pixels::Shape {
    pixels::Shape {
        width,
        height,
        channels: info.channels.count(),
        bytes: info.sample.bytes(),
    }
}

#[cfg(feature = "encode")]
pub(crate) fn encode_with_progress(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &QuickConfig,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    Stages::new(
        pulse,
        &[
            PhaseSpec::new("quick units", 1, Total::Unknown),
            PhaseSpec::new("index and checksum", 1, Total::Unknown),
        ],
    )
    .complete_with(|stages| {
        let out = stages.run(|p| encode_frame(pixels, stride, info, meta, cfg, p))?;
        stages.run(|p| {
            p.check().map_err(|_| Error::Cancelled)?;
            let out = crate::container::finish_unit_table(out)?;
            let out = crate::container::locate(out)?;
            let out = crate::checksum::finish_file(out, cfg.checksum.then_some(cfg.hash))?;
            p.check().map_err(|_| Error::Cancelled)?;
            Ok(out)
        })
    })
}
#[cfg(feature = "encode")]
pub(crate) fn encode_frame(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &QuickConfig,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    pulse.check().map_err(|_| Error::Cancelled)?;
    info.validate_precision()?;
    info.checked_buffer_len().ok_or(Error::TooLarge)?;
    shape(info, info.width as usize, info.height as usize)
        .check(pixels.len(), stride)
        .map_err(Error::InvalidInput)?;
    if cfg.stripe_rows == 0 {
        return Err(Error::InvalidInput("quick stripe rows"));
    }
    let hs = (cfg.stripe_rows as usize).min(info.height as usize);
    let gw = if cfg.column_groups == 0 {
        info.width
    } else {
        cfg.column_groups.min(info.width)
    } as usize;
    let ng = (info.width as usize).div_ceil(gw);
    let count = (info.height as usize)
        .div_ceil(hs)
        .checked_mul(ng)
        .ok_or(Error::TooLarge)?;
    let mut items = Vec::new();
    items
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    items.extend(0..count);
    let progress = pulse.live();
    let units = crate::par::par_map(items, cfg.threads, |_, u| {
        let y = (u / ng) * hs;
        let x = (u % ng) * gw;
        let s = shape(
            info,
            gw.min(info.width as usize - x),
            hs.min(info.height as usize - y),
        );
        let unit = unit_encode(
            &pixels[y * stride + x * info.bytes_per_pixel()..],
            stride,
            s,
            cfg.bit_smarts,
            pulse,
        )?;
        progress.step(1).map_err(|_| Error::Cancelled)?;
        Ok(unit)
    })
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    let spans: Vec<_> = units
        .iter()
        .map(|u| crate::parse::records::framed_len(crate::v1::R_STRIPE, u.len()))
        .collect();
    let mut records = Vec::new();
    crate::v1::put_unit_table(&mut records, count, &spans);
    let mut data = Vec::new();
    crate::put_varint(
        &mut data,
        crate::v1::H_QUICK
            | if cfg.bit_smarts != QuickBitSmarts::Off {
                crate::v1::H_QUICK_BITS
            } else {
                0
            }
            | if records.is_empty() {
                0
            } else {
                crate::v1::H_RECORDS
            },
    );
    data.push(1);
    crate::put_varint(&mut data, hs as u64);
    crate::put_varint(&mut data, if ng == 1 { 0 } else { gw as u64 });
    if !records.is_empty() {
        crate::put_varint(&mut data, records.len() as u64);
        data.extend_from_slice(&records);
    }
    let payload_len = spans
        .iter()
        .try_fold(data.len(), |n, &span| {
            n.checked_add(usize::try_from(span).ok()?)
        })
        .ok_or(Error::TooLarge)?;
    let mut out = Vec::new();
    out.try_reserve_exact(payload_len.checked_add(256).ok_or(Error::TooLarge)?)
        .map_err(|_| Error::TooLarge)?;
    crate::container::write_head_features(
        &mut out,
        info,
        meta,
        crate::Coding::Lossless,
        None,
        crate::DecodeSpeed::Balanced,
        features(cfg.bit_smarts),
    )?;
    // The unit bytes already exist. Write framing straight into the final file,
    // avoiding an intermediate full DATA allocation and raster-sized copy.
    crate::container::begin_data(&mut out, payload_len);
    out.extend_from_slice(&data);
    for unit in &units {
        pulse.check().map_err(|_| Error::Cancelled)?;
        crate::parse::records::put_record(&mut out, crate::v1::R_STRIPE, unit);
    }
    Ok(out)
}

#[cfg(feature = "encode")]
pub(crate) fn unit_encode(
    input: &[u8],
    stride: usize,
    shape: pixels::Shape,
    bit_smarts: QuickBitSmarts,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    let row = shape
        .check(input.len(), stride)
        .map_err(Error::InvalidInput)?;
    let raw = row.checked_mul(shape.height).ok_or(Error::TooLarge)?;
    let bound = raw
        .checked_add(
            shape
                .width
                .checked_mul(shape.height)
                .ok_or(Error::TooLarge)?,
        )
        .and_then(|n| n.checked_add(1))
        .ok_or(Error::TooLarge)?;
    #[cfg(target_endian = "big")]
    let (native, stride) = {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(raw).map_err(|_| Error::TooLarge)?;
        for y in 0..shape.height {
            for sample in input[y * stride..][..row].chunks_exact(shape.bytes) {
                bytes.extend(sample.iter().rev());
            }
        }
        (bytes, row)
    };
    #[cfg(target_endian = "big")]
    let input = native.as_slice();
    let mut out = Vec::new();
    out.try_reserve_exact(bound).map_err(|_| Error::TooLarge)?;
    out.push(1);
    let mut cancelled = false;
    let result = pixels::encode_into(input, stride, shape, &mut out, &mut || {
        if pulse.check().is_err() {
            cancelled = true;
            Err("cancelled")
        } else {
            Ok(())
        }
    });
    if cancelled {
        return Err(Error::Cancelled);
    }
    result.map_err(Error::InvalidInput)?;
    if out.len() > raw {
        // includes the packet subtype; raw wins ties
        out.clear();
        out.push(0);
        for y in 0..shape.height {
            pulse.check().map_err(|_| Error::Cancelled)?;
            out.extend_from_slice(&input[y * stride..][..row]);
        }
    }
    if bit_smarts != QuickBitSmarts::Off {
        let mut cancelled = false;
        let mut check = || {
            if pulse.check().is_err() {
                cancelled = true;
                Err("cancelled")
            } else {
                Ok(())
            }
        };
        let packed = if bit_smarts == QuickBitSmarts::Packets {
            planes::encode(input, stride, shape, &mut check)
        } else {
            planes::encode_variant(input, stride, shape, false, &mut check)
        };
        if cancelled {
            return Err(Error::Cancelled);
        }
        let packed = packed.map_err(plane_error)?;
        if packed.len() < out.len() {
            out = packed;
        }
    }
    Ok(out)
}

#[cfg(feature = "encode")]
pub(crate) fn features(bits: QuickBitSmarts) -> crate::parse::header::Features {
    let f = crate::parse::header::Features::EMPTY.with(crate::Feature::QuickPixels);
    if bits == QuickBitSmarts::Off {
        f
    } else {
        f.with(crate::Feature::QuickBitSmarts)
    }
}
fn plane_error(error: &'static str) -> Error {
    match error {
        "allocation" | "plane size" => Error::TooLarge,
        "unsupported quick plane mode" => Error::Unsupported(error),
        _ => Error::Corrupt(error),
    }
}
pub(crate) fn state_bytes(info: &ImageInfo, width: u128, bits: bool) -> Result<u128, Error> {
    let base = core::mem::size_of::<State>() as u128;
    if !bits {
        return Ok(base);
    }
    planes::working_bytes(width, info.bytes_per_pixel() as u128)
        .and_then(|heap| base.checked_add(heap))
        .ok_or(Error::TooLarge)
}

pub(crate) struct State {
    packet: Option<pixels::Decoder>,
    planes: Option<planes::State>,
    shape: pixels::Shape,
    row: usize,
}
/// Validate and borrow a raw unit without constructing packet-cache state.
#[cfg(feature = "std")]
pub(crate) fn raw<'a>(
    p: &crate::v1::Parsed<'_>,
    u: usize,
    bytes: &'a [u8],
) -> Result<Option<&'a [u8]>, Error> {
    if bytes.first() != Some(&0) {
        return Ok(None);
    }
    let y = (u / p.ngroups()).checked_mul(p.hs).ok_or(Error::TooLarge)?;
    if y >= p.info.height as usize {
        return Err(Error::Corrupt("quick unit ordinal"));
    }
    let row = p
        .group(u % p.ngroups())
        .1
        .checked_mul(p.bpp())
        .ok_or(Error::TooLarge)?;
    let size = row
        .checked_mul(p.hs.min(p.info.height as usize - y))
        .ok_or(Error::TooLarge)?;
    if bytes.len() != size.checked_add(1).ok_or(Error::TooLarge)? {
        return Err(Error::Corrupt("quick raw unit length"));
    }
    Ok(Some(&bytes[1..]))
}
impl State {
    pub(crate) fn new(p: &crate::v1::Parsed<'_>, u: usize, bytes: &[u8]) -> Result<Self, Error> {
        let y = (u / p.ngroups()).checked_mul(p.hs).ok_or(Error::TooLarge)?;
        if y >= p.info.height as usize {
            return Err(Error::Corrupt("quick unit ordinal"));
        }
        Self::new_rectangle(
            p,
            p.group(u % p.ngroups()).1,
            p.hs.min(p.info.height as usize - y),
            bytes,
        )
    }

    pub(crate) fn new_rectangle(
        p: &crate::v1::Parsed<'_>,
        width: usize,
        rows: usize,
        bytes: &[u8],
    ) -> Result<Self, Error> {
        if width == 0
            || width > p.info.width as usize
            || rows == 0
            || rows > p.hs
            || rows > p.info.height as usize
        {
            return Err(Error::Corrupt("quick rectangle geometry"));
        }
        let shape = shape(&p.info, width, rows);
        let row = shape.row().map_err(Error::Corrupt)?;
        #[cfg(all(feature = "lab", feature = "std"))]
        crate::trace::quick_unit(
            bytes,
            p.info.channels,
            shape.bytes,
            shape.width.saturating_mul(shape.height),
        );
        let mut bit_planes = None;
        let packet = match bytes.first() {
            Some(0) => {
                if bytes.len()
                    != row
                        .checked_mul(shape.height)
                        .and_then(|n| n.checked_add(1))
                        .ok_or(Error::TooLarge)?
                {
                    return Err(Error::Corrupt("quick raw unit length"));
                }
                None
            }
            Some(1) => Some(pixels::Decoder::new(shape).map_err(Error::Corrupt)?),
            Some(2) => {
                if !p.quick_bits {
                    return Err(Error::Corrupt("quick bit planes without declaration"));
                }
                bit_planes = Some(planes::State::new(shape, bytes).map_err(plane_error)?);
                None
            }
            Some(_) => return Err(Error::Unsupported("quick unit mode")),
            None => return Err(Error::Corrupt("truncated quick unit")),
        };
        Ok(Self {
            packet,
            planes: bit_planes,
            shape,
            row: 0,
        })
    }
    #[cfg(all(test, feature = "encode"))]
    pub(crate) fn working_bytes(&self) -> usize {
        core::mem::size_of::<Self>() + self.planes.as_ref().map_or(0, |p| p.heap_bytes())
    }
    pub(crate) fn rows(&self) -> usize {
        self.shape.height
    }
    pub(crate) fn done(&self) -> bool {
        self.row == self.shape.height
    }
    pub(crate) fn next(&mut self, bytes: &[u8], out: &mut [u8]) -> Result<(), Error> {
        let n = self.shape.row().map_err(Error::Corrupt)?;
        if out.len() < n {
            return Err(Error::InvalidInput("short output row"));
        }
        if self.done() {
            return Err(Error::Corrupt("quick unit exhausted"));
        }
        if let Some(planes) = &mut self.planes {
            planes.next(bytes, &mut out[..n]).map_err(plane_error)?;
        } else if let Some(packet) = &mut self.packet {
            packet
                .next(&bytes[1..], &mut out[..n])
                .map_err(Error::Corrupt)?;
        } else {
            let start = 1 + self.row * n;
            out[..n].copy_from_slice(&bytes[start..start + n]);
        }
        #[cfg(target_endian = "big")]
        for sample in out[..n].chunks_exact_mut(self.shape.bytes) {
            sample.reverse();
        }
        self.row += 1;
        Ok(())
    }
}

// Conservative owned-capacity admission, not an RSS prediction. Includes packet
// worst case, retained units, assembly/finalizer copies and scheduler descriptors.
#[cfg(feature = "encode")]
pub(crate) fn working_bytes(
    info: &ImageInfo,
    cfg: &QuickConfig,
    metadata: u128,
) -> Result<u128, Error> {
    if cfg.stripe_rows == 0 || info.width == 0 || info.height == 0 {
        return Err(Error::InvalidInput("quick geometry"));
    }
    let w = u128::from(info.width);
    let h = u128::from(info.height);
    let gw = if cfg.column_groups == 0 {
        w
    } else {
        u128::from(cfg.column_groups).min(w)
    };
    let units = w.div_ceil(gw) * h.div_ceil(u128::from(cfg.stripe_rows));
    let bytes = w * h * (info.bytes_per_pixel() as u128 + 1);
    metadata
        .checked_mul(8)
        .and_then(|n| {
            n.checked_add(
                (if cfg.bit_smarts == QuickBitSmarts::Off {
                    8
                } else {
                    12
                }) * bytes
                    + (if cfg.bit_smarts == QuickBitSmarts::Off {
                        512
                    } else {
                        4096
                    }) * units
                    + (1 << 20),
            )
        })
        .ok_or(Error::TooLarge)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, SampleType, StreamDecoder};
    #[cfg(feature = "std")]
    #[test]
    fn paired_packets_keep_hidden_alpha_cache_runs_and_extents() {
        let a = [255, 1, 2, 3, 17, 127, 160, 136, 194, 29, 0];
        let b = [254, 9, 8, 7, 196];
        let expected = [
            1, 2, 3, 2, 3, 4, 2, 3, 4, 2, 3, 4, 2, 3, 4, 2, 3, 4, 1, 2, 3, 0, 0, 0,
        ];
        let mut ao = [0; 24];
        let mut bo = [0; 18];
        pixels::pair_rgb8(&a, &b, &mut ao, &mut bo).unwrap();
        assert_eq!(ao, expected);
        assert_eq!(bo, [9, 8, 7, 9, 8, 7, 9, 8, 7, 9, 8, 7, 9, 8, 7, 9, 8, 7]);
        for n in 0..a.len() {
            assert!(pixels::pair_rgb8(&a[..n], &b, &mut ao, &mut bo).is_err());
        }
        for n in 0..b.len() {
            assert!(pixels::pair_rgb8(&a, &b[..n], &mut ao, &mut bo).is_err());
        }
        let mut extra = a.to_vec();
        extra.push(0);
        assert!(pixels::pair_rgb8(&extra, &b, &mut ao, &mut bo).is_err());
        assert!(pixels::pair_rgb8(&a, &b, &mut ao[..21], &mut bo).is_err());
        assert!(pixels::pair_rgb8(&a, &b, &mut ao[..23], &mut bo).is_err());
    }
    fn file(body: &[u8], declared: bool) -> Vec<u8> {
        let info = ImageInfo::new(3, 2, Channels::Rgb, SampleType::U8);
        let mut out = Vec::new();
        let features = if declared {
            crate::Features::EMPTY.with(crate::Feature::QuickPixels)
        } else {
            crate::Features::EMPTY
        };
        crate::container::write_head_features(
            &mut out,
            &info,
            &Metadata::default(),
            crate::Coding::Lossless,
            None,
            crate::DecodeSpeed::Balanced,
            features,
        )
        .unwrap();
        let mut data = alloc::vec![8, 1, 2, 0];
        crate::parse::records::put_record(&mut data, crate::v1::R_STRIPE, body);
        crate::container::put_data(&mut out, &data);
        out
    }
    #[test]
    fn bounded_packets_reject_missing_excess_and_unknown_syntax() {
        // Six initially black RGB pixels, crossing the row boundary.
        let valid = file(&[1, 0xc5], true);
        assert_eq!(crate::decode(&valid).unwrap().1, alloc::vec![0;18]);
        assert_eq!(
            crate::decode(&file(&[1, 0xc5], false)).err(),
            Some(Error::Corrupt(
                "quick pixels without the feature quick_pixels"
            ))
        );
        for body in [
            &[][..],
            &[0],
            &[0; 18],
            &[0; 20],
            &[1],
            &[1, 0xfe, 1, 2],
            &[1, 0xc6],
            &[1, 0xc5, 0],
        ] {
            let bad = file(body, true);
            assert!(crate::decode(&bad).is_err(), "{body:?}");
            let mut stream = StreamDecoder::new(&bad).unwrap();
            let mut row = [0xa5; 9];
            let mut failure = None;
            for _ in 0..3 {
                if let Err(e) = stream.next_row(&mut row) {
                    failure = Some(e);
                    break;
                }
            }
            let failure = failure.expect("malformed unit must fail");
            assert_eq!(stream.next_row(&mut row), Err(failure));
        }
        assert_eq!(
            crate::decode(&file(&[2], true)).err(),
            Some(Error::Corrupt("quick bit planes without declaration"))
        );
        assert_eq!(
            crate::decode(&file(&[3], true)).err(),
            Some(Error::Unsupported("quick unit mode"))
        );
    }
    #[test]
    fn bit_plane_declarations_and_bounded_envelopes() {
        fn smart(body: &[u8], flags: u8, features: crate::Features) -> Vec<u8> {
            let info = ImageInfo::new(3, 2, Channels::Rgb, SampleType::U8);
            let mut out = Vec::new();
            crate::container::write_head_features(
                &mut out,
                &info,
                &Metadata::default(),
                crate::Coding::Lossless,
                None,
                crate::DecodeSpeed::Balanced,
                features,
            )
            .unwrap();
            let mut data = alloc::vec![flags, 1, 2, 0];
            crate::parse::records::put_record(&mut data, crate::v1::R_STRIPE, body);
            crate::container::put_data(&mut out, &data);
            out
        }
        let features = crate::Features::EMPTY
            .with(crate::Feature::QuickPixels)
            .with(crate::Feature::QuickBitSmarts);
        let valid = [2, 2, 2, 2, 2, 10, 7, 0, 7, 1];
        let file = smart(&valid, 24, features);
        assert_eq!(crate::decode(&file).unwrap().1, alloc::vec![10;18]);
        for (flags, f) in [
            (16, features),
            (24, crate::Features::EMPTY.with(crate::Feature::QuickPixels)),
            (8, features),
        ] {
            assert!(matches!(
                crate::decode(&smart(&valid, flags, f)),
                Err(Error::Corrupt(_))
            ));
        }
        assert_eq!(
            crate::decode(&smart(&[2, 1, 2, 2, 8, 2, 10, 2, 10], 24, features)).err(),
            Some(Error::Unsupported("unsupported quick plane mode"))
        );
        for body in [
            &[2, 0x82, 0, 2, 2, 2, 10, 2, 10, 2, 10][..],
            &[2, 2, 2, 2, 7, 0, 2, 10, 2, 10],
            &[2, 2, 2, 2, 2, 10, 2, 10, 2, 10, 0],
        ] {
            assert!(matches!(
                crate::decode(&smart(body, 24, features)),
                Err(Error::Corrupt(_))
            ));
        }
    }
    #[test]
    fn fallback_bound_and_packet_run_choice() {
        let shape = pixels::Shape {
            width: 64,
            height: 3,
            channels: 3,
            bytes: 1,
        };
        let zero = alloc::vec![0;64*3*3];
        assert_eq!(
            unit_encode(&zero, 192, shape, QuickBitSmarts::Off, &how_far::NoPulse).unwrap()[0],
            1
        );
        let mut state = 0x19572132u32;
        let noisy: Vec<_> = (0..zero.len())
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        let bytes =
            unit_encode(&noisy, 192, shape, QuickBitSmarts::Off, &how_far::NoPulse).unwrap();
        assert_eq!(bytes[0], 0);
        assert_eq!(bytes.len(), noisy.len() + 1);
        assert_eq!(&bytes[1..], noisy);
    }
}
