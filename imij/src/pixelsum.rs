//! Canonical reconstructed bitmap checksums (SPEC 1.4.1).
use crate::hash::{Digest, HashAlgorithm, Hasher};
use crate::{ByteReader, Error, ImageInfo};
use alloc::{boxed::Box, vec::Vec};

pub(crate) const PSUM: [u8; 4] = *b"pSUM";
pub(crate) const MAX_COUNT: u64 = 1_000_000;
pub(crate) const MAX_PAYLOAD: u64 = 14 + Digest::MAX_RESULT_WIDTH as u64 * MAX_COUNT;
#[cfg(any(feature = "blake3", feature = "sha2"))]
pub(crate) const PREFIX_BYTES: usize = 14 + Digest::MAX_RESULT_WIDTH;
#[cfg(not(any(feature = "blake3", feature = "sha2")))]
pub(crate) const PREFIX_BYTES: usize = 30;
pub(crate) fn width_of(algo: HashAlgorithm) -> usize {
    algo.width()
}
pub(crate) const MISMATCH: Error = Error::Corrupt("pixel checksum mismatch");

/// Verification of decoded pixels, independently of compressed-file checksums.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PixelChecksumStatus {
    /// The file has no decoded bitmap checksum.
    Absent,
    /// The version or algorithm is unknown; no pixels were verified.
    Unsupported,
    /// A supported checksum exists, but the complete image has not been checked.
    Pending,
    /// The complete image or current full animation canvas matched its digest.
    Verified,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Table<'a> {
    Absent,
    Unknown,
    /// The digests and their algorithm.
    Known(&'a [u8], HashAlgorithm),
}

/// Bounded source location of the digest array, learned from the pSUM prefix.
#[derive(Clone, Copy)]
pub(crate) struct LocatedTable {
    pub(crate) first: u64,
    pub(crate) count: u64,
    pub(crate) algo: HashAlgorithm,
}

impl LocatedTable {
    pub(crate) fn expected(
        self,
        ordinal: usize,
        max_bytes: u64,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<(), Error>,
    ) -> Result<Option<Digest>, Error> {
        let ordinal = ordinal as u64;
        if ordinal >= self.count {
            return Err(Error::Corrupt("pSUM ordinal"));
        }
        let width = self.algo.width();
        if width as u64 > max_bytes {
            return Err(Error::TooLarge);
        }
        let at = self
            .first
            .checked_add(ordinal * width as u64)
            .ok_or(Error::TooLarge)?;
        let mut bytes = [0; Digest::MAX_RESULT_WIDTH];
        read(at, &mut bytes[..width])?;
        Table::Known(&bytes[..width], self.algo).expected(0)
    }
}
impl Table<'_> {
    pub(crate) fn status(self) -> PixelChecksumStatus {
        match self {
            Self::Absent => PixelChecksumStatus::Absent,
            Self::Unknown => PixelChecksumStatus::Unsupported,
            Self::Known(..) => PixelChecksumStatus::Pending,
        }
    }
    pub(crate) fn check_count(self, count: u64) -> Result<(), Error> {
        if let Self::Known(d, algo) = self
            && d.len() as u64 / width_of(algo) as u64 != count
        {
            return Err(Error::Corrupt("pSUM image count"));
        }
        Ok(())
    }
    /// The stored digest of image `ordinal`.
    pub(crate) fn expected(self, ordinal: usize) -> Result<Option<Digest>, Error> {
        match self {
            Self::Known(d, algo) => {
                let w = algo.width();
                let at = ordinal
                    .checked_mul(w)
                    .ok_or(Error::Corrupt("pSUM ordinal"))?;
                let bytes = d
                    .get(at..at.checked_add(w).ok_or(Error::Corrupt("pSUM ordinal"))?)
                    .ok_or(Error::Corrupt("pSUM ordinal"))?;
                Ok(Digest::from_bytes(algo, bytes))
            }
            _ => Ok(None),
        }
    }
    pub(crate) fn verify(
        self,
        ordinal: usize,
        info: &ImageInfo,
        pixels: &[u8],
        stride: usize,
    ) -> Result<(), Error> {
        self.verify_checked(ordinal, info, pixels, stride, || Ok(()))
    }
    pub(crate) fn verify_checked(
        self,
        ordinal: usize,
        info: &ImageInfo,
        pixels: &[u8],
        stride: usize,
        mut check: impl FnMut() -> Result<(), Error>,
    ) -> Result<(), Error> {
        if let Some(expected) = self.expected(ordinal)? {
            let got = digest_checked(
                expected.algorithm(),
                info,
                ordinal as u64,
                pixels,
                stride,
                &mut check,
            )?;
            check()?;
            if got != expected {
                return Err(MISMATCH);
            }
        }
        Ok(())
    }
}

