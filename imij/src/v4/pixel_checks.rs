//! Canonical group/tile sample checks, shared by buffered and row-fed paths.

use super::{CheckList, CheckScope};
use crate::decode_resources::{DecodeSession, V4Retained};
use crate::{Error, Hasher, ImageInfo, ImageRef};
use alloc::vec::Vec;

/// Logical identity of a local reconstructed group or tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelCheckIdentity {
    pub image: u64,
    pub id: u64,
    pub x: u32,
    pub y: u32,
}

/// Completed digest comparisons. Unsupported suites are never verified.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckCoverage {
    pub verified: usize,
    pub unsupported: usize,
    pub skipped: usize,
}

/// One native raster's checks. Feed complete visible rows in raster order;
/// buffers may be strided and group workers may finish in any order before
/// this traversal. Native floating values are hashed as bits, without loads.
/// A zero declared height allows a forward writer to discover the final height.
pub struct PixelChecks<'a> {
    list: CheckList<'a>,
    hashes: [Option<Hasher>; 2],
    #[cfg(feature = "blake3")]
    blake3_hashes: Vec<Hasher>,
    info: ImageInfo,
    rows: u32,
    finalized: bool,
    failed: bool,
}

pub(crate) type RetainedPixelCheckState = (PixelCheckState, V4Retained);

#[cfg(all(test, feature = "blake3"))]
#[test]
fn live_tile_check_storage_is_charged_across_suspension() {
    use super::{CheckDescriptor, CheckLimits, Checks};
    use crate::{Channels, SampleType, decode_resources::DecodePolicy};

    let descriptor = CheckDescriptor {
        suite_id: 2,
        result_bytes: 32,
        parameters: &[],
    };
    let mut declaration = Vec::new();
    Checks::append(
        &mut declaration,
        [&[], &[descriptor], &[]],
        CheckLimits::default(),
    )
    .unwrap();
    let (checks, _) = Checks::read(&declaration, CheckLimits::default()).unwrap();
    let list = checks.list(CheckScope::TilePixels);
    let limit = PixelChecks::heap_bytes(list);
    assert!(limit > 0);
    let session = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(limit as u64));
    let info = ImageInfo::new(1, 2, Channels::Gray, SampleType::U8);
    let identity = PixelCheckIdentity {
        image: 0,
        id: 0,
        x: 0,
        y: 0,
    };
    let (checker, charge) = PixelChecks::tile_state(list, None, identity, &info, &session).unwrap();
    let resumed = session.v4_resume().session(|| false);
    assert!(matches!(
        PixelChecks::tile_state(list, None, identity, &info, &resumed),
        Err(Error::TooLarge)
    ));
    let (checker, charge) = PixelChecks::tile_state(
        list,
        Some((checker.into_state(), charge)),
        identity,
        &info,
        &resumed,
    )
    .unwrap();
    drop((checker, charge));
    PixelChecks::tile_state(list, None, identity, &info, &resumed).unwrap();
}

impl<'a> PixelChecks<'a> {
    pub(crate) const SCRATCH_BYTES: usize = 148;

    pub(crate) fn working_bytes(list: CheckList<'_>) -> usize {
        core::mem::size_of::<Self>() + Self::SCRATCH_BYTES + Self::heap_bytes(list)
    }

    fn heap_bytes(_list: CheckList<'_>) -> usize {
        #[cfg(feature = "blake3")]
        {
            let extra = _list
                .iter()
                .filter(|descriptor| descriptor.algorithm().is_some_and(|a| a.wire() > 1))
                .count();
            extra * (core::mem::size_of::<Hasher>() + core::mem::size_of::<blake3::Hasher>())
        }
        #[cfg(not(feature = "blake3"))]
        0
    }

    pub(crate) fn tile_state<C: Fn() -> bool + Sync>(
        list: CheckList<'a>,
        state: Option<RetainedPixelCheckState>,
        identity: PixelCheckIdentity,
        info: &ImageInfo,
        session: &DecodeSession<C>,
    ) -> Result<(Self, V4Retained), Error> {
        if let Some((state, charge)) = state {
            return Ok((Self::from_state(list, state), charge));
        }
        let charge = session
            .v4_working(Self::heap_bytes(list) as u128)?
            .retain()?;
        let hash = Self::new(list, CheckScope::TilePixels, identity, info)?;
        Ok((hash, charge))
    }

