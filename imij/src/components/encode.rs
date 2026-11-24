//! Native component publication using existing incremental numeric encoders.
mod producer;
#[cfg(test)]
mod tests;

use super::numeric;
use crate::decode_resources::{DecodePolicy, feed::Budget};
use crate::parse::header::Features;
use crate::stream_encode::checked::Pieces;
use crate::stream_encode::spool::Spool;
use crate::stream_encode::{GroupRect, StreamEncodeError};
use crate::{ByteReader, Coder, Error, HashAlgorithm, Hasher, ImiEncoderConfig, put_varint};
use alloc::vec::Vec;
use std::io::{Read, Seek, Write};
use zenpixels::plane::{AnyPlane, PlanarLayout, PlaneEncoding, PlaneRole};
use zenpixels::sample::{ChromaSiting, ChromaSubsampling};
use zenpixels::{AlphaMode, ChannelType, Cicp, ColorAuthority};

/// Supplies exactly a requested component rectangle, with native typed codes.
///
/// Returned rows may be strided; final-row padding is unnecessary. The borrow
/// ends before another request. A source may retain just its current input band.
/// Errors terminate encoding; discard partial output and provisional rows.
pub trait ComponentSource {
    fn group(&mut self, component: u8, rect: GroupRect) -> Result<AnyPlane<'_>, StreamEncodeError>;
}

/// Explicit aggregate codec/output and caller-scratch admission for this lab path.
/// Caller source/sink/scratch backing stores and allocator overhead are excluded
/// from working admission. These capacities do not guarantee process RSS or CBR.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ComponentEncodeLimits {
    pub max_working_bytes: u64,
    pub max_output_bytes: u64,
    pub max_index_bytes: u64,
    pub max_checksum_bytes: u64,
    pub max_numeric_scratch_bytes: u64,
}
impl ComponentEncodeLimits {
    pub const fn new(working: u64, output: u64, index: u64, checksum: u64, numeric: u64) -> Self {
        Self {
            max_working_bytes: working,
            max_output_bytes: output,
            max_index_bytes: index,
            max_checksum_bytes: checksum,
            max_numeric_scratch_bytes: numeric,
        }
    }
}

/// Unallocated component-mode configuration, for review under `lab + std`.
///
/// Lossless/quick planes use the existing fixed group encoder and require row
/// groups; lossy planes use the restricted first-band-trained one-pass profile.
/// Per-plane file checksums/ordinals are refused: one shared index and the
/// explicitly configured outer checksum bind this file. Alpha must be exact.
/// The initial writer emits DATA2, without decoded pSUM attestation. It does
/// not convert color, change source codes, or allocate a composed frame raster.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ComponentEncodeConfig<'a> {
    pub width: u32,
    pub height: u32,
    pub layout: &'a PlanarLayout,
    pub cicp: Cicp,
    pub orientation: u8,
    pub alpha_mode: AlphaMode,
    pub color_authority: ColorAuthority,
    pub icc: Option<&'a [u8]>,
    pub encoders: &'a [ImiEncoderConfig],
    pub limits: ComponentEncodeLimits,
    pub piece_log2: u8,
    pub hash_algorithm: HashAlgorithm,
}
impl<'a> ComponentEncodeConfig<'a> {
    pub fn new(
        width: u32,
        height: u32,
        layout: &'a PlanarLayout,
        cicp: Cicp,
        encoders: &'a [ImiEncoderConfig],
        limits: ComponentEncodeLimits,
    ) -> Self {
        Self {
            width,
            height,
            layout,
            cicp,
            orientation: 1,
            alpha_mode: AlphaMode::Straight,
            color_authority: ColorAuthority::Cicp,
            icc: None,
            encoders,
            limits,
            piece_log2: 14,
            hash_algorithm: HashAlgorithm::Xxh3_128,
        }
    }
}

fn scalar(value: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(10);
    put_varint(&mut bytes, value);
    bytes
}