/// Validate the small table header against the whole bounded payload length.
/// Returns (count, first digest offset); callers need not retain the table.
pub(crate) fn prefix(
    payload: &[u8],
    length: u64,
) -> Result<Option<(u64, usize, HashAlgorithm)>, Error> {
    let mut r = ByteReader::new(payload);
    let version = r.u8().map_err(|_| Error::Corrupt("empty pSUM"))?;
    if version != 1 {
        return Ok(None);
    }
    let Some(algo) = HashAlgorithm::from_wire(r.u8().map_err(|_| Error::Corrupt("short pSUM"))?)
    else {
        return Ok(None);
    };
    let width = algo.width();
    if r.u8().map_err(|_| Error::Corrupt("short pSUM"))? as usize != width
        || r.varint().map_err(|_| Error::Corrupt("short pSUM"))? != 0
    {
        return Err(Error::Corrupt("pSUM digest format"));
    }
    let count = r.varint().map_err(|_| Error::Corrupt("pSUM image count"))?;
    if count == 0 || count > MAX_COUNT || length != r.pos as u64 + count * width as u64 {
        return Err(Error::Corrupt("pSUM image count"));
    }
    Ok(Some((count, r.pos, algo)))
}
pub(crate) fn parse(payload: &[u8]) -> Result<Table<'_>, Error> {
    match prefix(payload, payload.len() as u64)? {
        Some((_, offset, algo)) => Ok(Table::Known(&payload[offset..], algo)),
        None => Ok(Table::Unknown),
    }
}

pub(crate) fn image_count(anim: Option<&[u8]>) -> Result<u64, Error> {
    let Some(anim) = anim else {
        return Ok(1);
    };
    let mut r = ByteReader::new(anim);
    r.u8()?;
    r.varint()?; // loop count
    let count = r.varint()?;
    if count == 0 || count > MAX_COUNT {
        return Err(Error::Corrupt("pSUM image count"));
    }
    Ok(count)
}

fn descriptor(info: &ImageInfo, ordinal: u64) -> [u8; 34] {
    let mut bytes = [0; 34];
    bytes[..16].copy_from_slice(b"IMIJ/pSUM/v1\0\0\0\0");
    bytes[16..20].copy_from_slice(&info.width.to_le_bytes());
    bytes[20..24].copy_from_slice(&info.height.to_le_bytes());
    bytes[24] = info.channels as u8;
    bytes[25] = info.sample as u8;
    bytes[26..].copy_from_slice(&ordinal.to_le_bytes());
    bytes
}

pub(crate) struct Raster {
    hash: Hasher,
    width: usize,
    sample_bytes: usize,
    height: u32,
    rows: u32,
}
impl Raster {
    pub(crate) fn new(algo: HashAlgorithm, info: &ImageInfo, ordinal: u64) -> Result<Self, Error> {
        let width = checked_row_bytes(info.width as usize, info.bytes_per_pixel())?;
        let mut hash = Hasher::new(algo);
        hash.update(&descriptor(info, ordinal));
        Ok(Self {
            hash,
            width,
            sample_bytes: info.sample.bytes(),
            height: info.height,
            rows: 0,
        })
    }
    pub(crate) fn row(&mut self, row: &[u8]) -> Result<(), Error> {
        self.row_checked(row, &mut || Ok(()))
    }
    fn row_checked(
        &mut self,
        row: &[u8],
        check: &mut impl FnMut() -> Result<(), Error>,
    ) -> Result<(), Error> {
        if row.len() < self.width || self.rows >= self.height {
            return Err(Error::Corrupt("pSUM raster rows"));
        }
        // A bounded byte quantum also checks cancellation inside very wide rows.
        // It is aligned to every storage sample width, without float loads.
        for block in row[..self.width].chunks(64 * 1024) {
            check()?;
            canonical_update(
                &mut self.hash,
                block,
                self.sample_bytes,
                cfg!(target_endian = "big"),
            );
        }
        self.rows += 1;
        check()
    }
    pub(crate) fn finish(&self) -> Result<Digest, Error> {
        if self.rows != self.height {
            return Err(Error::Corrupt("pSUM incomplete raster"));
        }
        Ok(self.hash.finish())
    }
}
fn checked_row_bytes(width: usize, pixel_bytes: usize) -> Result<usize, Error> {
    width.checked_mul(pixel_bytes).ok_or(Error::TooLarge)
}