    pub fn new(
        list: CheckList<'a>,
        scope: CheckScope,
        identity: PixelCheckIdentity,
        info: &ImageInfo,
    ) -> Result<Self, Error> {
        if info.width == 0 || scope == CheckScope::EncodedObject {
            return Err(Error::InvalidInput("V4 pixel check declaration"));
        }
        info.validate_precision()?;
        let mut prefix = Vec::new();
        prefix.try_reserve_exact(128).map_err(|_| Error::TooLarge)?;
        prefix.extend_from_slice(match scope {
            CheckScope::GroupPixels => b"IMI/v4/pixel-group\0",
            CheckScope::TilePixels => b"IMI/v4/pixel-tile\0",
            CheckScope::EncodedObject => unreachable!(),
        });
        for value in [
            identity.image,
            identity.id,
            u64::from(identity.x),
            u64::from(identity.y),
            u64::from(info.width),
            info.channels as u64,
            info.sample as u64,
        ] {
            crate::put_varint(&mut prefix, value);
        }
        let mut hashes: [Option<Hasher>; 2] = [None, None];
        #[cfg(feature = "blake3")]
        let mut blake3_hashes: Vec<Hasher> = Vec::new();
        for descriptor in list.iter() {
            if let Some(algorithm) = descriptor.algorithm() {
                if let Some(slot) = hashes.get_mut(algorithm.wire() as usize) {
                    let mut hash = Hasher::new(algorithm);
                    hash.update(&prefix);
                    *slot = Some(hash);
                }
                #[cfg(feature = "blake3")]
                if algorithm.wire() > 1
                    && !blake3_hashes
                        .iter()
                        .any(|hash| hash.algorithm() == algorithm)
                {
                    let mut hash = Hasher::new(algorithm);
                    hash.update(&prefix);
                    blake3_hashes
                        .try_reserve_exact(1)
                        .map_err(|_| Error::TooLarge)?;
                    blake3_hashes.push(hash);
                }
            }
        }
        Ok(Self {
            list,
            hashes,
            #[cfg(feature = "blake3")]
            blake3_hashes,
            info: info.clone(),
            rows: 0,
            finalized: false,
            failed: false,
        })
    }

    fn hash_for(&self, algorithm: crate::HashAlgorithm) -> &Hasher {
        #[cfg(feature = "blake3")]
        if algorithm.wire() > 1 {
            return self
                .blake3_hashes
                .iter()
                .find(|h| h.algorithm() == algorithm)
                .unwrap();
        }
        self.hashes[algorithm.wire() as usize].as_ref().unwrap()
    }

    /// Feed strided native rows, excluding all padding. The last row need not
    /// contain trailing padding. Errors poison this state permanently.
    pub fn rows(&mut self, pixels: ImageRef<'_>) -> Result<(), Error> {
        self.rows_checked(pixels, || Ok(()))
    }

    /// Like rows(), checking cancellation at each 64-KiB sample boundary.
    pub fn rows_checked(
        &mut self,
        pixels: ImageRef<'_>,
        mut check: impl FnMut() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let result = self.rows_inner(pixels, &mut check);
        self.failed |= result.is_err();
        result
    }

    fn rows_inner(
        &mut self,
        pixels: ImageRef<'_>,
        check: &mut impl FnMut() -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.failed || self.finalized {
            return Err(Error::InvalidInput("completed or failed V4 pixel checks"));
        }
        let info = pixels.info;
        if info.width != self.info.width
            || info.channels != self.info.channels
            || info.sample != self.info.sample
            || info.height == 0
        {
            return Err(Error::InvalidInput("V4 pixel check row interpretation"));
        }
        let rows = self.rows.checked_add(info.height).ok_or(Error::TooLarge)?;
        if self.info.height != 0 && rows > self.info.height {
            return Err(Error::Corrupt("too many V4 pixel check rows"));
        }
        let row = (info.width as usize)
            .checked_mul(info.bytes_per_pixel())
            .ok_or(Error::TooLarge)?;
        let extent = info
            .checked_stride_len(pixels.stride)
            .ok_or(Error::InvalidInput("V4 pixel check stride"))?;
        if pixels.pixels.len() < extent {
            return Err(Error::InvalidInput("short V4 pixel check raster"));
        }
        for y in 0..info.height as usize {
            let at = y.checked_mul(pixels.stride).ok_or(Error::TooLarge)?;
            for block in pixels.pixels[at..at + row].chunks(64 << 10) {
                check()?;
                for hash in self.hashes.iter_mut().flatten() {
                    crate::pixelsum::canonical_update(
                        hash,
                        block,
                        info.sample.bytes(),
                        cfg!(target_endian = "big"),
                    );
                }
                #[cfg(feature = "blake3")]
                for hash in &mut self.blake3_hashes {
                    crate::pixelsum::canonical_update(
                        hash,
                        block,
                        info.sample.bytes(),
                        cfg!(target_endian = "big"),
                    );
                }
            }
            self.rows += 1;
            check()?;
        }
        Ok(())
    }

