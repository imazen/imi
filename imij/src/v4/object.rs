use alloc::vec::Vec;

use crate::decode_resources::DecodeSession;
use crate::{ByteReader, Error, HashAlgorithm, Hasher, put_varint, varint_len};

const WIDTH: usize = 16;
const HASH_CHUNK: usize = 64 * 1024;

/// Integrity mode selected once in the V4 setup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ObjectIntegrity {
    /// No local digest; structural checks still apply.
    #[default]
    None,
    /// XXH3-128 trailer on each encoded object.
    Objects,
    /// Object trailers plus an aggregate digest at spatial tile close.
    ObjectsAndTiles,
}

impl TryFrom<u64> for ObjectIntegrity {
    type Error = Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Objects),
            2 => Ok(Self::ObjectsAndTiles),
            _ => Err(Error::Unsupported("V4 object integrity mode")),
        }
    }
}

impl ObjectIntegrity {
    /// Canonical unsigned-varint value in the setup.
    pub const fn wire_value(self) -> u64 {
        match self {
            Self::None => 0,
            Self::Objects => 1,
            Self::ObjectsAndTiles => 2,
        }
    }

    /// Local digest bytes appended to each object.
    pub const fn trailer_len(self) -> usize {
        match self {
            Self::None => 0,
            Self::Objects | Self::ObjectsAndTiles => WIDTH,
        }
    }
}

/// Hash binding to an already admitted canonical image/coding declaration.
///
/// Construct this from semantic setup bytes, excluding mutable index slots,
/// physical offsets and padding. The V4 setup serializer owns that declaration;
/// callers must not substitute the entire relocatable file header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupContext([u8; WIDTH], #[cfg(feature = "blake3")] [u8; 32]);

impl SetupContext {
    /// Hash an already admitted canonical declaration without allocating.
    pub fn from_declaration(declaration: &[u8]) -> Self {
        Self::from_parts(&[], declaration)
    }

    fn from_parts(fields: &[u8], setup: &[u8]) -> Self {
        let mut hash = Hasher::new(HashAlgorithm::Xxh3_128);
        hash.update(b"IMI/v4/setup\0");
        hash.update(fields);
        hash.update(setup);
        #[cfg(feature = "blake3")]
        let strong = {
            let mut hash = blake3::Hasher::new();
            hash.update(b"IMI/v4/setup-blake3\0");
            hash.update(fields);
            hash.update(setup);
            *hash.finalize().as_bytes()
        };
        Self(
            hash.finish().as_bytes().try_into().unwrap(),
            #[cfg(feature = "blake3")]
            strong,
        )
    }

    pub(super) fn for_encoded_suite(&self, _suite_id: u64) -> &[u8] {
        #[cfg(feature = "blake3")]
        if matches!(_suite_id, 2 | 5) {
            return &self.1;
        }
        &self.0
    }

    /// Bind immutable setup to the stored image interpretation. Mutable front
    /// directories/reservations must have been excluded by the setup serializer.
    /// Metadata that does not participate in stored-sample reconstruction is
    /// outside this context; its own container checksum policy still applies.
    pub fn from_image_declaration(
        info: &crate::ImageInfo,
        coding: crate::Coding,
        color: Option<crate::LossyColor>,
        immutable_setup: &[u8],
    ) -> Result<Self, Error> {
        if coding.is_lossy() != color.is_some() {
            return Err(Error::InvalidInput("V4 coding/color declaration"));
        }
        info.validate_precision()?;
        let mut fields = Vec::new();
        fields.try_reserve_exact(128).map_err(|_| Error::TooLarge)?;
        for value in [
            u64::from(info.width),
            u64::from(info.height),
            coding as u64,
            info.channels as u64,
            info.sample as u64,
            u64::from(info.cicp.color_primaries),
            u64::from(info.cicp.transfer_characteristics),
            u64::from(info.cicp.matrix_coefficients),
            u64::from(info.cicp.full_range),
            u64::from(info.alpha_wire()?),
        ] {
            put_varint(&mut fields, value);
        }
        for c in 0..4 {
            put_varint(&mut fields, u64::from(info.channel_bits(c).unwrap_or(0)));
        }
        if let Some(color) = color {
            put_varint(&mut fields, color as u64);
        }
        put_varint(&mut fields, immutable_setup.len() as u64);
        Ok(Self::from_parts(&fields, immutable_setup))
    }

