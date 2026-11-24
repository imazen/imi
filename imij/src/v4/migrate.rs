//! Encoded-domain V4 migration of images and known nested collection structures.
use super::emit::extend;
use crate::decode_resources::{DecodePolicy, DecodeSession};
use crate::parse::{chunks, header, limits::Structure, records};
use crate::{ByteReader, Error, UpgradeLimits};
use alloc::{borrow::Cow, vec::Vec};

/// Upgrade V2/V3 images and known frames/pages, previews and gain maps to V4.
/// Preserves compressed pixel entropy, metadata and reconstructed-raster digests.
/// Complete encoded input/output and nested rewrites are bounded by the caller's
/// byte/depth limits. Plane-set offset/digest finalization remains separate.
/// Already V4 images are structurally checked and retained byte-for-byte when
/// their known nested images need no change. No pixel raster is reconstructed.
pub fn upgrade_image(file: &[u8], limits: &UpgradeLimits) -> Result<Vec<u8>, Error> {
    rewrite(file, limits, 0)
}

// Only used with bytes produced by this encoder. Public migration retains its
// input/parser limits. Encoding already admitted source and configuration; a
// decoder's default byte/model limits must not become new encoder restrictions.
#[cfg(feature = "encode")]
pub(crate) fn finish_encoded(file: Vec<u8>) -> Result<Vec<u8>, Error> {
    finish_produced(file, false)
}

#[cfg(feature = "encode")]
pub(crate) fn finish_encoded_image(file: Vec<u8>) -> Result<Vec<u8>, Error> {
    finish_produced(file, true)
}

#[cfg(feature = "encode")]
fn finish_produced(file: Vec<u8>, fresh_xyb_power: bool) -> Result<Vec<u8>, Error> {
    let limits = UpgradeLimits {
        max_encoded_bytes: usize::MAX,
        ..Default::default()
    };
    rewrite_inner(&file, &limits, 0, true, true, fresh_xyb_power)
}
fn producer_policy() -> DecodePolicy {
    DecodePolicy::default()
        .with_output_limits(crate::Limits::NONE)
        .for_structure()
        .with_max_input_bytes(u64::MAX)
        .with_max_header_bytes(u64::MAX)
        .with_max_metadata_bytes(u64::MAX)
        .with_max_chunks(u64::MAX)
        .with_max_units(u64::MAX)
        .with_max_work_units(u64::MAX)
        .with_max_working_bytes(u64::MAX)
}

