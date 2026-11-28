//! Common animation/page items, with caller-owned references.
use super::*;
use crate::config::Lowered;

/// Presentation of a sequence. Pixel dependencies use the same grammar in both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceKind {
    Animation,
    Pages,
}

/// An optional prediction candidate. References are decoded rasters, not source
/// pixels before lossy/near-lossless coding. Only earlier items can be referenced.
/// The encoder retains descriptors and hashes, not a raster cache.
#[derive(Clone, Copy)]
pub struct ImageReference<'a> {
    pub index: usize,
    pub pixels: &'a [u8],
    pub stride: usize,
}

/// One complete, strided image and its item-local metadata.
pub struct SequenceItem<'a> {
    pixels: &'a [u8],
    stride: usize,
    info: &'a ImageInfo,
    metadata: Option<&'a Metadata>,
    coding: Option<&'a crate::ImiEncoderConfig>,
    duration: FrameDuration,
    reference: Option<ImageReference<'a>>,
}
impl<'a> SequenceItem<'a> {
    pub fn new(pixels: &'a [u8], stride: usize, info: &'a ImageInfo) -> Self {
        Self {
            pixels,
            stride,
            info,
            metadata: None,
            coding: None,
            duration: FrameDuration::ZERO,
            reference: None,
        }
    }
    pub fn with_metadata(mut self, metadata: &'a Metadata) -> Self {
        self.metadata = Some(metadata);
        self
    }
    pub fn with_coding(mut self, coding: &'a crate::ImiEncoderConfig) -> Self {
        self.coding = Some(coding);
        self
    }
    pub fn with_duration(mut self, duration: FrameDuration) -> Self {
        self.duration = duration;
        self
    }
    pub fn with_reference(mut self, reference: ImageReference<'a>) -> Self {
        self.reference = Some(reference);
        self
    }
}