    /// Canonical little-endian context bytes.
    pub const fn as_bytes(&self) -> &[u8; WIDTH] {
        &self.0
    }

    fn object_hash(self) -> Hasher {
        let mut hash = Hasher::new(HashAlgorithm::Xxh3_128);
        hash.update(b"IMI/v4/object\0");
        hash.update(&self.0);
        hash
    }

    pub(super) fn tile_hash(self) -> Hasher {
        let mut hash = Hasher::new(HashAlgorithm::Xxh3_128);
        hash.update(b"IMI/v4/tile\0");
        hash.update(&self.0);
        hash
    }
}

/// A framed object view. Its type-specific grammar remains the caller's job.
#[derive(Clone, Copy, Debug)]
pub struct Object<'a> {
    /// Canonical record kind, including unknown ancillary kinds.
    pub kind: u64,
    /// Body without the integrity trailer; borrows the supplied input.
    pub body: &'a [u8],
    /// Complete frame bytes consumed, including its integrity trailer.
    pub encoded_len: usize,
    /// Whether the declared local digest was checked successfully.
    pub integrity_verified: bool,
}

/// Incrementally charged V4 object parser with request cancellation.
///
/// Input is charged per delivered object, including after a failed digest.
/// Do not additionally charge the same bytes through a whole-slice session
/// operation. This parser verifies framing/local integrity, not directory,
/// model, tile or pixel validity. A later object need not have arrived yet.
pub struct ObjectReader<'s, C> {
    mode: ObjectIntegrity,
    context: SetupContext,
    session: &'s DecodeSession<C>,
    max_object_bytes: u64,
    checks: Option<alloc::sync::Arc<super::OwnedChecks>>,
}

impl<'s, C: Fn() -> bool + Sync> ObjectReader<'s, C> {
    /// Bind a cumulative session and an additional single-object byte ceiling.
    pub fn new(
        mode: ObjectIntegrity,
        context: SetupContext,
        session: &'s DecodeSession<C>,
        max_object_bytes: u64,
    ) -> Self {
        Self {
            mode,
            context,
            session,
            max_object_bytes,
            checks: None,
        }
    }

    pub(crate) fn with_checks(
        mut self,
        checks: Option<alloc::sync::Arc<super::OwnedChecks>>,
    ) -> Self {
        self.checks = checks;
        self
    }
    pub(super) fn check_config(&self) -> Option<&alloc::sync::Arc<super::OwnedChecks>> {
        self.checks.as_ref()
    }