pub(crate) fn rewrite(file: &[u8], limits: &UpgradeLimits, depth: usize) -> Result<Vec<u8>, Error> {
    rewrite_inner(file, limits, depth, true, false, false)
}
fn rewrite_primary(file: &[u8], limits: &UpgradeLimits, depth: usize) -> Result<Vec<u8>, Error> {
    rewrite_inner(file, limits, depth, false, false, false)
}
fn rewrite_inner(
    file: &[u8],
    limits: &UpgradeLimits,
    depth: usize,
    allow_planes: bool,
    producer: bool,
    fresh_xyb_power: bool,
) -> Result<Vec<u8>, Error> {
    if file.len() > limits.max_encoded_bytes || depth > limits.max_depth {
        return Err(Error::TooLarge);
    }
    let source = header::read_versioned(&mut ByteReader::new(file), u64::MAX)?;
    if source.declared.contains(crate::Feature::NativeComponents) {
        return Err(Error::Unsupported("native components cannot be upgraded"));
    }
    let version = source.header.version;
    let session = DecodeSession::new(if producer {
        producer_policy()
    } else {
        DecodePolicy::default()
            .with_output_limits(crate::Limits::NONE)
            .for_structure()
    });
    let parsed = if version == 4 {
        super::file::parse_file(file, &session)?
    } else {
        crate::upgrade::parse(file)?
    };
    crate::frames::admit_headers(&parsed, &Structure)?;
    let mut scan = ByteReader::new(file);
    header::read_versioned(&mut scan, u64::MAX)?;
    while scan.remaining() != 0 {
        let (tag, length) = chunks::read_head(&mut scan)?;
        if chunks::is_plane_chunk(&tag) {
            if !allow_planes {
                return Err(Error::Corrupt("plane primary contains plane chunks"));
            }
            if file.len() < 37 {
                return Err(Error::Corrupt("missing plane locator"));
            }
            return crate::planes::upgrade_image_planes_with(
                file,
                parsed,
                limits,
                depth,
                4,
                rewrite_primary,
                rewrite,
            );
        }
        match length {
            Some(n) => {
                chunks::payload(&mut scan, n)?;
            }
            None => {
                scan.bytes(parsed.data.len())?;
            }
        }
    }
    let summed = crate::container::ChecksumChunk::rewritable(parsed.checksum)?;
    let (mut data, mut tail, mut needed) = if version == 4 {
        // A retained directory is not itself proof of physical framing. Walk
        // models/tile schedules without reconstructing pixel entropy.
        let limits = super::TileLimits {
            max_object_bytes: u64::MAX,
            max_tile_bytes: u64::MAX,
        };
        let image = parsed.image_view();
        match parsed.header.coding {
            crate::Coding::Lossless => {
                super::NumericFile::from_parsed(image, file.len(), limits, &session)?
                    .visit_groups(|_| Ok(()))?;
            }
            crate::Coding::Lossy => {
                super::LossyFile::from_parsed(image, file.len(), limits, &session)?
                    .visit_units(|_| Ok(()))?;
            }
        }
        (
            Cow::Borrowed(parsed.data),
            parsed.trailing_index.map(|(body, _)| Cow::Borrowed(body)),
            crate::Features::EMPTY,
        )
    } else {
        let converted = match parsed.header.coding {
            crate::Coding::Lossless => super::numeric_migrate::payload(
                parsed.header.info.clone(),
                version,
                parsed.declared,
                parsed.data,
                limits.max_encoded_bytes,
                &session,
            )?,
            crate::Coding::Lossy => crate::lossy::v4::convert::payload(
                parsed.header.info.clone(),
                version,
                parsed
                    .header
                    .lossy_color
                    .ok_or(Error::Corrupt("missing lossy color"))?,
                parsed.declared,
                parsed.data,
                limits.max_encoded_bytes,
                fresh_xyb_power && matches!(parsed.pixelsum, crate::pixelsum::Table::Absent),
                &session,
            )?,
        };
        (
            Cow::Owned(converted.data),
            Some(Cow::Owned(converted.index)),
            converted.extra_features,
        )
    };
    // Completed images know the primary directory before publication.
    // Keep it beside primary setup so selecting an early item does not seek
    // through a distant EOF and evict the primary's transport window.
    // Forward-only producers retain their explicitly selected placement;
    // already-V4 migration retains its byte-preservation contract.
    if version != 4 {
        let index = tail
            .take()
            .ok_or(Error::Corrupt("missing converted directory"))?;
        let chunk_len = 4usize
            .checked_add(crate::varint_len(index.len() as u64 + 1))
            .and_then(|n| n.checked_add(index.len()))
            .ok_or(Error::TooLarge)?;
        let body = super::file::check_tail_locator(
            &index,
            u64::try_from(chunk_len).map_err(|_| Error::TooLarge)?,
        )?;
        data = Cow::Owned(super::reframe::front_payload(&data, body, limits)?);
    }
    let mut r = ByteReader::new(file);
    let hp = header::read_versioned(&mut r, u64::MAX)?;
    let header_end = r.pos;
    let mut body_out = Vec::new();
    let mut changed = version != 4;
    while r.remaining() != 0 {
        let start = r.pos;
        let (tag, length) = chunks::read_head(&mut r)?;
        let body = match length {
            Some(length) => chunks::payload(&mut r, length)?,
            None => r.bytes(parsed.data.len())?,
        };
        match &tag {
            b"DATA" => crate::upgrade::chunk(&mut body_out, b"DATA", &data, limits)?,
            b"cSUM" | b"uIDX" => {}
            &crate::item_metadata::LEGACY_TAG if version < 4 => {
                crate::upgrade::chunk(&mut body_out, &crate::item_metadata::TAG, body, limits)?;
            }
            b"fRAM" => {
                let nested =
                    crate::upgrade::frame_with(body, version, limits, depth, &|f, l, d| {
                        rewrite_inner(f, l, d, true, producer, false)
                    })?;
                changed |= nested != body;
                if let Some(image) = crate::frames::frame_file(&nested, 4)? {
                    let h = header::read_versioned(&mut ByteReader::new(image), u64::MAX)?;
                    needed = needed.union(h.declared);
                }
                crate::upgrade::chunk(&mut body_out, &tag, &nested, limits)?;
            }
            b"pRVW" => {
                let nested = rewrite_inner(body, limits, depth + 1, true, producer, false)?;
                changed |= nested != body;
                crate::upgrade::chunk(&mut body_out, &tag, &nested, limits)?;
            }
            b"gMAP" if !producer => {
                let nested = crate::upgrade::gain_map_with(body, limits, depth, &rewrite)?;
                changed |= nested != body;
                crate::upgrade::chunk(&mut body_out, &tag, &nested, limits)?;
            }
            _ => extend(&mut body_out, &file[start..r.pos], limits.max_encoded_bytes)?,
        }
    }
    if version == 4 && !changed {
        let mut output = Vec::new();
        extend(&mut output, file, limits.max_encoded_bytes)?;
        return Ok(output);
    }
    if let Some(tail) = tail {
        crate::upgrade::chunk(&mut body_out, b"uIDX", &tail, limits)?;
    }
    // Descendants can add algorithm capabilities (for example bank 1 under
    // the new model grammar). Form the outer mask only after those are known.
    let fields = &file[header_end - hp.len..hp.ext_at];
    let mut f = ByteReader::new(fields);
    f.u8()?;
    let original_present = f.varint()?;
    let present = original_present
        | if needed.bits() != 0 {
            header::F_FEATURES
        } else {
            0
        };
    if original_present & header::F_FEATURES != 0 {
        header::read_feature_mask(&mut f)?;
    }
    let mut head = Vec::new();
    head.try_reserve_exact(hp.len + 20)
        .map_err(|_| Error::TooLarge)?;
    head.push(4);
    crate::put_varint(&mut head, present);
    if present & header::F_FEATURES != 0 {
        crate::put_varint(&mut head, (hp.mask.bits() & !(0x3f << 26)) | needed.bits());
    }
    extend(&mut head, &fields[f.pos..], limits.max_encoded_bytes)?;
    records::head_records(&file[hp.ext_at..header_end], |id, body| {
        if id != header::EXT_LOCATOR {
            if id == crate::registry::HEADER_CHECKSUM && body.len() == 1 {
                crate::upgrade::record(
                    &mut head,
                    id,
                    &[body[0], crate::HashAlgorithm::Xxh3_64.wire()],
                    limits,
                )?;
            } else {
                crate::upgrade::record(&mut head, id, body, limits)?;
            }
        }
        Ok(())
    })?;
    let mut prefix = Vec::new();
    extend(&mut prefix, &header::SIGNATURE, limits.max_encoded_bytes)?;
    crate::put_varint(&mut prefix, head.len() as u64);
    extend(&mut prefix, &head, limits.max_encoded_bytes)?;
    let length = body_out
        .len()
        .checked_add(prefix.len())
        .filter(|&n| n <= limits.max_encoded_bytes)
        .ok_or(Error::TooLarge)?;
    body_out
        .try_reserve_exact(prefix.len())
        .map_err(|_| Error::TooLarge)?;
    let previous = body_out.len();
    body_out.resize(length, 0);
    body_out.copy_within(0..previous, prefix.len());
    body_out[..prefix.len()].copy_from_slice(&prefix);
    if summed {
        body_out = crate::checksum::refinish(body_out)?;
    }
    if body_out.len() > limits.max_encoded_bytes {
        return Err(Error::TooLarge);
    }
    super::file::parse_file(
        &body_out,
        &DecodeSession::new(if producer {
            producer_policy()
        } else {
            DecodePolicy::default()
                .with_output_limits(crate::Limits::NONE)
                .for_structure()
        }),
    )?;
    Ok(body_out)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
    use alloc::vec;
    #[test]
    fn completed_sequences_put_the_primary_directory_before_pixels() {
        for checksum in [false, true] {
            let info = ImageInfo::new(65, 33, Channels::Gray, SampleType::U8);
            let config = ImiEncoderConfig::lossless()
                .with_threads(1)
                .with_checksum(checksum);
            let mut encoder = crate::FrameEncoder::pages(
                info.clone(),
                Metadata::default(),
                crate::FrameConfig::new(config),
            );
            let frames: Vec<Vec<u8>> = (0..3)
                .map(|frame| {
                    (0..info.checked_buffer_len().unwrap())
                        .map(|at| (at * 37 + frame * 91) as u8)
                        .collect()
                })
                .collect();
            for pixels in &frames {
                encoder.push_page(pixels, info.row_bytes(), &info).unwrap();
            }
            let file = encoder.finish().unwrap();
            let session = DecodeSession::new(DecodePolicy::default());
            let parsed = super::super::file::parse_file(&file, &session).unwrap();
            let mut data = ByteReader::new(parsed.data);
            let setup = data.var_usize().unwrap();
            data.bytes(setup).unwrap();
            assert_eq!(
                super::super::DirectoryPlacement::try_from(data.varint().unwrap()).unwrap(),
                super::super::DirectoryPlacement::Front
            );
            assert!(parsed.trailing_index.is_none());
            let mut decoder = crate::FrameDecoder::new(&file).unwrap();
            for (ordinal, expected) in frames.iter().enumerate() {
                decoder.seek_frame(ordinal).unwrap();
                let mut pixels = vec![0; expected.len()];
                decoder
                    .decode_into(&mut pixels, info.row_bytes(), 1)
                    .unwrap();
                assert_eq!(&pixels, expected);
            }
            assert_eq!(
                upgrade_image(&file, &UpgradeLimits::default()).unwrap(),
                file
            );
        }
    }

    #[test]
    fn previews_and_gain_maps_rewrite_without_losing_metadata_or_pixel_bits() {
        let nested = include_bytes!("../../../fuzz/seeds/24-seed-csum-lossless.imi").to_vec();
        let metadata = Metadata {
            gain_map: Some(crate::GainMap {
                params: crate::GainMapParams::default(),
                alt_cicp: Some(crate::Cicp::SRGB),
                alt_icc: Some(b"opaque profile".to_vec()),
                image: nested.clone(),
            }),
            ..Default::default()
        };
        let info = ImageInfo::new(1, 1, Channels::Gray, SampleType::U8);
        let file = ImiEncoderConfig::default()
            .with_checksum(false)
            .encode_samples(ImageRef::new(&[42], 1, &info), &metadata)
            .unwrap();
        let mut r = ByteReader::new(&file);
        header::read_versioned(&mut r, u64::MAX).unwrap();
        let mut combined = file[..r.pos].to_vec();
        crate::upgrade::chunk(&mut combined, b"pRVW", &nested, &Default::default()).unwrap();
        combined.extend_from_slice(&file[r.pos..]);
        let output = upgrade_image(&combined, &Default::default()).unwrap();
        assert_eq!(
            crate::decode(&combined).unwrap(),
            crate::decode(&output).unwrap()
        );
        let (_, meta) = crate::probe(&output).unwrap();
        let gain = meta.gain_map.unwrap();
        assert_eq!(gain.params, metadata.gain_map.as_ref().unwrap().params);
        assert_eq!(gain.alt_cicp, Some(crate::Cicp::SRGB));
        assert_eq!(gain.alt_icc, Some(b"opaque profile".to_vec()));
        assert_eq!(crate::header(&gain.image).unwrap().version, 4);
        assert_eq!(
            crate::decode(&gain.image).unwrap(),
            crate::decode(&nested).unwrap()
        );
        let previews = crate::previews(&output).unwrap();
        assert_eq!(previews.len(), 1);
        assert_eq!(crate::header(previews[0]).unwrap().version, 4);
        assert_eq!(
            crate::decode(previews[0]).unwrap(),
            crate::decode(&nested).unwrap()
        );
        assert_eq!(upgrade_image(&output, &Default::default()).unwrap(), output);
        let limits = UpgradeLimits {
            max_depth: 0,
            ..Default::default()
        };
        assert_eq!(upgrade_image(&combined, &limits), Err(Error::TooLarge));
    }

    #[test]
    fn numeric_migration_preserves_all_native_sample_bits_and_strided_padding() {
        for sample in [
            SampleType::U8,
            SampleType::U16,
            SampleType::U32,
            SampleType::U64,
            SampleType::I8,
            SampleType::I16,
            SampleType::I32,
            SampleType::I64,
            SampleType::F16,
            SampleType::F32,
            SampleType::F64,
        ] {
            for channels in [
                Channels::Gray,
                Channels::GrayAlpha,
                Channels::Rgb,
                Channels::Rgba,
                Channels::Cmyk,
                Channels::Oklab,
                Channels::OklabAlpha,
            ] {
                let info = ImageInfo::new(41, 31, channels, sample);
                let source: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| (i * 73 + i / 7 * 41) as u8)
                    .collect();
                for quick in [false, true] {
                    let config = if quick {
                        ImiEncoderConfig::quick()
                    } else {
                        ImiEncoderConfig::lossless()
                    }
                    .with_unit_ordinals(true)
                    .with_column_groups(Some(9))
                    .tune_lossless(|t| t.stripe_rows = Some(7));
                    let meta = Metadata {
                        orientation: 6,
                        xmp: Some(b"<migration/>".to_vec()),
                        ..Default::default()
                    };
                    let native = config
                        .encode_legacy_samples(ImageRef::packed(&source, &info), &meta)
                        .unwrap();
                    let migrated = upgrade_image(&native, &UpgradeLimits::default()).unwrap();
                    let check = DecodeSession::new(DecodePolicy::default());
                    let file = crate::v4::NumericFile::open(
                        &migrated,
                        crate::v4::TileLimits {
                            max_object_bytes: 8 << 20,
                            max_tile_bytes: 64 << 20,
                        },
                        &check,
                    )
                    .unwrap();
                    let stride = info.row_bytes() + 17;
                    let mut output = vec![0x75; stride * info.height as usize];
                    let (header, metadata) = file.decode_into(&mut output, stride).unwrap();
                    assert_eq!(header.info, info);
                    assert_eq!(metadata.orientation, meta.orientation);
                    assert_eq!(metadata.xmp, meta.xmp);
                    for y in 0..info.height as usize {
                        assert_eq!(
                            &output[y * stride..][..info.row_bytes()],
                            &source[y * info.row_bytes()..][..info.row_bytes()],
                            "{sample:?} {channels:?} quick{quick}"
                        );
                        assert!(
                            output[y * stride + info.row_bytes()..(y + 1) * stride]
                                .iter()
                                .all(|&b| b == 0x75)
                        );
                    }
                }
            }
        }
    }
}