pub(crate) fn canonical_update(hash: &mut Hasher, row: &[u8], sample_bytes: usize, big: bool) {
    if !big || sample_bytes == 1 {
        hash.update(row);
        return;
    }
    // Byte operations alone preserve signaling NaNs and their payloads.
    let mut block = [0u8; 1024];
    for source in row.chunks(1024) {
        for (sample, dest) in source
            .chunks_exact(sample_bytes)
            .zip(block.chunks_exact_mut(sample_bytes))
        {
            for (a, b) in sample.iter().rev().zip(dest.iter_mut()) {
                *b = *a;
            }
        }
        hash.update(&block[..source.len()]);
    }
}
pub(crate) fn hash_work(info: &ImageInfo) -> Result<u128, Error> {
    (info.width as u128)
        .checked_mul(info.bytes_per_pixel() as u128)
        .and_then(|row| row.checked_mul(info.height as u128))
        .and_then(|bytes| bytes.checked_add(34))
        .ok_or(Error::TooLarge)
}
/// The raster digest under the default algorithm.
#[cfg(all(test, feature = "encode"))]
pub(crate) fn digest(
    info: &ImageInfo,
    ordinal: u64,
    pixels: &[u8],
    stride: usize,
) -> Result<Digest, Error> {
    digest_checked(
        HashAlgorithm::DEFAULT,
        info,
        ordinal,
        pixels,
        stride,
        &mut || Ok(()),
    )
}
/// The raster digest under `algo`.
pub(crate) fn digest_with(
    algo: HashAlgorithm,
    info: &ImageInfo,
    ordinal: u64,
    pixels: &[u8],
    stride: usize,
) -> Result<Digest, Error> {
    digest_checked(algo, info, ordinal, pixels, stride, &mut || Ok(()))
}
pub(crate) fn digest_checked(
    algo: HashAlgorithm,
    info: &ImageInfo,
    ordinal: u64,
    pixels: &[u8],
    stride: usize,
    check: &mut impl FnMut() -> Result<(), Error>,
) -> Result<Digest, Error> {
    check()?;
    let extent = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("invalid pSUM output layout"))?;
    if pixels.len() < extent {
        return Err(Error::InvalidInput("pSUM output buffer too small"));
    }
    let mut raster = Raster::new(algo, info, ordinal)?;
    for row in pixels.chunks(stride).take(info.height as usize) {
        raster.row_checked(row, check)?;
    }
    check()?;
    raster.finish()
}

/// A row checksum has a sticky failure: retrying finalization cannot return EOF.
pub(crate) struct RowCheck {
    raster: Box<Raster>,
    expected: Digest,
    result: Option<Result<(), Error>>,
}
impl RowCheck {
    pub(crate) fn new(table: Table<'_>, info: &ImageInfo) -> Result<Option<Self>, Error> {
        let Some(expected) = table.expected(0)? else {
            return Ok(None);
        };
        Self::for_digest(expected, info, 0).map(Some)
    }
    pub(crate) fn for_digest(
        expected: Digest,
        info: &ImageInfo,
        ordinal: u64,
    ) -> Result<Self, Error> {
        let raster = Raster::new(expected.algorithm(), info, ordinal)?;
        Ok(Self {
            raster: Box::new(raster),
            expected,
            result: None,
        })
    }
    pub(crate) fn before(&self) -> Result<(), Error> {
        self.result.clone().unwrap_or(Ok(()))
    }
    pub(crate) fn row(&mut self, row: &[u8]) -> Result<(), Error> {
        self.row_checked(row, || Ok(()))
    }
    pub(crate) fn row_checked(
        &mut self,
        row: &[u8],
        mut check: impl FnMut() -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.before()?;
        self.raster.row_checked(row, &mut check)?;
        if self.raster.rows == self.raster.height {
            let result = if self.raster.finish()? == self.expected {
                Ok(())
            } else {
                Err(MISMATCH)
            };
            check()?;
            self.result = Some(result.clone());
            result?;
        }
        Ok(())
    }
    pub(crate) fn status(&self) -> PixelChecksumStatus {
        if self.result == Some(Ok(())) {
            PixelChecksumStatus::Verified
        } else {
            PixelChecksumStatus::Pending
        }
    }
}

