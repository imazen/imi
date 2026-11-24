//! Historical color description, separate from the current rendering contract.
use crate::{ByteReader, Chunk, Error, Metadata};
use alloc::vec::Vec;
use zenpixels::{Cicp, ColorAuthority, ColorOrigin, ColorProvenance};

const TAG: [u8; 4] = *b"cORg";

impl Metadata {
    /// Read the optional historical color description (`cORg` version 1).
    ///
    /// This never overrides active [`Metadata::icc`] or [`crate::ImageInfo::cicp`].
    /// In particular, a superseded source ICC profile is not a rendering profile.
    /// Pixel decoders preserve this ancillary record without interpreting it;
    /// this accessor rejects duplicate, malformed or unsupported records.
    pub fn color_origin(&self) -> Result<Option<ColorOrigin>, Error> {
        let mut chunks = self.chunks.iter().filter(|chunk| chunk.tag == TAG);
        let Some(chunk) = chunks.next() else {
            return Ok(None);
        };
        if chunks.next().is_some() {
            return Err(Error::Corrupt("repeated color origin"));
        }
        let mut input = ByteReader::new(&chunk.data);
        if input.u8()? != 1 {
            return Err(Error::Unsupported("color origin version"));
        }
        let provenance = match input.u8()? {
            0 => ColorProvenance::Assumed,
            1 => ColorProvenance::Icc,
            2 => ColorProvenance::Cicp,
            3 => ColorProvenance::GamaChrm,
            _ => return Err(Error::Unsupported("color provenance")),
        };
        let authority = match input.u8()? {
            0 => ColorAuthority::Icc,
            1 => ColorAuthority::Cicp,
            _ => return Err(Error::Unsupported("color origin authority")),
        };
        let flags = input.varint()?;
        if flags & !3 != 0 {
            return Err(Error::Unsupported("color origin fields"));
        }
        let mut origin = ColorOrigin::assumed().with_color_authority(authority);
        origin.provenance = provenance;
        if flags & 2 != 0 {
            let bytes = input.bytes(4)?;
            if bytes[3] > 1 {
                return Err(Error::Corrupt("color origin range"));
            }
            origin.cicp = Some(Cicp::new(bytes[0], bytes[1], bytes[2], bytes[3] != 0));
        }
        if flags & 1 != 0 {
            let bytes = input.bytes(input.remaining())?;
            let mut icc = Vec::new();
            icc.try_reserve_exact(bytes.len())
                .map_err(|_| Error::TooLarge)?;
            icc.extend_from_slice(bytes);
            origin.icc = Some(icc.into());
        }
        if input.remaining() != 0 {
            return Err(Error::Corrupt("color origin trailing bytes"));
        }
        Ok(Some(origin))
    }

    /// Replace or remove historical color provenance without changing active color.
    ///
    /// Callers carrying converted pixels retain the original origin and update
    /// their active color separately. Passing `None` removes all `cORg` records.
    /// No ICC validation or color conversion is performed. An unsupported future
    /// shared enum value returns an error without changing this metadata.
    pub fn set_color_origin(&mut self, origin: Option<&ColorOrigin>) -> Result<(), Error> {
        let replacement = origin.map(serialize).transpose()?;
        if replacement.is_some() {
            self.chunks.try_reserve(1).map_err(|_| Error::TooLarge)?;
        }
        self.chunks.retain(|chunk| chunk.tag != TAG);
        if let Some(data) = replacement {
            self.chunks.push(Chunk { tag: TAG, data });
        }
        Ok(())
    }
}

fn serialize(origin: &ColorOrigin) -> Result<Vec<u8>, Error> {
    let provenance = match origin.provenance {
        ColorProvenance::Assumed => 0,
        ColorProvenance::Icc => 1,
        ColorProvenance::Cicp => 2,
        ColorProvenance::GamaChrm => 3,
        _ => return Err(Error::Unsupported("color provenance")),
    };
    let authority = match origin.color_authority {
        ColorAuthority::Icc => 0,
        ColorAuthority::Cicp => 1,
        _ => return Err(Error::Unsupported("color authority")),
    };
    let size = 4usize
        .checked_add(usize::from(origin.cicp.is_some()) * 4)
        .and_then(|n| n.checked_add(origin.icc.as_ref().map_or(0, |icc| icc.len())))
        .ok_or(Error::TooLarge)?;
    let mut data = Vec::new();
    data.try_reserve_exact(size).map_err(|_| Error::TooLarge)?;
    data.extend_from_slice(&[1, provenance, authority]);
    crate::put_varint(
        &mut data,
        u64::from(origin.icc.is_some()) | (u64::from(origin.cicp.is_some()) << 1),
    );
    if let Some(c) = origin.cicp {
        data.extend_from_slice(&[
            c.color_primaries,
            c.transfer_characteristics,
            c.matrix_coefficients,
            u8::from(c.full_range),
        ]);
    }
    if let Some(icc) = &origin.icc {
        data.extend_from_slice(icc);
    }
    Ok(data)
}