    /// Read one complete object from a prefix; leave following bytes untouched.
    ///
    /// A missing suffix is Corrupt, not successful completion or EOF. A feed
    /// adapter must distinguish temporary input exhaustion before calling this
    /// complete-object operation. No count-dependent allocation is performed.
    pub fn read<'a>(&self, data: &'a [u8]) -> Result<Object<'a>, Error> {
        self.read_inner(data, false)
    }

    pub(super) fn read_admitted<'a>(&self, data: &'a [u8]) -> Result<Object<'a>, Error> {
        self.read_inner(data, true)
    }

    fn read_inner<'a>(&self, data: &'a [u8], admitted: bool) -> Result<Object<'a>, Error> {
        self.session.adapter_work(1)?;
        let mut r = ByteReader::new(data);
        crate::trace::trace_enter_addr("record", "V4:6.1", "V4 framed object", r.mark());
        let kind = crate::trace::tf!("object kind", r, r.varint()?);
        let length = crate::trace::tf!("framed length", r, r.varint()?);
        let total = length.checked_add(r.pos as u64).ok_or(Error::TooLarge)?;
        if total > self.max_object_bytes {
            return Err(Error::TooLarge);
        }
        let length = usize::try_from(length).map_err(|_| Error::TooLarge)?;
        let total = usize::try_from(total).map_err(|_| Error::TooLarge)?;
        let digest_bytes = self.mode.trailer_len();
        if length < digest_bytes {
            return Err(Error::Corrupt("short V4 integrity trailer"));
        }
        let body_mark = r.mark();
        let body = r.bytes(length)?;
        #[cfg(all(feature = "lab", feature = "std"))]
        if crate::trace::on() {
            trace_body(
                self.checks.as_deref(),
                kind,
                length - digest_bytes,
                body_mark,
            );
        }
        if digest_bytes != 0 {
            crate::trace::trace_enter_addr(
                "digest",
                "V4:6.1",
                "XXH3-128 object digest",
                body_mark.plus(length - digest_bytes),
            );
            crate::trace::exit_addr(r.mark());
        }
        let unit = matches!(
            kind,
            crate::registry::UNIT_STRIPE
                | crate::registry::UNIT_DC
                | crate::registry::UNIT_AC
                | crate::registry::UNIT_ALPHA
                | crate::registry::UNIT_EXACT
        );
        if admitted {
            self.session.v4_object_admitted(total, unit)?;
        } else {
            self.session.v4_object(total, unit)?;
        }
        let payload_end = total - digest_bytes;
        if digest_bytes != 0 {
            let mut hash = self.context.object_hash();
            for chunk in data[..payload_end].chunks(HASH_CHUNK) {
                self.session.adapter_work(chunk.len() as u128)?;
                hash.update(chunk);
            }
            if hash.finish().as_bytes() != &data[payload_end..total] {
                return Err(Error::Corrupt("V4 object checksum mismatch"));
            }
        }
        crate::trace::tnote!(
            "local integrity",
            if digest_bytes == 0 {
                alloc::string::String::from("not declared")
            } else {
                alloc::string::String::from("XXH3-128 verified")
            }
        );
        crate::trace::exit_addr(r.mark());
        let encoded_check_bytes = self
            .checks
            .as_ref()
            .map_or(0, |checks| checks.width(super::CheckScope::EncodedObject));
        let checked_end = payload_end
            .checked_sub(encoded_check_bytes)
            .filter(|&end| end >= r.pos - length)
            .ok_or(Error::Corrupt("short V4 encoded check results"))?;
        if let Some(checks) = &self.checks {
            checks.verify_encoded(
                &self.context,
                &[&data[..checked_end]],
                &data[checked_end..payload_end],
                self.session,
            )?;
        }
        Ok(Object {
            kind,
            body: &body[..length - digest_bytes - encoded_check_bytes],
            encoded_len: total,
            integrity_verified: digest_bytes != 0,
        })
    }
}

/// Append one admitted object, binding kind/length/body to its setup context.
///
/// Capacity and the single-object ceiling are checked before changing `out`.
/// Tile aggregate hashing is handled by the enclosing tile writer.
pub fn append_object(
    out: &mut Vec<u8>,
    kind: u64,
    body: &[u8],
    mode: ObjectIntegrity,
    context: SetupContext,
    max_object_bytes: u64,
) -> Result<(), Error> {
    append_object_parts(out, kind, &[body], mode, context, max_object_bytes)
}

pub(super) fn object_len(
    kind: u64,
    body_len: usize,
    mode: ObjectIntegrity,
) -> Result<usize, Error> {
    let length = body_len
        .checked_add(mode.trailer_len())
        .ok_or(Error::TooLarge)?;
    let total = varint_len(kind)
        .checked_add(varint_len(length as u64))
        .and_then(|n| n.checked_add(length))
        .ok_or(Error::TooLarge)?;
    Ok(total)
}