/// Explicitly adds a decoded bitmap checksum to a complete encoded file.
///
/// Encoder defaults remain unchanged. Lossy and near-lossless checksums use
/// reconstructed output. Animations attest every full codec canvas, including
/// empty frames; pages attest their own full images. Call separately on preview
/// and gain-map files before embedding them to attest those nested images.
/// Standalone inter-frame updates need a reference and are rejected here.
/// Existing pixel checksums and indexed plane sets are refused. A known file
/// checksum is verified and recomputed; an unknown file checksum is refused.
/// Reconstruction uses row storage for stills and a reusable frame/page buffer.
/// Adds a `pSUM` table under the default algorithm; see
/// [`add_pixel_checksum_with`].
pub fn add_pixel_checksum(file: Vec<u8>) -> Result<Vec<u8>, Error> {
    add_pixel_checksum_with(file, HashAlgorithm::DEFAULT)
}

/// Adds a `pSUM` table of `algo` digests of every decoded image to a complete
/// file without one.
pub fn add_pixel_checksum_with(mut file: Vec<u8>, algo: HashAlgorithm) -> Result<Vec<u8>, Error> {
    let session = crate::decode_resources::DecodeSession::new(
        crate::decode_resources::DecodePolicy::default().with_output_limits(crate::Limits::NONE),
    );
    let v4 = crate::parse::header::header_version(&file)? == 4;
    let f = if v4 {
        crate::v4::file::parse_file(&file, &session)?
    } else {
        crate::v1::parse_file(
            &file,
            &crate::parse::limits::Unrestricted,
            crate::checksum::Verify::Now(1),
        )?
    };
    if !matches!(f.pixelsum, Table::Absent) {
        return Err(Error::InvalidInput("file already has pSUM"));
    }
    let csum = crate::container::ChecksumChunk::rewritable(f.checksum)?;
    let count = image_count(f.anim)?;
    // Plane offsets cannot survive insertion into the primary.
    let mut r = ByteReader::new(&file);
    crate::parse::header::read_header_sized(&mut r)?;
    while r.remaining() != 0 {
        let (tag, len) = crate::parse::chunks::read_head(&mut r)?;
        if crate::parse::chunks::is_plane_chunk(&tag) {
            return Err(Error::Unsupported(
                "pSUM insertion requires rebuilding plane indexes",
            ));
        }
        let n = match len {
            Some(n) => usize::try_from(n).map_err(|_| Error::TooLarge)?,
            None if v4 => f.data.len(),
            None => crate::v1::payload_len(&f.header, f.declared, &file[r.pos..])?,
        };
        r.bytes(n)?;
    }
    let mut payload = Vec::new();
    let capacity =
        usize::try_from(count * algo.width() as u64 + 14).map_err(|_| Error::TooLarge)?;
    payload
        .try_reserve_exact(capacity)
        .map_err(|_| Error::TooLarge)?;
    payload.extend_from_slice(&[1, algo.wire(), algo.width() as u8, 0]);
    crate::put_varint(&mut payload, count);
    if f.anim.is_none() {
        let mut decoder = crate::StreamDecoder::new(&file)?;
        let mut raster = Raster::new(algo, decoder.info(), 0)?;
        let mut row = Vec::new();
        row.try_reserve_exact(raster.width)
            .map_err(|_| Error::TooLarge)?;
        row.resize(raster.width, 0);
        while decoder.next_row(&mut row)? {
            raster.row(&row)?;
        }
        payload.extend_from_slice(raster.finish()?.as_bytes());
    } else {
        let mut decoder = crate::FrameDecoder::new(&file)?;
        let mut last_use = alloc::collections::BTreeMap::new();
        let unified = f.anim.is_some_and(|a| a.first() == Some(&4));
        if unified {
            while let Some(frame) = decoder.next_frame()? {
                if let Some(reference) = decoder.reference_index() {
                    last_use.insert(reference, frame.index);
                }
            }
            decoder = crate::FrameDecoder::new(&file)?;
        }
        let mut references: alloc::collections::BTreeMap<usize, Vec<u8>> =
            alloc::collections::BTreeMap::new();
        let mut pixels = Vec::new();
        while let Some(frame) = decoder.next_frame()? {
            let info = if decoder.kind() == crate::FrameKind::Animation {
                decoder.info().clone()
            } else {
                decoder
                    .frame_info()
                    .ok_or(Error::Corrupt("pSUM page info"))?
            };
            let size = info.checked_buffer_len().ok_or(Error::TooLarge)?;
            if size > pixels.len() {
                pixels
                    .try_reserve_exact(size - pixels.len())
                    .map_err(|_| Error::TooLarge)?;
            }
            pixels.resize(size, 0);
            if unified && let Some(reference) = decoder.reference_index() {
                let raster = references
                    .get(&reference)
                    .ok_or(Error::Corrupt("missing checksum reference"))?;
                decoder.decode_into_with_reference(
                    raster,
                    info.row_bytes(),
                    &mut pixels,
                    info.row_bytes(),
                    1,
                )?;
                if last_use.get(&reference) == Some(&frame.index) {
                    references.remove(&reference);
                }
            } else {
                decoder.decode_into(&mut pixels, info.row_bytes(), 1)?;
            }
            if last_use.contains_key(&frame.index) {
                let mut retained = Vec::new();
                retained
                    .try_reserve_exact(size)
                    .map_err(|_| Error::TooLarge)?;
                retained.extend_from_slice(&pixels);
                references.insert(frame.index, retained);
            }
            payload.extend_from_slice(
                digest_with(algo, &info, frame.index as u64, &pixels, info.row_bytes())?.as_bytes(),
            );
        }
    }
    let checksum_range = f.checksum.map(|c| c.at..c.at + c.len);
    if let Some(range) = checksum_range {
        file.drain(range);
    }
    let at = crate::container::data_at(&file)?;
    let mut chunk = Vec::new();
    chunk
        .try_reserve_exact(payload.len() + 14)
        .map_err(|_| Error::TooLarge)?;
    crate::container::put_chunk(&mut chunk, &PSUM, &payload);
    file.try_reserve_exact(chunk.len())
        .map_err(|_| Error::TooLarge)?;
    file.splice(at..at, chunk);
    let file = crate::container::locate(file)?;
    if csum {
        crate::checksum::refinish(file)
    } else {
        Ok(file)
    }
}
#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, SampleType};

    #[test]
    fn row_size_is_checked_before_hash_state_construction() {
        for pixel_bytes in [1, 2, 3, 4, 8, 12, 16, 24, 32] {
            let width = usize::MAX / pixel_bytes;
            assert_eq!(
                checked_row_bytes(width, pixel_bytes),
                Ok(width * pixel_bytes)
            );
            if pixel_bytes > 1 {
                assert_eq!(
                    checked_row_bytes(width + 1, pixel_bytes),
                    Err(Error::TooLarge)
                );
            }
        }
        for channels in [
            Channels::Gray,
            Channels::GrayAlpha,
            Channels::Rgb,
            Channels::Rgba,
        ] {
            for sample in [
                SampleType::U8,
                SampleType::U16,
                SampleType::U32,
                SampleType::F64,
            ] {
                let info = ImageInfo::new(u32::MAX, 1, channels, sample);
                let expected = (u32::MAX as usize).checked_mul(info.bytes_per_pixel());
                let raster = Raster::new(HashAlgorithm::DEFAULT, &info, 0);
                assert_eq!(
                    raster.as_ref().map(|r| r.width).map_err(Clone::clone),
                    expected.ok_or(Error::TooLarge)
                );
                let digest = 0u64.to_le_bytes();
                assert_eq!(
                    RowCheck::new(Table::Known(&digest, HashAlgorithm::Xxh3_64), &info).map(|_| ()),
                    expected.map(|_| ()).ok_or(Error::TooLarge)
                );
                assert!(RowCheck::new(Table::Absent, &info).unwrap().is_none());
                assert!(RowCheck::new(Table::Unknown, &info).unwrap().is_none());
            }
        }
    }

    #[test]
    fn verification_checks_cancellation_within_wide_rows_and_before_success() {
        let info = ImageInfo::new(65537, 2, Channels::Gray, SampleType::U8);
        let pixels: Vec<_> = (0..info.checked_buffer_len().unwrap())
            .map(|i| (i * 37) as u8)
            .collect();
        let canonical = [&descriptor(&info, 0)[..], &pixels].concat();
        let expected = xxhash_rust::xxh3::xxh3_64(&canonical).to_le_bytes();
        let table = Table::Known(&expected, HashAlgorithm::Xxh3_64);
        let mut checks = 0;
        table
            .verify_checked(0, &info, &pixels, info.row_bytes(), || {
                checks += 1;
                Ok(())
            })
            .unwrap();
        // Initial + two blocks and a row boundary per row + two final checks.
        assert_eq!(checks, 9);
        for stop in [1, 3, checks] {
            let mut calls = 0;
            assert_eq!(
                table.verify_checked(0, &info, &pixels, info.row_bytes(), || {
                    calls += 1;
                    if calls == stop {
                        Err(Error::Cancelled)
                    } else {
                        Ok(())
                    }
                }),
                Err(Error::Cancelled)
            );
            assert_eq!(calls, stop);
        }
    }

    fn unhex(hex: &str) -> Vec<u8> {
        hex.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(core::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect()
    }
    // Independent fixed vectors: xxHash C 0.8.3, XXH3_64bits, seed 0,
    // default secret; generated 2026-10-11 from the SPEC 1.4.1 descriptor.
    // Algorithm reference: https://github.com/Cyan4973/xxHash
    const VECTORS: &[(&str, &str, u64)] = &[
        (
            "gray-u8-zero",
            "494d494a2f7053554d2f76310000000001000000010000000100000000000000000000",
            0x5278d81d924584da,
        ),
        (
            "gray-u8-full",
            "494d494a2f7053554d2f763100000000010000000100000001000000000000000000ff",
            0xed54542d097ec142,
        ),
        (
            "ga-u16-bits",
            "494d494a2f7053554d2f763100000000020000000100000002010000000000000000ffff00000080ffff",
            0x0ce75f99e5621295,
        ),
        (
            "rgba-f16-specials",
            "494d494a2f7053554d2f7631000000000100000002000000040200000000000000000080007c017e003c010000fcff7f0000",
            0x7924c1623f04d7f2,
        ),
        (
            "gray-f32-specials",
            "494d494a2f7053554d2f763100000000020000000100000001050000000000000000000000803412c07f",
            0x843f4b69c6a77359,
        ),
        (
            "gray-f64-specials",
            "494d494a2f7053554d2f7631000000000100000002000000010a0000000000000000341200000000f87f000000000000f0ff",
            0xe16d183fc52a4ddb,
        ),
        (
            "gray-i32-min",
            "494d494a2f7053554d2f76310000000001000000010000000104000000000000000000000080",
            0x77fa7232cd00793a,
        ),
        (
            "gray-u8-ordinal1",
            "494d494a2f7053554d2f76310000000001000000010000000100010000000000000000",
            0x83f7814f74758b4d,
        ),
        (
            "gray-u8-geometry2x1",
            "494d494a2f7053554d2f7631000000000200000001000000010000000000000000000102",
            0x22d4ded48b3c49ed,
        ),
        (
            "gray-u8-geometry1x2",
            "494d494a2f7053554d2f7631000000000100000002000000010000000000000000000102",
            0x8959a5424e389f34,
        ),
    ];
    #[test]
    fn canonical_vectors_match_independent_c_and_rust() {
        for &(name, hex, expected) in VECTORS {
            let bytes = unhex(hex);
            let info = ImageInfo::new(
                u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
                u32::from_le_bytes(bytes[20..24].try_into().unwrap()),
                Channels::from_u8(bytes[24]).unwrap(),
                SampleType::from_u8(bytes[25]).unwrap(),
            );
            let ordinal = u64::from_le_bytes(bytes[26..34].try_into().unwrap());
            assert_eq!(&descriptor(&info, ordinal), &bytes[..34], "{name}");
            assert_eq!(
                xxhash_rust::xxh3::xxh3_64(&bytes),
                expected,
                "independent rust {name}"
            );
            for split in 1..=bytes.len() {
                let mut hash = zenhash::Xxh3::new();
                for piece in bytes.chunks(split) {
                    hash.update(piece);
                }
                assert_eq!(hash.digest(), expected, "fragment {split} {name}");
            }
            let bps = info.sample.bytes();
            let native: Vec<u8> = bytes[34..]
                .chunks_exact(bps)
                .flat_map(|sample| {
                    if cfg!(target_endian = "big") {
                        sample.iter().rev().copied().collect::<Vec<_>>()
                    } else {
                        sample.to_vec()
                    }
                })
                .collect();
            assert_eq!(
                digest_with(
                    HashAlgorithm::Xxh3_64,
                    &info,
                    ordinal,
                    &native,
                    info.row_bytes()
                )
                .unwrap()
                .low_u64(),
                expected,
                "canonical {name}"
            );
            let stride = info.row_bytes() + 13;
            let mut padded = alloc::vec![0xa5; stride * info.height as usize];
            for (src, dst) in native
                .chunks(info.row_bytes())
                .zip(padded.chunks_mut(stride))
            {
                dst[..src.len()].copy_from_slice(src);
            }
            assert_eq!(
                digest_with(HashAlgorithm::Xxh3_64, &info, ordinal, &padded, stride)
                    .unwrap()
                    .low_u64(),
                expected,
                "stride {name}"
            );
            let be: Vec<u8> = bytes[34..]
                .chunks_exact(bps)
                .flat_map(|p| p.iter().rev().copied())
                .collect();
            let mut hash = Hasher::new(HashAlgorithm::Xxh3_64);
            hash.update(&descriptor(&info, ordinal));
            canonical_update(&mut hash, &be, bps, true);
            assert_eq!(hash.finish().low_u64(), expected, "big endian {name}");
        }
    }

    #[test]
    fn large_canonical_rows_match_reference_at_every_storage_width() {
        for sample in [
            SampleType::U8,
            SampleType::I8,
            SampleType::U16,
            SampleType::I16,
            SampleType::F16,
            SampleType::U32,
            SampleType::I32,
            SampleType::F32,
            SampleType::U64,
            SampleType::I64,
            SampleType::F64,
        ] {
            for channels in [
                Channels::Gray,
                Channels::GrayAlpha,
                Channels::Rgb,
                Channels::Rgba,
            ] {
                for width in [1, 15, 16, 17, 255, 256, 257] {
                    let info = ImageInfo::new(width, 3, channels, sample);
                    let pixels: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                        .map(|i| (i.wrapping_mul(79) ^ (i >> 8)) as u8)
                        .collect();
                    let mut input = descriptor(&info, 0).to_vec();
                    let canonical: Vec<u8> = pixels
                        .chunks_exact(sample.bytes())
                        .flat_map(|p| {
                            if cfg!(target_endian = "big") {
                                p.iter().rev().copied().collect::<Vec<_>>()
                            } else {
                                p.to_vec()
                            }
                        })
                        .collect();
                    input.extend_from_slice(&canonical);
                    assert_eq!(
                        digest_with(HashAlgorithm::Xxh3_64, &info, 0, &pixels, info.row_bytes())
                            .unwrap()
                            .low_u64(),
                        xxhash_rust::xxh3::xxh3_64(&input)
                    );
                    assert_eq!(
                        digest(&info, 0, &pixels, info.row_bytes())
                            .unwrap()
                            .as_bytes(),
                        xxhash_rust::xxh3::xxh3_128(&input).to_le_bytes()
                    );
                    let be: Vec<u8> = canonical
                        .chunks_exact(sample.bytes())
                        .flat_map(|p| p.iter().rev().copied())
                        .collect();
                    let mut hash = Hasher::new(HashAlgorithm::Xxh3_64);
                    hash.update(&descriptor(&info, 0));
                    canonical_update(&mut hash, &be, sample.bytes(), true);
                    assert_eq!(hash.finish().low_u64(), xxhash_rust::xxh3::xxh3_64(&input));
                }
            }
        }
    }

    #[test]
    fn unchanged_encoded_file_rejects_deliberately_wrong_result() {
        let info = ImageInfo::new(17, 3, Channels::Gray, SampleType::U8);
        let pixels: Vec<_> = (0..51).collect();
        let file = crate::encode(
            &pixels,
            17,
            &info,
            &crate::Metadata::default(),
            &crate::EncodeConfig::default(),
        )
        .unwrap();
        let file = add_pixel_checksum(file).unwrap();
        let before = zenhash::xxh3_64(&file);
        let parsed = crate::v1::parse_file(
            &file,
            &crate::parse::limits::Unrestricted,
            crate::checksum::Verify::Now(1),
        )
        .unwrap();
        let (_, mut decoded) = crate::decode(&file).unwrap();
        decoded[17] ^= 1;
        assert_eq!(
            parsed.pixelsum.verify(0, &info, &decoded, 17),
            Err(MISMATCH)
        );
        assert_eq!(zenhash::xxh3_64(&file), before);
        assert_eq!(crate::decode(&file).unwrap().1, pixels);
    }
}