fn declaration(
    cfg: &ComponentEncodeConfig<'_>,
    codings: &[u8],
    features: Features,
) -> Result<Vec<u8>, Error> {
    let count = cfg.layout.planes().len();
    if !(3..=4).contains(&count) || codings.len() != count {
        return Err(Error::InvalidInput("component encoder plane count"));
    }
    let sampling = match cfg.layout.subsampling() {
        ChromaSubsampling::Yuv444 => 0,
        ChromaSubsampling::Yuv422 => 1,
        ChromaSubsampling::Yuv420 => 2,
        _ => return Err(Error::Unsupported("component sampling")),
    };
    let siting = match cfg.layout.siting() {
        ChromaSiting::Unknown => 0,
        ChromaSiting::Center => 1,
        ChromaSiting::Left => 2,
        ChromaSiting::TopLeft => 3,
        _ => return Err(Error::Unsupported("component siting")),
    };
    let authority = match cfg.color_authority {
        ColorAuthority::Cicp => 0,
        ColorAuthority::Icc => 1,
        _ => return Err(Error::Unsupported("component color authority")),
    };
    if cfg.icc.is_some_and(<[u8]>::is_empty) || (authority == 1 && cfg.icc.is_none()) {
        return Err(Error::InvalidInput(
            "component ICC authority requires a nonempty profile",
        ));
    }
    let alpha = match cfg.alpha_mode {
        AlphaMode::Straight => 0,
        AlphaMode::Premultiplied => 1,
        AlphaMode::Opaque => 2,
        AlphaMode::Undefined => 3,
        _ => return Err(Error::Unsupported("component alpha interpretation")),
    };
    let mut descriptor = Vec::with_capacity(128);
    descriptor.extend_from_slice(&[
        2,
        count as u8,
        sampling,
        siting,
        authority,
        alpha,
        cfg.cicp.color_primaries,
        cfg.cicp.transfer_characteristics,
        cfg.cicp.matrix_coefficients,
        u8::from(cfg.cicp.full_range),
    ]);
    for (role, (plane, &coding)) in cfg.layout.planes().iter().zip(codings).enumerate() {
        if plane.role()
            != [
                PlaneRole::Luma,
                PlaneRole::Cb,
                PlaneRole::Cr,
                PlaneRole::Alpha,
            ][role]
        {
            return Err(Error::Unsupported(
                "component encoder requires separate ordered Y/Cb/Cr/Alpha",
            ));
        }
        let PlaneEncoding::Code(encoding) = plane.encoding() else {
            return Err(Error::Unsupported("component integer codes required"));
        };
        if encoding.bit_shift() != 0 || encoding.code_bits() < 8 {
            return Err(Error::Unsupported(
                "component source must supply unshifted native codes",
            ));
        }
        let storage = match encoding.storage() {
            ChannelType::U8 => 8,
            ChannelType::U16 => 16,
            _ => return Err(Error::Unsupported("component U8/U16 storage required")),
        };
        let (w, h) = cfg
            .layout
            .plane_size(role, cfg.width, cfg.height)
            .ok_or(Error::TooLarge)?;
        descriptor.push(role as u8);
        put_varint(&mut descriptor, u64::from(w));
        put_varint(&mut descriptor, u64::from(h));
        descriptor.extend_from_slice(&[storage, encoding.code_bits(), coding]);
    }
    let mut body = Vec::with_capacity(256);
    body.push(3);
    put_varint(&mut body, 21); // coding, features, explicit orientation
    put_varint(&mut body, super::COMPONENT_FEATURE | features.bits());
    body.extend_from_slice(&[
        if codings.iter().all(|&c| c == 0) {
            super::ALL_LOSSLESS
        } else {
            super::POTENTIALLY_LOSSY
        },
        cfg.orientation,
    ]);
    put_varint(&mut body, u64::from(cfg.width));
    put_varint(&mut body, u64::from(cfg.height));
    put_varint(&mut body, crate::checksum::EXT_CHECKSUM);
    put_varint(&mut body, 2);
    body.extend_from_slice(&[cfg.piece_log2, cfg.hash_algorithm.wire()]);
    put_varint(&mut body, super::COMPONENT_DESCRIPTOR);
    put_varint(&mut body, descriptor.len() as u64);
    body.extend_from_slice(&descriptor);
    let mut prefix = crate::parse::header::SIGNATURE.to_vec();
    put_varint(&mut prefix, body.len() as u64);
    prefix.extend_from_slice(&body);
    // Share actual declaration admission; do not synthesize packed ImageInfo.
    super::inspect_declaration(&prefix, DecodePolicy::default())?;
    Ok(prefix)
}