    fn finalize(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Corrupt("failed V4 pixel checks"));
        }
        if self.finalized {
            return Ok(());
        }
        if self.rows == 0 || self.info.height != 0 && self.rows != self.info.height {
            self.failed = true;
            return Err(Error::Corrupt("incomplete V4 pixel checks"));
        }
        let bytes = u64::from(self.info.width)
            .checked_mul(self.info.bytes_per_pixel() as u64)
            .and_then(|n| n.checked_mul(u64::from(self.rows)))
            .ok_or(Error::TooLarge)?;
        let mut suffix = Vec::new();
        suffix.try_reserve_exact(20).map_err(|_| Error::TooLarge)?;
        crate::put_varint(&mut suffix, u64::from(self.rows));
        crate::put_varint(&mut suffix, bytes);
        for hash in self.hashes.iter_mut().flatten() {
            hash.update(&suffix);
        }
        #[cfg(feature = "blake3")]
        for hash in &mut self.blake3_hashes {
            hash.update(&suffix);
        }
        self.finalized = true;
        Ok(())
    }

    /// Return results for built-in suites. Opaque suites need an external
    /// implementation; this method never invents placeholder signature bytes.
    pub fn results(&mut self) -> Result<Vec<u8>, Error> {
        self.finalize()?;
        if self.list.iter().any(|d| d.algorithm().is_none()) {
            return Err(Error::Unsupported("V4 pixel check suite"));
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.list.result_bytes())
            .map_err(|_| Error::TooLarge)?;
        for descriptor in self.list.iter() {
            let algorithm = descriptor.algorithm().unwrap();
            bytes.extend_from_slice(self.hash_for(algorithm).finish().as_bytes());
        }
        Ok(bytes)
    }

    /// Verify every supported result and explicitly count unsupported suites.
    /// A mismatch is sticky even if a caller retries with different results.
    pub fn verify(&mut self, expected: &[u8]) -> Result<CheckCoverage, Error> {
        let result = (|| {
            self.finalize()?;
            let mut coverage = CheckCoverage::default();
            for result in self.list.results(expected)? {
                if let Some(algorithm) = result.check.algorithm() {
                    if self.hash_for(algorithm).finish().as_bytes() != result.bytes {
                        return Err(Error::Corrupt("V4 pixel check mismatch"));
                    }
                    coverage.verified += 1;
                } else {
                    coverage.unsupported += 1;
                }
            }
            Ok(coverage)
        })();
        self.failed |= result.is_err();
        result
    }
}

