//! Counted V4 integrity declarations and results. Cryptographic suites remain
//! opaque until their immutable message and trust contracts are implemented.

use crate::{ByteReader, Error, HashAlgorithm};
use alloc::vec::Vec;
use alloc::{boxed::Box, sync::Arc};

/// The bytes whose integrity a declaration describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckScope {
    GroupPixels = 0,
    TilePixels = 1,
    EncodedObject = 2,
}

/// Aggregate admission ceilings for one image's check declarations.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct CheckLimits {
    pub checks: usize,
    pub parameter_bytes: usize,
    pub result_bytes: usize,
}

impl Default for CheckLimits {
    fn default() -> Self {
        Self {
            checks: 1024,
            parameter_bytes: 128 << 10,
            result_bytes: 128 << 10,
        }
    }
}

/// One immutable suite declaration. Parameters contain public configuration;
/// applications must keep signing/MAC secrets outside encoded declarations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckDescriptor<'a> {
    pub suite_id: u64,
    pub result_bytes: usize,
    pub parameters: &'a [u8],
}

impl CheckDescriptor<'_> {
    /// A built-in XXH3 declaration, using its frozen seed/default secret.
    /// Panics when passed a non-XXH3 algorithm.
    pub fn xxh3(algorithm: HashAlgorithm) -> Self {
        assert!(matches!(
            algorithm,
            HashAlgorithm::Xxh3_64 | HashAlgorithm::Xxh3_128
        ));
        Self {
            suite_id: u64::from(algorithm.wire()),
            result_bytes: algorithm.width(),
            parameters: &[],
        }
    }

    pub(crate) fn algorithm(self) -> Option<HashAlgorithm> {
        match self.suite_id {
            0 => Some(HashAlgorithm::Xxh3_64),
            1 => Some(HashAlgorithm::Xxh3_128),
            2 => HashAlgorithm::from_wire(2),
            5 => match self.result_bytes {
                32 => HashAlgorithm::from_wire(2),
                64 => HashAlgorithm::from_wire(5),
                128 => HashAlgorithm::from_wire(6),
                _ => None,
            },
            _ => None,
        }
    }

    fn admit(self, remaining: &mut CheckLimits) -> Result<(), Error> {
        if self.result_bytes == 0 {
            return Err(Error::Corrupt("empty V4 integrity result"));
        }
        let invalid = match self.suite_id {
            0 => self.result_bytes != 8 || !self.parameters.is_empty(),
            1 => self.result_bytes != 16 || !self.parameters.is_empty(),
            2 => self.result_bytes != 32 || !self.parameters.is_empty(),
            3 => self.result_bytes != 32,
            4 => self.result_bytes != 64,
            5 => !matches!(self.result_bytes, 32 | 64 | 128) || !self.parameters.is_empty(),
            _ => false,
        };
        if invalid {
            return Err(Error::Corrupt("V4 integrity suite parameters"));
        }
        remaining.checks = remaining.checks.checked_sub(1).ok_or(Error::TooLarge)?;
        remaining.parameter_bytes = remaining
            .parameter_bytes
            .checked_sub(self.parameters.len())
            .ok_or(Error::TooLarge)?;
        remaining.result_bytes = remaining
            .result_bytes
            .checked_sub(self.result_bytes)
            .ok_or(Error::TooLarge)?;
        Ok(())
    }

    /// Check a supplied canonical byte preimage. Pixel callers must supply the
    /// specified pixel preimage, not compressed bytes or a strided allocation.
    /// Unsupported suites never succeed, including opaque signature slots.
    pub fn verify_bytes(self, preimage: &[u8], expected: &[u8]) -> Result<(), Error> {
        let algorithm = self
            .algorithm()
            .ok_or(Error::Unsupported("V4 integrity suite"))?;
        self.admit(&mut CheckLimits::default())?;
        if expected.len() != self.result_bytes {
            return Err(Error::Corrupt("V4 integrity result width"));
        }
        if algorithm.digest(preimage).as_bytes() != expected {
            return Err(Error::Corrupt("V4 integrity mismatch"));
        }
        Ok(())
    }
}