pub(super) fn append_object_parts(
    out: &mut Vec<u8>,
    kind: u64,
    parts: &[&[u8]],
    mode: ObjectIntegrity,
    context: SetupContext,
    max_object_bytes: u64,
) -> Result<(), Error> {
    let body_len = parts.iter().try_fold(0usize, |sum, part| {
        sum.checked_add(part.len()).ok_or(Error::TooLarge)
    })?;
    let total = object_len(kind, body_len, mode)?;
    if total as u64 > max_object_bytes {
        return Err(Error::TooLarge);
    }
    out.try_reserve_exact(total).map_err(|_| Error::TooLarge)?;
    let start = out.len();
    put_varint(out, kind);
    put_varint(out, (body_len + mode.trailer_len()) as u64);
    for part in parts {
        out.extend_from_slice(part);
    }
    if mode.trailer_len() != 0 {
        let mut hash = context.object_hash();
        hash.update(&out[start..]);
        out.extend_from_slice(hash.finish().as_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_resources::DecodePolicy;
    use alloc::vec;

    fn frame(mode: ObjectIntegrity) -> Vec<u8> {
        let mut out = Vec::new();
        append_object(
            &mut out,
            crate::registry::MODEL_DEFINITION,
            b"model bytes",
            mode,
            SetupContext::from_declaration(b"setup"),
            4096,
        )
        .unwrap();
        out
    }

    #[test]
    fn digest_matches_independent_xxhash_and_binds_prefix() {
        let bytes = frame(ObjectIntegrity::Objects);
        let end = bytes.len() - WIDTH;
        let ctx = SetupContext::from_declaration(b"setup");
        let mut expected = b"IMI/v4/object\0".to_vec();
        expected.extend_from_slice(ctx.as_bytes());
        expected.extend_from_slice(&bytes[..end]);
        assert_eq!(
            &bytes[end..],
            xxhash_rust::xxh3::xxh3_128(&expected).to_le_bytes()
        );
        let mut setup = b"IMI/v4/setup\0".to_vec();
        setup.extend_from_slice(b"setup");
        assert_eq!(
            ctx.as_bytes(),
            &xxhash_rust::xxh3::xxh3_128(&setup).to_le_bytes()
        );
    }

    #[test]
    fn complete_object_is_available_before_next_object() {
        for mode in [
            ObjectIntegrity::None,
            ObjectIntegrity::Objects,
            ObjectIntegrity::ObjectsAndTiles,
        ] {
            let bytes = frame(mode);
            let session = DecodeSession::new(DecodePolicy::default());
            let reader = ObjectReader::new(
                mode,
                SetupContext::from_declaration(b"setup"),
                &session,
                4096,
            );
            let object = reader.read(&bytes).unwrap();
            assert_eq!(object.body, b"model bytes");
            assert_eq!(object.encoded_len, bytes.len());
            assert_eq!(object.integrity_verified, mode != ObjectIntegrity::None);
            // Model definitions consume input/work; the unit cap counts
            // reconstruction groups, including their numeric dependencies.
            assert_eq!(session.usage().units, 0);
            assert_eq!(session.usage().input_bytes, bytes.len() as u64);
        }
    }

    #[test]
    fn mutations_truncation_and_wrong_context_never_verify() {
        let bytes = frame(ObjectIntegrity::Objects);
        for n in 0..bytes.len() {
            let session = DecodeSession::new(DecodePolicy::default());
            let reader = ObjectReader::new(
                ObjectIntegrity::Objects,
                SetupContext::from_declaration(b"setup"),
                &session,
                4096,
            );
            assert!(reader.read(&bytes[..n]).is_err());
            let mut changed = bytes.clone();
            changed[n] ^= 1;
            assert!(reader.read(&changed).is_err(), "mutation at {n}");
        }
        let session = DecodeSession::new(DecodePolicy::default());
        let reader = ObjectReader::new(
            ObjectIntegrity::Objects,
            SetupContext::from_declaration(b"other setup"),
            &session,
            4096,
        );
        assert!(reader.read(&bytes).is_err());
    }

    #[test]
    fn limits_charge_tiny_objects_and_corrupt_input_cumulatively() {
        let bytes = frame(ObjectIntegrity::Objects);
        let probe = DecodeSession::new(DecodePolicy::default());
        let probe_reader = ObjectReader::new(
            ObjectIntegrity::Objects,
            SetupContext::from_declaration(b"setup"),
            &probe,
            4096,
        );
        probe_reader.read(&bytes).unwrap();
        let session = DecodeSession::new(
            DecodePolicy::default().with_max_work_units(probe.usage().work_units),
        );
        let reader = ObjectReader::new(
            ObjectIntegrity::Objects,
            SetupContext::from_declaration(b"setup"),
            &session,
            4096,
        );
        let mut changed = bytes.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert!(reader.read(&changed).is_err());
        assert_eq!(session.usage().input_bytes, bytes.len() as u64);
        assert!(matches!(reader.read(&bytes), Err(Error::TooLarge)));

        let session = DecodeSession::new(
            DecodePolicy::default().with_max_input_bytes(bytes.len() as u64 - 1),
        );
        let reader = ObjectReader::new(
            ObjectIntegrity::Objects,
            SetupContext::from_declaration(b"setup"),
            &session,
            4096,
        );
        assert!(matches!(reader.read(&bytes), Err(Error::TooLarge)));
    }

    #[test]
    fn cancellation_and_output_limit_precede_mutation() {
        let bytes = frame(ObjectIntegrity::Objects);
        let session = DecodeSession::with_cancel(DecodePolicy::default(), || true);
        let reader = ObjectReader::new(
            ObjectIntegrity::Objects,
            SetupContext::from_declaration(b"setup"),
            &session,
            4096,
        );
        assert!(matches!(reader.read(&bytes), Err(Error::Cancelled)));
        assert_eq!(session.usage().input_bytes, 0);
        let mut output = vec![1, 2, 3];
        assert!(
            append_object(
                &mut output,
                crate::registry::MODEL_DEFINITION,
                b"data",
                ObjectIntegrity::Objects,
                SetupContext::from_declaration(b"setup"),
                1
            )
            .is_err()
        );
        assert_eq!(output, [1, 2, 3]);
    }

    #[test]
    fn image_context_uses_spec_field_order_and_effective_sample_depths() {
        let mut info =
            crate::ImageInfo::new(123, 97, crate::Channels::Rgba, crate::SampleType::U16);
        let setup = [1, 2, 3];
        let context =
            SetupContext::from_image_declaration(&info, crate::Coding::Lossless, None, &setup)
                .unwrap();
        let mut declaration = vec![123, 97, 0, 4, 1, 1, 13, 0, 1, 0, 16, 16, 16, 16, 3];
        declaration.extend_from_slice(&setup);
        assert_eq!(context, SetupContext::from_declaration(&declaration));
        info.significant_bits = [16; 4];
        assert_eq!(
            context,
            SetupContext::from_image_declaration(&info, crate::Coding::Lossless, None, &setup)
                .unwrap()
        );
        info.significant_bits = [12; 4];
        assert_ne!(
            context,
            SetupContext::from_image_declaration(&info, crate::Coding::Lossless, None, &setup)
                .unwrap()
        );
        assert!(
            SetupContext::from_image_declaration(
                &info,
                crate::Coding::Lossless,
                Some(crate::LossyColor::Xyb),
                &setup
            )
            .is_err()
        );
    }

    #[test]
    fn overlong_and_overflowing_frame_lengths_are_rejected() {
        let session = DecodeSession::new(DecodePolicy::default());
        let reader = ObjectReader::new(
            ObjectIntegrity::None,
            SetupContext::from_declaration(b"setup"),
            &session,
            4096,
        );
        assert!(reader.read(&[0x80, 0, 0]).is_err());
        assert!(
            reader
                .read(&[1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 1])
                .is_err()
        );
        assert!(ObjectIntegrity::try_from(3).is_err());
    }
}

#[cfg(all(feature = "lab", feature = "std"))]
fn trace_body(
    checks: Option<&super::OwnedChecks>,
    kind: u64,
    len: usize,
    start: crate::trace::Mark,
) {
    use super::CheckScope;
    use crate::registry::{UNIT_AC, UNIT_STRIPE};
    use crate::trace::{exit_addr, trace_enter_addr};

    let (pixel_label, pixel_width, encoded_width) = checks.map_or(("", 0, 0), |checks| {
        let descriptors = checks.get();
        let (label, pixel) = match kind {
            UNIT_STRIPE | UNIT_AC => (
                "group pixel check results",
                descriptors.list(CheckScope::GroupPixels).result_bytes(),
            ),
            super::tile::TILE_END => (
                "tile pixel check results",
                descriptors.list(CheckScope::TilePixels).result_bytes(),
            ),
            _ => ("", 0),
        };
        (
            label,
            pixel,
            descriptors.list(CheckScope::EncodedObject).result_bytes(),
        )
    });
    let (pixel_width, encoded_width) = if pixel_width + encoded_width <= len {
        (pixel_width, encoded_width)
    } else {
        (0, 0) // Keep a coherent span when the ordinary parser diagnoses truncation.
    };
    let encoded_start = len - encoded_width;
    let pixel_start = encoded_start - pixel_width;
    trace_enter_addr("payload", "V4:6.1", "object body", start);
    exit_addr(start.plus(pixel_start));
    if pixel_width != 0 {
        trace_enter_addr("digest", "V4:6.2", pixel_label, start.plus(pixel_start));
        exit_addr(start.plus(encoded_start));
    }
    if encoded_width != 0 {
        trace_enter_addr(
            "digest",
            "V4:6.2",
            "encoded object check results",
            start.plus(encoded_start),
        );
        exit_addr(start.plus(len));
    }
}