/// Move incremental hash state between admitted row bands without borrowing a
/// descriptor stored in the same owner. The immutable list is rebound each time.
pub(crate) struct PixelCheckState {
    hashes: [Option<Hasher>; 2],
    #[cfg(feature = "blake3")]
    blake3_hashes: Vec<Hasher>,
    info: ImageInfo,
    rows: u32,
    finalized: bool,
    failed: bool,
}
impl<'a> PixelChecks<'a> {
    pub(crate) fn into_state(self) -> PixelCheckState {
        PixelCheckState {
            hashes: self.hashes,
            #[cfg(feature = "blake3")]
            blake3_hashes: self.blake3_hashes,
            info: self.info,
            rows: self.rows,
            finalized: self.finalized,
            failed: self.failed,
        }
    }
    pub(crate) fn from_state(list: CheckList<'a>, state: PixelCheckState) -> Self {
        Self {
            list,
            hashes: state.hashes,
            #[cfg(feature = "blake3")]
            blake3_hashes: state.blake3_hashes,
            info: state.info,
            rows: state.rows,
            finalized: state.finalized,
            failed: state.failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v4::{CheckDescriptor, CheckLimits, Checks};
    use crate::{Channels, HashAlgorithm, SampleType};
    fn declaration() -> Vec<u8> {
        let descriptors = [
            CheckDescriptor::xxh3(HashAlgorithm::Xxh3_64),
            CheckDescriptor::xxh3(HashAlgorithm::Xxh3_128),
        ];
        let mut bytes = Vec::new();
        Checks::append(
            &mut bytes,
            [&descriptors, &descriptors, &[]],
            CheckLimits::default(),
        )
        .unwrap();
        bytes
    }
    const ID: PixelCheckIdentity = PixelCheckIdentity {
        image: 0,
        id: 7,
        x: 2,
        y: 3,
    };

    #[test]
    fn strided_and_incremental_rows_match_packed_samples_and_independent_oracle() {
        let declaration = declaration();
        let (checks, _) = Checks::read(&declaration, CheckLimits::default()).unwrap();
        let info = ImageInfo::new(2, 2, Channels::Gray, SampleType::U16);
        let packed = [1u16, 65535, 255, 32768].map(u16::to_ne_bytes).concat();
        let mut padded = packed[..4].to_vec();
        padded.extend_from_slice(&[0xa7; 4]);
        padded.extend_from_slice(&packed[4..]);
        let mut whole = PixelChecks::new(
            checks.list(CheckScope::GroupPixels),
            CheckScope::GroupPixels,
            ID,
            &info,
        )
        .unwrap();
        whole.rows(ImageRef::new(&padded, 8, &info)).unwrap();
        let result = whole.results().unwrap();
        let mut chunks = PixelChecks::new(
            checks.list(CheckScope::GroupPixels),
            CheckScope::GroupPixels,
            ID,
            &info,
        )
        .unwrap();
        let row = ImageInfo::new(2, 1, Channels::Gray, SampleType::U16);
        for bytes in packed.as_chunks::<4>().0 {
            chunks.rows(ImageRef::packed(bytes, &row)).unwrap();
        }
        assert_eq!(chunks.results().unwrap(), result);
        assert_eq!(
            chunks.verify(&result).unwrap(),
            CheckCoverage {
                verified: 2,
                unsupported: 0,
                skipped: 0
            }
        );
        let mut preimage = b"IMI/v4/pixel-group\0".to_vec();
        preimage.extend_from_slice(&[0, 7, 2, 3, 2, Channels::Gray as u8, SampleType::U16 as u8]);
        preimage.extend_from_slice(&[1u16, 65535, 255, 32768].map(u16::to_le_bytes).concat());
        preimage.extend_from_slice(&[2, 8]);
        assert_eq!(
            &result[..8],
            &xxhash_rust::xxh3::xxh3_64(&preimage).to_le_bytes()
        );
        assert_eq!(
            &result[8..],
            &xxhash_rust::xxh3::xxh3_128(&preimage).to_le_bytes()
        );
    }

    #[test]
    fn scopes_and_identity_are_bound_and_failures_cannot_be_retried_away() {
        let declaration = declaration();
        let (checks, _) = Checks::read(&declaration, CheckLimits::default()).unwrap();
        let info = ImageInfo::new(2, 1, Channels::Gray, SampleType::U8);
        let pixels = ImageRef::packed(&[9, 13], &info);
        let mut group = PixelChecks::new(
            checks.list(CheckScope::GroupPixels),
            CheckScope::GroupPixels,
            ID,
            &info,
        )
        .unwrap();
        group.rows(pixels).unwrap();
        let expected = group.results().unwrap();
        let mut tile = PixelChecks::new(
            checks.list(CheckScope::TilePixels),
            CheckScope::TilePixels,
            ID,
            &info,
        )
        .unwrap();
        tile.rows(pixels).unwrap();
        assert_ne!(tile.results().unwrap(), expected);
        let mut changed = PixelChecks::new(
            checks.list(CheckScope::GroupPixels),
            CheckScope::GroupPixels,
            PixelCheckIdentity { id: 8, ..ID },
            &info,
        )
        .unwrap();
        changed.rows(pixels).unwrap();
        assert!(changed.verify(&expected).is_err());
        let own = changed.results();
        assert!(own.is_err());
        let mut invalid = PixelChecks::new(
            checks.list(CheckScope::GroupPixels),
            CheckScope::GroupPixels,
            ID,
            &info,
        )
        .unwrap();
        assert!(invalid.rows(ImageRef::new(&[9], 2, &info)).is_err());
        assert!(invalid.rows(pixels).is_err());
        assert!(invalid.verify(&expected).is_err());
    }

    #[test]
    fn discovered_height_preserves_float_bits_and_cancellation_is_sticky() {
        let declaration = declaration();
        let (checks, _) = Checks::read(&declaration, CheckLimits::default()).unwrap();
        let info = ImageInfo::new(2, 2, Channels::Gray, SampleType::F32);
        let pixels = [0x7f800001u32, 0x80000000, 0x7fc00007, 0x7f800000]
            .map(u32::to_ne_bytes)
            .concat();
        let mut fixed = PixelChecks::new(
            checks.list(CheckScope::TilePixels),
            CheckScope::TilePixels,
            ID,
            &info,
        )
        .unwrap();
        fixed.rows(ImageRef::packed(&pixels, &info)).unwrap();
        let expected = fixed.results().unwrap();
        let mut growing = info.clone();
        growing.height = 0;
        let mut unknown = PixelChecks::new(
            checks.list(CheckScope::TilePixels),
            CheckScope::TilePixels,
            ID,
            &growing,
        )
        .unwrap();
        let mut row = info.clone();
        row.height = 1;
        for bytes in pixels.as_chunks::<8>().0 {
            unknown.rows(ImageRef::packed(bytes, &row)).unwrap();
        }
        assert_eq!(unknown.results().unwrap(), expected);
        let mut cancelled = PixelChecks::new(
            checks.list(CheckScope::TilePixels),
            CheckScope::TilePixels,
            ID,
            &info,
        )
        .unwrap();
        assert!(matches!(
            cancelled.rows_checked(ImageRef::packed(&pixels, &info), || Err(Error::Cancelled)),
            Err(Error::Cancelled)
        ));
        assert!(cancelled.verify(&expected).is_err());
    }
}