/// Borrowed descriptor list; reading declarations allocates no tag or key state.
#[derive(Clone, Copy, Debug, Default)]
pub struct CheckList<'a> {
    descriptors: &'a [u8],
    count: usize,
    result_bytes: usize,
}

fn descriptor<'a>(r: &mut ByteReader<'a>) -> Result<CheckDescriptor<'a>, Error> {
    let suite_id = r.varint()?;
    let result_bytes = r.var_usize()?;
    let length = r.varint()?;
    Ok(CheckDescriptor {
        suite_id,
        result_bytes,
        parameters: r.bytes_len(length)?,
    })
}

fn ordered(previous: Option<CheckDescriptor<'_>>, next: CheckDescriptor<'_>) -> Result<(), Error> {
    if let Some(previous) = previous
        && (previous.suite_id, previous.parameters) >= (next.suite_id, next.parameters)
    {
        return Err(Error::Corrupt("V4 integrity descriptor order"));
    }
    Ok(())
}

impl<'a> CheckList<'a> {
    pub const fn len(self) -> usize {
        self.count
    }
    pub const fn is_empty(self) -> bool {
        self.count == 0
    }
    pub const fn result_bytes(self) -> usize {
        self.result_bytes
    }

    /// Iterate only already admitted declarations. No allocation depends on IDs.
    pub fn iter(self) -> impl Iterator<Item = CheckDescriptor<'a>> {
        let mut r = ByteReader::new(self.descriptors);
        (0..self.count).map(move |_| descriptor(&mut r).expect("admitted V4 integrity declaration"))
    }

    /// Split exactly this scope's result bytes in declaration order. This proves
    /// framing only; it does not verify a digest or authenticate a signature.
    pub fn results(self, bytes: &'a [u8]) -> Result<impl Iterator<Item = CheckResult<'a>>, Error> {
        if bytes.len() != self.result_bytes {
            return Err(Error::Corrupt("V4 integrity result span"));
        }
        let mut at = 0;
        Ok(self.iter().map(move |check| {
            let end = at + check.result_bytes;
            let result = CheckResult {
                check,
                bytes: &bytes[at..end],
            };
            at = end;
            result
        }))
    }
}

/// A framed integrity value and its declaration; no verification is implied.
#[derive(Clone, Copy, Debug)]
pub struct CheckResult<'a> {
    pub check: CheckDescriptor<'a>,
    pub bytes: &'a [u8],
}

/// Independent group-pixel, tile-pixel and encoded-object check lists.
#[derive(Clone, Copy, Debug, Default)]
pub struct Checks<'a> {
    lists: [CheckList<'a>; 3],
}