/// A unified sequence writer. Both presentations default to independent items;
/// animation callers can offer the previous decoded canvas as a candidate, and
/// page callers can offer any earlier compatible item. An intra encoding wins
/// when smaller or when the configured keyframe interval requires it.
///
/// Complete encoded output is retained until finish. Only a descriptor and
/// canonical decoded-raster hash are retained per prior item. Callers choose
/// which decoded reference rasters to cache. This is not a bounded-output writer.
pub struct SequenceEncoder {
    kind: SequenceKind,
    config: FrameConfig,
    first: Option<(Vec<u8>, FrameDuration)>,
    items: Vec<Vec<u8>>,
    identities: Vec<(ImageInfo, crate::Digest)>,
    features: Features,
    since_key: u32,
    defaults: Vec<u8>,
}
impl SequenceEncoder {
    pub fn new(kind: SequenceKind, config: FrameConfig) -> Self {
        Self {
            kind,
            config,
            first: None,
            items: Vec::new(),
            identities: Vec::new(),
            features: Features::EMPTY.with(Feature::SequenceItems),
            since_key: 0,
            defaults: Vec::new(),
        }
    }
    /// Set advisory metadata shared by this sequence's items.
    /// Only kVAL attributes, named/unnamed sMDT and science descriptors are
    /// accepted. Pixel prediction and rendering continue to use item-local data.
    pub fn set_metadata_defaults(&mut self, chunks: &[crate::Chunk]) -> Result<(), Error> {
        self.defaults = if chunks.is_empty() {
            Vec::new()
        } else {
            defaults::serialize(chunks)?
        };
        Ok(())
    }
    /// Encode one item. All offered references are checked against the exact
    /// reconstructed image previously encoded at that ordinal before prediction.
    /// A reference candidate does not force inter coding; wide/near-lossless and
    /// unsupported lossy layouts continue to use the independent encoding.
    /// Returns the tight reconstructed raster, including lossy/near-lossless
    /// changes, so callers can retain it as a subsequent `ImageReference`.
    pub fn push(&mut self, item: SequenceItem<'_>) -> Result<Vec<u8>, Error> {
        if item.duration.den == 0 {
            return Err(Error::InvalidInput("zero duration denominator"));
        }
        let i = self.identities.len();
        if self.kind == SequenceKind::Animation
            && self
                .identities
                .first()
                .is_some_and(|(info, _)| info != item.info)
        {
            return Err(Error::InvalidInput("animation canvas format changes"));
        }
        let extent = item
            .info
            .checked_stride_len(item.stride)
            .ok_or(Error::InvalidInput("invalid image layout"))?;
        if item.pixels.len() < extent {
            return Err(Error::InvalidInput("pixel buffer too small"));
        }
        let coding = &item.coding.unwrap_or(&self.config.coding).lower();
        let default_meta = Metadata::default();
        let meta = item.metadata.unwrap_or(&default_meta);
        let mut candidate = None;
        if let Some(reference) = item.reference {
            let (info, hash) = self
                .identities
                .get(reference.index)
                .ok_or(Error::InvalidInput("reference is not an earlier image"))?;
            if info != item.info {
                return Err(Error::InvalidInput("reference pixel description differs"));
            }
            if crate::pixelsum::digest_with(
                hash.algorithm(),
                info,
                reference.index as u64,
                reference.pixels,
                reference.stride,
            )? != *hash
            {
                return Err(Error::InvalidInput(
                    "reference raster does not match encoded image",
                ));
            }
            candidate = Some((reference, *hash));
        }
        let mut file = encode_image(coding, item.pixels, item.stride, item.info, meta)?;
        let key_due = self.config.keyframe_interval != 0
            && self.since_key.saturating_add(1) >= self.config.keyframe_interval;
        let mut chosen = None;
        if self.config.inter
            && !key_due
            && let Some((reference, hash)) = candidate
        {
            // Lossless references use one coded u32 word per sample. Wide HI/LO
            // references need a different contract; lossy keeps its existing bound.
            let eligible = match coding {
                Lowered::Lossless(c) => {
                    item.info.sample.depth() <= 32
                        && c.max_error[..item.info.channels.count()]
                            .iter()
                            .all(|&v| v == 0)
                }
                Lowered::Lossy(_) => item.info.sample.depth() <= 16,
                Lowered::Quick(_) => false,
            };
            if eligible {
                let encoded = match coding {
                    Lowered::Quick(_) => {
                        unreachable!("quick units do not use reference predictors")
                    }
                    Lowered::Lossless(c) => crate::v1::encode_frame(
                        item.pixels,
                        item.stride,
                        item.info,
                        meta,
                        c,
                        Some((reference.pixels, reference.stride)),
                    ),
                    Lowered::Lossy(c) => crate::lossy::encode_lossy_frame(
                        item.pixels,
                        item.stride,
                        item.info,
                        meta,
                        c,
                        Some((reference.pixels, reference.stride)),
                    ),
                };
                match encoded {
                    Ok(inter) if inter.len().saturating_add(20) < file.len() => {
                        file = inter;
                        chosen = Some((reference, hash));
                    }
                    Ok(_) | Err(Error::Unsupported(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        let row = item.info.row_bytes();
        let bytes = item.info.checked_stride_len(row).ok_or(Error::TooLarge)?;
        let mut decoded = Vec::new();
        decoded
            .try_reserve_exact(bytes)
            .map_err(|_| Error::TooLarge)?;
        let exact_lossless = match coding {
            Lowered::Quick(_) => true,
            Lowered::Lossless(c) => c.max_error[..item.info.channels.count()]
                .iter()
                .all(|&v| v == 0),
            Lowered::Lossy(_) => false,
        };
        if exact_lossless {
            if item.stride == row {
                decoded.extend_from_slice(&item.pixels[..bytes]);
            } else {
                for y in 0..item.info.height as usize {
                    decoded.extend_from_slice(&item.pixels[y * item.stride..y * item.stride + row]);
                }
            }
        } else {
            decoded.resize(bytes, 0);
            if let Some((reference, _)) = chosen {
                for y in 0..item.info.height as usize {
                    decoded[y * row..(y + 1) * row].copy_from_slice(
                        &reference.pixels[y * reference.stride..y * reference.stride + row],
                    );
                }
            }
            decode_file(&file, &mut decoded, row, 1, chosen.is_some(), None)?;
        }
        let hash = crate::pixelsum::digest_with(
            self.config.coding.hash_algorithm(),
            item.info,
            i as u64,
            &decoded,
            row,
        )?;
        let required = declared(&file)?;
        let payload = if i == 0 {
            None
        } else {
            Some(item_payload(
                item.duration,
                chosen.map(|(r, hash)| (r.index, hash)),
                &file,
            ))
        };
        // Fallible bookkeeping allocations precede mutation of the sequence.
        self.identities
            .try_reserve(1)
            .map_err(|_| Error::TooLarge)?;
        self.items.try_reserve(1).map_err(|_| Error::TooLarge)?;
        self.features = self.features.union(required);
        self.identities.push((item.info.clone(), hash));
        if let Some(payload) = payload {
            self.items.push(payload);
        } else {
            self.first = Some((file, item.duration));
        }
        self.since_key = if chosen.is_some() {
            self.since_key.saturating_add(1)
        } else {
            0
        };
        Ok(decoded)
    }
    pub fn finish(self) -> Result<Vec<u8>, Error> {
        let (first, duration) = self.first.ok_or(Error::InvalidInput("no frames"))?;
        let mut control = vec![4];
        put_varint(&mut control, u64::from(self.config.loop_count));
        put_varint(&mut control, self.identities.len() as u64);
        put_varint(&mut control, duration.num);
        put_varint(&mut control, u64::from(duration.den));
        control.push(u8::from(self.kind == SequenceKind::Animation));
        let chunks = [
            (ANIM, control.as_slice()),
            (defaults::TAG, self.defaults.as_slice()),
        ];
        crate::v4::migrate::finish_sequence(
            first,
            self.items,
            &chunks[..1 + usize::from(!self.defaults.is_empty())],
            Features::EMPTY.with(Feature::SequenceItems),
            self.config.coding.checksum_algorithm(),
            &how_far::NoPulse,
        )
    }

    /// Finish with the retained V3 item grammar for explicit compatibility
    /// consumers. [`Self::finish`] emits V4.
    pub fn finish_legacy(self) -> Result<Vec<u8>, Error> {
        if !self.defaults.is_empty() {
            return Err(Error::Unsupported("sequence defaults require V4"));
        }
        self.finish_container()
    }

    fn finish_container(self) -> Result<Vec<u8>, Error> {
        let (first, duration) = self.first.ok_or(Error::InvalidInput("no frames"))?;
        let first = container::require_features(first, self.features)?;
        let mut control = vec![4];
        put_varint(&mut control, u64::from(self.config.loop_count));
        put_varint(&mut control, self.identities.len() as u64);
        put_varint(&mut control, duration.num);
        put_varint(&mut control, u64::from(duration.den));
        control.push(u8::from(self.kind == SequenceKind::Animation));
        let chunks = [
            (ANIM, control.as_slice()),
            (defaults::TAG, self.defaults.as_slice()),
        ];
        let mut out = container::insert_before_data(
            &first,
            &chunks[..1 + usize::from(!self.defaults.is_empty())],
        )?;
        drop(first);
        let extra = self
            .items
            .iter()
            .try_fold(0usize, |acc, item| {
                let chunk = 4usize
                    .checked_add(crate::varint_len(item.len() as u64 + 1))?
                    .checked_add(item.len())?;
                acc.checked_add(chunk)
            })
            .ok_or(Error::TooLarge)?;
        out.try_reserve_exact(extra).map_err(|_| Error::TooLarge)?;
        for item in self.items {
            container::put_chunk(&mut out, &FRAM, &item);
        }
        let out = container::finish_unit_table(out)?;
        let out = crate::checksum::finish_file(out, self.config.coding.checksum_algorithm())?;
        Ok(out)
    }
}

/// The item's frame payload; a reference carries the earlier item's ordinal,
/// the digest algorithm and the raster digest (frame record 33).
fn item_payload(
    duration: FrameDuration,
    reference: Option<(usize, crate::Digest)>,
    file: &[u8],
) -> Vec<u8> {
    let Some((index, hash)) = reference else {
        return frame_payload(0, 0, duration, 0, file);
    };
    let mut value = Vec::new();
    put_varint(&mut value, index as u64);
    value.push(hash.algorithm().wire());
    value.extend_from_slice(hash.as_bytes());
    let mut records = Vec::new();
    crate::parse::records::put_record(&mut records, crate::registry::FRAME_REFERENCE, &value);
    let mut out = frame_payload(0, 0, duration, F_RECORDS, &[]);
    put_varint(&mut out, records.len() as u64);
    out.extend_from_slice(&records);
    out.extend_from_slice(file);
    out
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn advisory_defaults_resolve_on_demand_without_inheriting_item_zero() {
        use crate::item_metadata::{ItemMetadata, Value};
        let info = ImageInfo::new(8, 8, crate::Channels::Gray, crate::SampleType::U8);
        let mut encoder = SequenceEncoder::new(
            SequenceKind::Pages,
            FrameConfig::new(crate::ImiEncoderConfig::lossless()),
        );
        let mut attributes = ItemMetadata::default();
        attributes
            .attributes
            .insert("instrument".into(), Value::Unsigned(7));
        let mut defaults = Metadata::default();
        attributes.write_metadata(&mut defaults).unwrap();
        defaults.chunks[0].tag = crate::item_metadata::LEGACY_TAG;
        encoder.set_metadata_defaults(&defaults.chunks).unwrap();
        assert_eq!(&encoder.defaults[1..5], &crate::item_metadata::TAG);
        assert_eq!(
            encoder.set_metadata_defaults(&[crate::Chunk {
                tag: *b"iCCP",
                data: vec![1]
            }]),
            Err(Error::Unsupported("sequence default tag"))
        );
        let mut primary = Metadata::default();
        let mut attributes = ItemMetadata {
            filename: Some("primary".into()),
            ..ItemMetadata::default()
        };
        attributes.write_metadata(&mut primary).unwrap();
        encoder
            .push(SequenceItem::new(&[9; 64], 8, &info).with_metadata(&primary))
            .unwrap();
        let mut own = Metadata::default();
        attributes.filename = None;
        attributes
            .attributes
            .insert("instrument".into(), Value::Unsigned(11));
        attributes.write_metadata(&mut own).unwrap();
        encoder
            .push(SequenceItem::new(&[9; 64], 8, &info).with_metadata(&own))
            .unwrap();
        encoder.push(SequenceItem::new(&[9; 64], 8, &info)).unwrap();
        let bytes = encoder.finish().unwrap();
        let mut reader = FrameDecoder::new(&bytes).unwrap();
        for ordinal in [2, 0, 1] {
            reader.seek_frame(ordinal).unwrap();
            assert!(matches!(
                reader.resolved_frame_metadata(1),
                Err(Error::TooLarge)
            ));
            let raw = ItemMetadata::from_metadata(reader.frame_metadata().unwrap()).unwrap();
            let resolved = reader.resolved_frame_metadata(4096).unwrap().unwrap();
            let resolved = ItemMetadata::from_metadata(&resolved).unwrap();
            assert_eq!(
                resolved.filename.as_deref(),
                (ordinal == 0).then_some("primary")
            );
            assert_eq!(
                resolved.attributes["instrument"],
                Value::Unsigned(if ordinal == 1 { 11 } else { 7 })
            );
            assert_eq!(raw.attributes.contains_key("instrument"), ordinal == 1);
            let mut pixels = [0; 64];
            reader.decode_into(&mut pixels, 8, 1).unwrap();
            assert_eq!(pixels, [9; 64]);
        }
    }

    #[test]
    fn positional_open_skips_large_defaults_until_requested() {
        use crate::item_metadata::{ItemMetadata, Value};
        use crate::planes::SliceReader;
        use crate::region::RegionReader;

        let info = ImageInfo::new(8, 8, crate::Channels::Gray, crate::SampleType::U8);
        let mut encoder = SequenceEncoder::new(
            SequenceKind::Pages,
            FrameConfig::new(crate::ImiEncoderConfig::lossless().with_checksum(false)),
        );
        let mut attributes = ItemMetadata::default();
        let blob = Value::Bytes(vec![0x35; 1 << 20]);
        attributes.attributes.insert("blob".into(), blob.clone());
        let mut defaults = Metadata::default();
        attributes.write_metadata(&mut defaults).unwrap();
        encoder.set_metadata_defaults(&defaults.chunks).unwrap();
        for _ in 0..2 {
            encoder.push(SequenceItem::new(&[9; 64], 8, &info)).unwrap();
        }
        let bytes = encoder.finish().unwrap();
        let mut region = RegionReader::open(SliceReader::new(&bytes)).unwrap();
        let before = region.read_stats();
        assert!(before.bytes < 1 << 20);
        assert_eq!(
            region.resolve_sequence_metadata(&info, Metadata::default(), 1 << 20),
            Err(Error::TooLarge)
        );
        assert_eq!(region.read_stats(), before);
        let resolved = region
            .resolve_sequence_metadata(&info, Metadata::default(), 2 << 20)
            .unwrap();
        assert_eq!(region.read_stats().reads - before.reads, 1);
        assert_eq!(
            ItemMetadata::from_metadata(&resolved).unwrap().attributes["blob"],
            blob
        );
    }

    #[test]
    fn indexed_metadata_probe_admits_a_gigapixel_declaration_without_data() {
        let info = ImageInfo::new(
            32_768,
            32_768,
            crate::Channels::Scalar,
            crate::SampleType::U8,
        );
        let mut prefix = Vec::new();
        container::write_group_head_features(
            &mut prefix,
            &info,
            &Metadata::default(),
            Coding::Lossless,
            None,
            crate::DecodeSpeed::Balanced,
            Features::EMPTY,
            4,
        )
        .unwrap();
        let outer = crate::parse::header::read_versioned(&mut ByteReader::new(&prefix), u64::MAX)
            .unwrap()
            .header;
        prefix.extend_from_slice(b"DATA\0");
        assert_eq!(probe_indexed_nested(&prefix, &outer).unwrap().0, info);
    }

    fn forged(reference: &[u8], feature: bool) -> Vec<u8> {
        let info = ImageInfo::new(3, 2, crate::Channels::Gray, crate::SampleType::U8);
        let coding = crate::ImiEncoderConfig::default().lower();
        let first =
            encode_image(&coding, &[1, 2, 3, 4, 5, 6], 3, &info, &Metadata::default()).unwrap();
        let first = if feature {
            container::require_features(first, Features::EMPTY.with(Feature::SequenceItems))
                .unwrap()
        } else {
            first
        };
        let nested =
            encode_image(&coding, &[1, 2, 3, 4, 5, 6], 3, &info, &Metadata::default()).unwrap();
        let mut records = Vec::new();
        crate::parse::records::put_record(
            &mut records,
            crate::registry::FRAME_REFERENCE,
            reference,
        );
        let mut payload = frame_payload(0, 0, FrameDuration::ZERO, F_RECORDS, &[]);
        put_varint(&mut payload, records.len() as u64);
        payload.extend_from_slice(&records);
        payload.extend_from_slice(&nested);
        let mut out =
            container::insert_before_data(&first, &[(ANIM, &[4, 0, 2, 0, 1, 0])]).unwrap();
        container::put_chunk(&mut out, &FRAM, &payload);
        container::finish_unit_table(out).unwrap()
    }
    #[test]
    fn reference_controls_are_admitted_before_any_image() {
        for ordinal in [1, 2, 127, u64::MAX] {
            let mut reference = Vec::new();
            put_varint(&mut reference, ordinal);
            reference.push(crate::HashAlgorithm::Xxh3_64.wire());
            reference.extend_from_slice(&0u64.to_le_bytes());
            assert!(matches!(
                FrameDecoder::new(&forged(&reference, true)),
                Err(Error::Corrupt("reference is not an earlier image"))
            ));
        }
        assert!(matches!(
            FrameDecoder::new(&forged(&[0], true)),
            Err(Error::Corrupt(_))
        ));
        let mut reference = vec![0, crate::HashAlgorithm::Xxh3_64.wire()];
        reference.extend_from_slice(&0u64.to_le_bytes());
        let missing = forged(&reference, false);
        assert!(matches!(
            FrameDecoder::new(&missing),
            Err(Error::Corrupt(
                "aNIM kind 4 needs the feature sequence_items"
            ))
        ));
        assert!(matches!(
            crate::decode(&missing),
            Err(Error::Corrupt(
                "aNIM kind 4 needs the feature sequence_items"
            ))
        ));
        assert!(FrameDecoder::new(&forged(&reference, true)).is_ok());
    }
}