struct Sink<W> {
    writer: W,
    at: u64,
    max: u64,
}
impl<W: Write> Sink<W> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), StreamEncodeError> {
        let end = self
            .at
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= self.max)
            .ok_or(Error::TooLarge)?;
        self.writer.write_all(bytes)?;
        self.at = end;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), StreamEncodeError> {
        self.writer.flush()?;
        Ok(())
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Encode fresh typed component groups, publishing before requesting later groups.
///
/// Fixed configuration, conservative codec plans and index counts are admitted
/// before caller I/O. Source-dependent lossy heads are validated after reading
/// the initial source bands, before publication. Each later source rectangle is
/// requested once. The first common group prioritizes the component with the
/// fewest published luma-grid rows; later groups batch each component's work.
/// Writes are flushed at
/// every complete native unit; source/sink adapters own their own buffering.
/// Numeric encoders retain bands, shared heads are emitted once, and no complete
/// per-plane file or frame raster is collected. Existing numeric payloads/coders
/// are reused. The hard whole-file cap is not target-rate accuracy or CBR.
///
/// Supply one numeric scratch object per component; lossless entries are unused.
/// Only lossy entries share `max_numeric_scratch_bytes`. Index/checksum scratch
/// have separate explicit aggregate limits. Existing scratch prefixes survive.
/// Cancellation is checked at producer steps/records, not within a running
/// transform. Failure requires discarding all partial/provisional output.
#[allow(clippy::too_many_arguments)]
pub fn encode_groups<
    W: Write,
    N: Read + Write + Seek,
    I: Read + Write + Seek,
    C: Read + Write + Seek,
>(
    cfg: &ComponentEncodeConfig<'_>,
    source: &mut impl ComponentSource,
    writer: W,
    numeric_scratch: &mut [N],
    index_scratch: I,
    checksum_scratch: C,
    stop: &dyn how_far::Stop,
) -> Result<(W, I, C), StreamEncodeError> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let count = cfg.layout.planes().len();
    if cfg.encoders.len() != count || numeric_scratch.len() != count || !(3..=4).contains(&count) {
        return Err(Error::InvalidInput("component encoder configuration count").into());
    }
    if !(crate::checksum::MIN_PIECE_LOG2..=crate::checksum::MAX_PIECE_LOG2)
        .contains(&cfg.piece_log2)
    {
        return Err(Error::InvalidInput("component checksum piece size").into());
    }
    if cfg.limits.max_output_bytes == 0
        || cfg.limits.max_checksum_bytes < cfg.hash_algorithm.width() as u64
    {
        return Err(Error::TooLarge.into());
    }
    let share = cfg
        .limits
        .max_working_bytes
        .checked_sub(4 << 20)
        .ok_or(Error::TooLarge)?
        / count as u64;
    let lossy = cfg
        .encoders
        .iter()
        .filter(|c| matches!(c.coder(), Coder::Lossy(_)))
        .count();
    let numeric_limit = if lossy == 0 {
        0
    } else {
        cfg.limits.max_numeric_scratch_bytes / lossy as u64
    };
    let dummy: Vec<_> = cfg
        .encoders
        .iter()
        .map(|c| u8::from(matches!(c.coder(), Coder::Lossy(_))))
        .collect();
    declaration(cfg, &dummy, Features::EMPTY)?;
    let mut producers = Vec::new();
    producers
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    let mut record_count = 0u64;
    for role in 0..count {
        let producer = producer::Producer::new(cfg, role, share / 2, numeric_limit)?;
        record_count = record_count
            .checked_add(producer.records)
            .ok_or(Error::TooLarge)?;
        producers.push(producer);
    }
    if record_count
        .checked_mul(30)
        .and_then(|n| n.checked_add(64))
        .is_none_or(|n| n > cfg.limits.max_index_bytes)
    {
        return Err(Error::TooLarge.into());
    }
    // No caller scratch is touched until every producer has passed admission.
    let mut index = Spool::new(
        index_scratch,
        cfg.limits.max_index_bytes,
        "component index scratch changed",
    )?;
    let mut pieces = Pieces::new(
        checksum_scratch,
        cfg.piece_log2,
        cfg.hash_algorithm,
        cfg.limits.max_checksum_bytes,
    )?;
    for (role, (producer, scratch)) in producers
        .iter_mut()
        .zip(numeric_scratch.iter_mut())
        .enumerate()
    {
        producer.prime(role as u8, source, scratch, stop)?;
    }
    let codings: Vec<_> = producers.iter().map(|p| p.coding).collect();
    let features = producers
        .iter()
        .fold(Features::EMPTY, |f, p| f.union(p.features));
    let header = declaration(cfg, &codings, features)?;
    let admitted = super::inspect_declaration(&header, DecodePolicy::default())?;
    let mut cursors = Vec::new();
    cursors
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    let mut common = 1u64;
    for (role, producer) in producers.iter().enumerate() {
        let budget = Budget::new(
            DecodePolicy::default().with_max_working_bytes(share / 2),
            None,
            zencodec::ResourceLimits::none(),
        )?;
        let parsed =
            numeric::Head::parse(&admitted, role, producer.features, &producer.head, &budget)?;
        let cursor = parsed.cursor();
        if parsed.ordinals || cursor.count()? as u64 != producer.records {
            return Err(Error::Corrupt("component producer head geometry").into());
        }
        let sy = if matches!(role, 1 | 2) && cfg.layout.subsampling() == ChromaSubsampling::Yuv420 {
            2
        } else {
            1
        };
        let rows = (cursor.rows as u64)
            .checked_mul(sy)
            .ok_or(Error::TooLarge)?;
        common = (common / gcd(common, rows))
            .checked_mul(rows)
            .ok_or(Error::TooLarge)?;
        cursors.push(cursor);
    }
    let mut sink = Sink {
        writer,
        at: 0,
        max: cfg.limits.max_output_bytes,
    };
    let mut prefix = Hasher::new(HashAlgorithm::Xxh3_128);
    let mut write_prefix = |bytes: &[u8]| -> Result<(), StreamEncodeError> {
        pieces.update(bytes)?;
        prefix.update(bytes);
        sink.put(bytes)
    };
    write_prefix(&header)?;
    if let Some(icc) = cfg.icc {
        write_prefix(b"iCCP")?;
        write_prefix(&scalar(
            (icc.len() as u64).checked_add(1).ok_or(Error::TooLarge)?,
        ))?;
        write_prefix(icc)?;
    }
    write_prefix(b"DATA\0")?;
    let data_at = sink.at;
    let mut heads = Vec::new();
    heads
        .try_reserve_exact(count * (65536 + 32))
        .map_err(|_| Error::TooLarge)?;
    heads.extend_from_slice(&[2, count as u8]);
    for (role, p) in producers.iter().enumerate() {
        heads.push(role as u8);
        put_varint(&mut heads, p.features.bits());
        heads.push(p.speed as u8);
        put_varint(&mut heads, p.head.len() as u64);
        heads.extend_from_slice(&p.head);
    }
    pieces.update(&heads)?;
    prefix.update(&heads);
    sink.put(&heads)?;
    sink.flush()?;
    let mut start = Vec::with_capacity(27);
    start.push(253);
    start.extend_from_slice(prefix.finish().as_bytes());
    put_varint(&mut start, record_count);
    index.append(&start)?;
    let mut top = 0u64;
    while top < u64::from(cfg.height) {
        let end = top.saturating_add(common).min(u64::from(cfg.height));
        let mut batched_role = 0;
        loop {
            let mut next = None;
            let first = if top == 0 { 0 } else { batched_role };
            for (role, cursor) in cursors.iter().enumerate().skip(first) {
                if cursor.stripe == cursor.stripes {
                    continue;
                }
                let sy = if matches!(role, 1 | 2)
                    && cfg.layout.subsampling() == ChromaSubsampling::Yuv420
                {
                    2
                } else {
                    1
                };
                let y = cursor.expected()?[2];
                if y >= end.div_ceil(sy) {
                    continue;
                }
                let key = (y.checked_mul(sy).ok_or(Error::TooLarge)?, role);
                if top != 0 || next.is_none_or(|old| key < old) {
                    next = Some(key);
                }
                if top != 0 {
                    break;
                }
            }
            let Some((_, role)) = next else { break };
            batched_role = role;
            {
                stop.check().map_err(|_| Error::Cancelled)?;
                let expected = cursors[role].expected()?;
                let record =
                    producers[role].record(role as u8, source, &mut numeric_scratch[role], stop)?;
                let mut input = ByteReader::new(record);
                if input.varint()? != expected[0] {
                    return Err(Error::Corrupt("component producer record kind").into());
                }
                let size = input.varint()?;
                let payload = input.bytes_len(size)?;
                if input.remaining() != 0 {
                    return Err(Error::Corrupt("component producer record extent").into());
                }
                let mut association = Vec::with_capacity(32);
                put_varint(&mut association, role as u64);
                put_varint(&mut association, cursors[role].ordinal);
                let mut framing = scalar(u64::MAX - 4);
                put_varint(&mut framing, (association.len() + payload.len()) as u64);
                framing.extend_from_slice(&association);
                let at = sink.at;
                for bytes in [&framing[..], payload] {
                    pieces.update(bytes)?;
                    sink.put(bytes)?;
                }
                let mut entry = Vec::with_capacity(30);
                put_varint(&mut entry, role as u64);
                put_varint(&mut entry, cursors[role].ordinal);
                put_varint(&mut entry, sink.at - at);
                index.append(&entry)?;
                cursors[role].advance();
                sink.flush()?;
            }
        }
        top = end;
    }
    for (role, (p, s)) in producers
        .iter_mut()
        .zip(numeric_scratch.iter_mut())
        .enumerate()
    {
        p.finish(role as u8, source, s, stop)?;
    }
    let data_len = sink.at - data_at;
    let payload = index.len().checked_add(20).ok_or(Error::TooLarge)?;
    let mut index_header = b"uIDX".to_vec();
    put_varint(
        &mut index_header,
        payload.checked_add(1).ok_or(Error::TooLarge)?,
    );
    let index_len = (index_header.len() as u64)
        .checked_add(payload)
        .ok_or(Error::TooLarge)?;
    let mut footer = Vec::with_capacity(20);
    footer.extend_from_slice(&data_len.to_le_bytes());
    footer.extend_from_slice(&index_len.to_le_bytes());
    footer.extend_from_slice(b"uIDX");
    index.append(&footer)?;
    pieces.update(&index_header)?;
    index.replay(|bytes| pieces.update(bytes))?;
    pieces.finish_piece()?;
    pieces.write_table_with(|bytes| sink.put(bytes))?;
    sink.put(&index_header)?;
    index.replay(|bytes| sink.put(bytes))?;
    sink.flush()?;
    Ok((sink.writer, index.into_inner(), pieces.into_scratch()))
}