impl<'a> Checks<'a> {
    pub(crate) fn owned(self) -> Result<Option<Arc<OwnedChecks>>, Error> {
        let mut flags = 0u64;
        let mut size = 1usize;
        for (scope, list) in self.lists.iter().enumerate() {
            if !list.is_empty() {
                flags |= 1 << scope;
                size = size
                    .checked_add(crate::varint_len(list.count as u64))
                    .and_then(|n| n.checked_add(list.descriptors.len()))
                    .ok_or(Error::TooLarge)?;
            }
        }
        if flags == 0 {
            return Ok(None);
        }
        let mut encoded = Vec::new();
        encoded
            .try_reserve_exact(size)
            .map_err(|_| Error::TooLarge)?;
        crate::put_varint(&mut encoded, flags);
        for list in self.lists {
            if !list.is_empty() {
                crate::put_varint(&mut encoded, list.count as u64);
                encoded.extend_from_slice(list.descriptors);
            }
        }
        Ok(Some(Arc::new(OwnedChecks {
            encoded: encoded.into_boxed_slice(),
        })))
    }
    pub const fn list(self, scope: CheckScope) -> CheckList<'a> {
        self.lists[scope as usize]
    }

    #[cfg(feature = "std")]
    pub(crate) fn require_supported(self) -> Result<(), Error> {
        if self
            .lists
            .iter()
            .flat_map(|list| list.iter())
            .any(|check| check.algorithm().is_none())
        {
            return Err(Error::Unsupported(
                "V4 integrity suite in strict validation",
            ));
        }
        Ok(())
    }

    /// Read a canonical declaration prefix within explicit aggregate ceilings.
    /// Remaining input belongs to the enclosing setup parser.
    pub fn read(input: &'a [u8], mut remaining: CheckLimits) -> Result<(Self, usize), Error> {
        let mut r = ByteReader::new(input);
        let flags = r.varint()?;
        if flags & !7 != 0 {
            return Err(Error::Unsupported("V4 integrity scopes"));
        }
        let mut lists = [CheckList::default(); 3];
        for (scope, list) in lists.iter_mut().enumerate() {
            if flags & (1 << scope) == 0 {
                continue;
            }
            let count = r.var_usize()?;
            if count == 0 {
                return Err(Error::Corrupt("empty V4 integrity list"));
            }
            if count > remaining.checks {
                return Err(Error::TooLarge);
            }
            let start = r.pos;
            let mut previous = None;
            let mut result_bytes = 0usize;
            for _ in 0..count {
                let check = descriptor(&mut r)?;
                check.admit(&mut remaining)?;
                ordered(previous, check)?;
                result_bytes = result_bytes
                    .checked_add(check.result_bytes)
                    .ok_or(Error::TooLarge)?;
                previous = Some(check);
            }
            *list = CheckList {
                descriptors: &input[start..r.pos],
                count,
                result_bytes,
            };
        }
        Ok((Self { lists }, r.pos))
    }

    /// Append declarations atomically after validating all descriptors. The
    /// three slices are group pixels, tile pixels and encoded objects, in order.
    pub fn append(
        out: &mut Vec<u8>,
        lists: [&[CheckDescriptor<'_>]; 3],
        mut remaining: CheckLimits,
    ) -> Result<(), Error> {
        let mut flags = 0u64;
        let mut size = 1usize;
        for (scope, list) in lists.iter().enumerate() {
            if list.is_empty() {
                continue;
            }
            if list.len() > remaining.checks {
                return Err(Error::TooLarge);
            }
            flags |= 1 << scope;
            size = size
                .checked_add(crate::varint_len(list.len() as u64))
                .ok_or(Error::TooLarge)?;
            let mut previous = None;
            for &check in *list {
                check.admit(&mut remaining)?;
                ordered(previous, check)?;
                for part in [
                    crate::varint_len(check.suite_id),
                    crate::varint_len(check.result_bytes as u64),
                    crate::varint_len(check.parameters.len() as u64),
                    check.parameters.len(),
                ] {
                    size = size.checked_add(part).ok_or(Error::TooLarge)?;
                }
                previous = Some(check);
            }
        }
        out.try_reserve_exact(size).map_err(|_| Error::TooLarge)?;
        crate::put_varint(out, flags);
        for list in lists {
            if list.is_empty() {
                continue;
            }
            crate::put_varint(out, list.len() as u64);
            for check in list {
                crate::put_varint(out, check.suite_id);
                crate::put_varint(out, check.result_bytes as u64);
                crate::put_varint(out, check.parameters.len() as u64);
                out.extend_from_slice(check.parameters);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct OwnedChecks {
    encoded: Box<[u8]>,
}

impl OwnedChecks {
    pub(crate) fn read_tail<C: Fn() -> bool + Sync>(
        bytes: &[u8],
        session: &crate::decode_resources::DecodeSession<C>,
    ) -> Result<Option<Arc<Self>>, Error> {
        if bytes.is_empty() {
            return Ok(None);
        }
        let (checks, used) = Checks::read(bytes, CheckLimits::default())?;
        if used != bytes.len() {
            return Err(Error::Corrupt("V4 integrity setup trailing bytes"));
        }
        #[cfg(all(feature = "lab", feature = "std"))]
        if crate::trace::on() {
            let start = ByteReader::new(bytes).mark();
            crate::trace::trace_enter_addr("payload", "V4:6.2", "scoped check declarations", start);
            crate::trace::tnote!(
                "group pixel checks",
                alloc::format!("{}", checks.list(CheckScope::GroupPixels).len())
            );
            crate::trace::tnote!(
                "tile pixel checks",
                alloc::format!("{}", checks.list(CheckScope::TilePixels).len())
            );
            crate::trace::tnote!(
                "encoded object checks",
                alloc::format!("{}", checks.list(CheckScope::EncodedObject).len())
            );
            crate::trace::exit_addr(start.plus(used));
        }
        use crate::parse::limits::Admission;
        session.v4_models().hint(
            bytes
                .len()
                .checked_add(core::mem::size_of::<Self>() + 2 * core::mem::size_of::<usize>())
                .ok_or(Error::TooLarge)?,
        )?;
        let owned = checks
            .owned()?
            .ok_or(Error::Corrupt("empty V4 integrity setup suffix"))?;
        Ok(Some(owned))
    }

    pub(crate) fn get(&self) -> Checks<'_> {
        Checks::read(
            &self.encoded,
            CheckLimits {
                checks: usize::MAX,
                parameter_bytes: usize::MAX,
                result_bytes: usize::MAX,
            },
        )
        .expect("owned admitted V4 integrity declarations")
        .0
    }

    #[cfg(feature = "encode")]
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.encoded
    }

    pub(crate) fn width(&self, scope: CheckScope) -> usize {
        self.get().list(scope).result_bytes()
    }

    pub(crate) fn split<'a>(
        &self,
        scope: CheckScope,
        input: &'a [u8],
    ) -> Result<(&'a [u8], &'a [u8]), Error> {
        let at = input
            .len()
            .checked_sub(self.width(scope))
            .ok_or(Error::Corrupt("short V4 check results"))?;
        Ok(input.split_at(at))
    }

    #[cfg(feature = "encode")]
    pub(crate) fn pixels(
        &self,
        scope: CheckScope,
        identity: super::PixelCheckIdentity,
        image: crate::ImageRef<'_>,
    ) -> Result<Vec<u8>, Error> {
        if self.width(scope) == 0 {
            return Ok(Vec::new());
        }
        let mut state =
            super::PixelChecks::new(self.get().list(scope), scope, identity, image.info)?;
        state.rows(image)?;
        state.results()
    }

    pub(crate) fn encoded_results<'a>(
        &self,
        context: &super::SetupContext,
        parts: impl Iterator<Item = &'a [u8]> + Clone,
    ) -> Result<Vec<u8>, Error> {
        let list = self.get().list(CheckScope::EncodedObject);
        let mut values = Vec::new();
        values
            .try_reserve_exact(list.result_bytes())
            .map_err(|_| Error::TooLarge)?;
        for descriptor in list.iter() {
            let algorithm = descriptor
                .algorithm()
                .ok_or(Error::Unsupported("V4 integrity suite"))?;
            let mut hash = crate::Hasher::new(algorithm);
            hash.update(b"IMI/v4/encoded-check\0");
            hash.update(context.for_encoded_suite(descriptor.suite_id));
            for part in parts.clone() {
                hash.update(part);
            }
            values.extend_from_slice(hash.finish().as_bytes());
        }
        Ok(values)
    }

    pub(crate) fn verify_encoded<C: Fn() -> bool + Sync>(
        &self,
        context: &super::SetupContext,
        parts: &[&[u8]],
        values: &[u8],
        session: &crate::decode_resources::DecodeSession<C>,
    ) -> Result<(), Error> {
        let list = self.get().list(CheckScope::EncodedObject);
        if !list.is_empty() {
            session.adapter_work(list.len() as u128)?;
        }
        for result in list.results(values)? {
            if let Some(algorithm) = result.check.algorithm() {
                let mut hash = crate::Hasher::new(algorithm);
                hash.update(b"IMI/v4/encoded-check\0");
                hash.update(context.for_encoded_suite(result.check.suite_id));
                for part in parts {
                    for chunk in part.chunks(64 << 10) {
                        session.adapter_work(chunk.len() as u128)?;
                        hash.update(chunk);
                    }
                }
                if hash.finish().as_bytes() != result.bytes {
                    return Err(Error::Corrupt("V4 encoded check mismatch"));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn verify_pixels<C: Fn() -> bool + Sync>(
        &self,
        scope: CheckScope,
        identity: super::PixelCheckIdentity,
        image: crate::ImageRef<'_>,
        expected: &[u8],
        session: &crate::decode_resources::DecodeSession<C>,
    ) -> Result<super::CheckCoverage, Error> {
        let list = self.get().list(scope);
        if list.is_empty() {
            if !expected.is_empty() {
                return Err(Error::Corrupt("undeclared V4 pixel results"));
            }
            return Ok(super::CheckCoverage::default());
        }
        if !session.v4_pixel_checks(scope) {
            return Ok(super::CheckCoverage {
                skipped: list.iter().count(),
                ..Default::default()
            });
        }
        let known = list.iter().filter(|d| d.algorithm().is_some()).count();
        session.adapter_work(
            u128::from(image.info.width)
                * u128::from(image.info.height)
                * image.info.bytes_per_pixel() as u128
                * known as u128
                + list.len() as u128,
        )?;
        let _working = session.v4_working(super::PixelChecks::working_bytes(list) as u128)?;
        let mut state = super::PixelChecks::new(list, scope, identity, image.info)?;
        use crate::parse::limits::Admission;
        state.rows_checked(image, || session.v4_models().check())?;
        state.verify(expected)
    }
}

/// A borrowed tile tag plus immutable interpretation. Raster verification is
/// separate from structural parsing; parallel workers finish before this runs.
pub(crate) struct TilePixelCheck<'a> {
    pub(crate) checks: Arc<OwnedChecks>,
    pub(crate) id: u64,
    pub(crate) rect: crate::planes::Rect,
    pub(crate) expected: &'a [u8],
}

impl TilePixelCheck<'_> {
    /// A crop verifies a tile only when every visible native sample is present.
    pub(crate) fn verify_cropped<C: Fn() -> bool + Sync>(
        &self,
        info: &crate::ImageInfo,
        output: &[u8],
        stride: usize,
        crop: crate::planes::Rect,
        session: &crate::decode_resources::DecodeSession<C>,
    ) -> Result<Option<super::CheckCoverage>, Error> {
        if self.rect.x() < crop.x()
            || self.rect.y() < crop.y()
            || self.rect.x() + self.rect.width() > crop.x() + crop.width()
            || self.rect.y() + self.rect.height() > crop.y() + crop.height()
        {
            return Ok(None);
        }
        let mut local = info.clone();
        local.width = self.rect.width();
        local.height = self.rect.height();
        let at = (self.rect.y() - crop.y()) as usize * stride
            + (self.rect.x() - crop.x()) as usize * info.bytes_per_pixel();
        let pixels = output
            .get(at..)
            .ok_or(Error::InvalidInput("V4 cropped tile extent"))?;
        self.checks
            .verify_pixels(
                CheckScope::TilePixels,
                super::PixelCheckIdentity {
                    image: 0,
                    id: self.id,
                    x: self.rect.x(),
                    y: self.rect.y(),
                },
                crate::ImageRef::new(pixels, stride, &local),
                self.expected,
                session,
            )
            .map(Some)
    }

    pub(crate) fn verify<C: Fn() -> bool + Sync>(
        &self,
        info: &crate::ImageInfo,
        output: &[u8],
        stride: usize,
        session: &crate::decode_resources::DecodeSession<C>,
    ) -> Result<super::CheckCoverage, Error> {
        let mut local = info.clone();
        local.width = self.rect.width();
        local.height = self.rect.height();
        let at = (self.rect.y() as usize)
            .checked_mul(stride)
            .and_then(|v| v.checked_add(self.rect.x() as usize * info.bytes_per_pixel()))
            .ok_or(Error::TooLarge)?;
        let pixels = output
            .get(at..)
            .ok_or(Error::InvalidInput("V4 tile raster extent"))?;
        self.checks.verify_pixels(
            CheckScope::TilePixels,
            super::PixelCheckIdentity {
                image: 0,
                id: self.id,
                x: self.rect.x(),
                y: self.rect.y(),
            },
            crate::ImageRef::new(pixels, stride, &local),
            self.expected,
            session,
        )
    }
}

pub(crate) fn tile_pixel_rect(
    layout: super::TileLayout,
    id: u64,
    group: impl Fn(u64) -> Result<crate::planes::Rect, Error>,
) -> Result<crate::planes::Rect, Error> {
    let tile = layout.tile(id)?;
    let columns = u64::from(layout.group_grid().0);
    let first = group(u64::from(tile.group_y) * columns + u64::from(tile.group_x))?;
    let last = group(
        u64::from(tile.group_y + tile.groups_high - 1) * columns
            + u64::from(tile.group_x + tile.groups_wide - 1),
    )?;
    Ok(crate::planes::Rect::new(
        first.x(),
        first.y(),
        last.x() + last.width() - first.x(),
        last.y() + last.height() - first.y(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn independent_scopes_match_the_wire_example_and_keep_following_setup() {
        let group = [CheckDescriptor::xxh3(HashAlgorithm::Xxh3_64)];
        let tile = [CheckDescriptor::xxh3(HashAlgorithm::Xxh3_128)];
        let mut encoded = Vec::new();
        Checks::append(&mut encoded, [&group, &tile, &[]], CheckLimits::default()).unwrap();
        assert_eq!(encoded, [3, 1, 0, 8, 0, 1, 1, 16, 0]);
        let length = encoded.len();
        encoded.extend_from_slice(b"following setup");
        let (checks, used) = Checks::read(&encoded, CheckLimits::default()).unwrap();
        assert_eq!(used, length);
        assert_eq!(
            checks
                .list(CheckScope::GroupPixels)
                .iter()
                .collect::<Vec<_>>(),
            group
        );
        assert_eq!(
            checks
                .list(CheckScope::TilePixels)
                .iter()
                .collect::<Vec<_>>(),
            tile
        );
        assert!(checks.list(CheckScope::EncodedObject).is_empty());
        for flags in 0..8 {
            let mut bytes = Vec::new();
            let list: &[CheckDescriptor<'_>] = &group;
            let lists = core::array::from_fn(|i| if flags & (1 << i) != 0 { list } else { &[] });
            Checks::append(&mut bytes, lists, CheckLimits::default()).unwrap();
            let (parsed, used) = Checks::read(&bytes, CheckLimits::default()).unwrap();
            assert_eq!(used, bytes.len());
            for scope in [
                CheckScope::GroupPixels,
                CheckScope::TilePixels,
                CheckScope::EncodedObject,
            ] {
                assert_eq!(
                    !parsed.list(scope).is_empty(),
                    flags & (1 << scope as usize) != 0
                );
            }
        }
    }

    #[test]
    fn opaque_64_byte_signature_slots_do_not_claim_verification() {
        let descriptors = [
            CheckDescriptor::xxh3(HashAlgorithm::Xxh3_64),
            CheckDescriptor {
                suite_id: 1 << 40,
                result_bytes: 64,
                parameters: b"public key reference",
            },
        ];
        let mut bytes = Vec::new();
        Checks::append(&mut bytes, [&descriptors, &[], &[]], CheckLimits::default()).unwrap();
        let (checks, _) = Checks::read(&bytes, CheckLimits::default()).unwrap();
        let list = checks.list(CheckScope::GroupPixels);
        assert_eq!(list.result_bytes(), 72);
        let values = [0xa7; 72];
        let results = list.results(&values).unwrap().collect::<Vec<_>>();
        assert_eq!(results[0].bytes.len(), 8);
        assert_eq!(results[1].bytes.len(), 64);
        assert_eq!(results[1].check.parameters, b"public key reference");
        assert!(matches!(
            results[1].check.verify_bytes(b"payload", results[1].bytes),
            Err(Error::Unsupported(_))
        ));
        assert!(list.results(&values[..71]).is_err());
        assert!(list.results(&[0; 73]).is_err());
    }

    #[test]
    fn canonical_framing_and_admission_fail_before_allocation() {
        for bytes in [
            &[1, 0][..],
            &[8],
            &[1, 1, 0, 16, 0],
            &[1, 1, 1, 8, 0],
            &[1, 1, 0, 8, 1, 0],
            &[1, 2, 1, 16, 0, 0, 8, 0],
            &[1, 2, 0, 8, 0, 0, 8, 0],
            &[1, 1, 2, 0, 0],
            &[1, 1, 0, 0x88, 0, 0],
        ] {
            assert!(
                Checks::read(bytes, CheckLimits::default()).is_err(),
                "{bytes:?}"
            );
        }
        let valid = [3, 1, 0, 8, 0, 1, 1, 16, 0];
        for end in 0..valid.len() {
            assert!(Checks::read(&valid[..end], CheckLimits::default()).is_err());
        }
        let limits = CheckLimits {
            checks: 1,
            ..CheckLimits::default()
        };
        assert!(matches!(Checks::read(&valid, limits), Err(Error::TooLarge)));
        let limits = CheckLimits {
            result_bytes: 23,
            ..CheckLimits::default()
        };
        assert!(matches!(Checks::read(&valid, limits), Err(Error::TooLarge)));
        let huge = [1, 255, 255, 255, 255, 15];
        assert!(matches!(
            Checks::read(&huge, CheckLimits::default()),
            Err(Error::TooLarge)
        ));
        let mut out = vec![0xaa];
        let duplicate = [CheckDescriptor::xxh3(HashAlgorithm::Xxh3_64); 2];
        assert!(Checks::append(&mut out, [&duplicate, &[], &[]], CheckLimits::default()).is_err());
        assert_eq!(out, [0xaa]);
    }

    #[test]
    fn a_thousand_wide_results_are_bounded_by_the_counted_envelope() {
        let mut declarations: Vec<_> = (128..1152)
            .map(|suite_id| CheckDescriptor {
                suite_id,
                result_bytes: 128,
                parameters: &[],
            })
            .collect();
        let mut wire = Vec::new();
        Checks::append(&mut wire, [&declarations, &[], &[]], CheckLimits::default()).unwrap();
        let (checks, used) = Checks::read(&wire, CheckLimits::default()).unwrap();
        assert_eq!(used, wire.len());
        let list = checks.list(CheckScope::GroupPixels);
        assert_eq!(list.len(), 1024);
        assert_eq!(list.result_bytes(), 128 << 10);
        declarations.push(CheckDescriptor {
            suite_id: 1152,
            result_bytes: 1,
            parameters: &[],
        });
        assert!(matches!(
            Checks::append(&mut wire, [&declarations, &[], &[]], CheckLimits::default()),
            Err(Error::TooLarge)
        ));
    }

    #[test]
    fn both_xxh3_definitions_check_actual_bytes_and_reject_corruption() {
        let input = b"canonical native sample bytes";
        for algorithm in [HashAlgorithm::Xxh3_64, HashAlgorithm::Xxh3_128] {
            let expected = algorithm.digest(input);
            CheckDescriptor::xxh3(algorithm)
                .verify_bytes(input, expected.as_bytes())
                .unwrap();
            assert!(
                CheckDescriptor::xxh3(algorithm)
                    .verify_bytes(b"altered pixels", expected.as_bytes())
                    .is_err()
            );
            let mut corrupt = expected.as_bytes().to_vec();
            corrupt[0] ^= 1;
            assert!(
                CheckDescriptor::xxh3(algorithm)
                    .verify_bytes(input, &corrupt)
                    .is_err()
            );
        }
    }
}
