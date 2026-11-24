//! Mutable directory placement is outside immutable coding setup.

use alloc::vec::Vec;

use crate::decode_resources::DecodeSession;
use crate::{ByteReader, Error, put_varint, varint_len};

/// Directory location after the immutable length-delimited DATA setup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum DirectoryPlacement {
    #[default]
    None,
    Tail,
    Front,
}

impl DirectoryPlacement {
    pub const fn wire_value(self) -> u64 {
        match self {
            Self::None => 0,
            Self::Tail => 1,
            Self::Front => 2,
        }
    }
}

impl TryFrom<u64> for DirectoryPlacement {
    type Error = Error;
    fn try_from(value: u64) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Tail),
            2 => Ok(Self::Front),
            _ => Err(Error::Unsupported("V4 directory placement")),
        }
    }
}

/// Borrowed serialized placement. An empty front reservation is provisional;
/// complete-file validation requires its final nonempty directory. Parsing this
/// prefix checks encoding/padding only, not directory entries or physical objects.
#[derive(Clone, Copy, Debug)]
pub struct DirectoryPrefix<'a> {
    pub placement: DirectoryPlacement,
    /// Complete prefix bytes consumed, including front reservation padding.
    pub encoded_len: usize,
    /// Front reservation capacity, including its index-length integer.
    pub reserved_bytes: u64,
    /// Active front directory bytes; None for other placements or an empty slot.
    pub front: Option<&'a [u8]>,
    /// Byte offset of the active front body relative to this placement prefix.
    pub front_offset: usize,
}

impl<'a> DirectoryPrefix<'a> {
    pub fn read<C: Fn() -> bool + Sync>(
        data: &'a [u8],
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        session.adapter_work(1)?;
        let mut r = ByteReader::new(data);
        crate::trace::trace_enter_addr("directory prefix", "V4:6", "directory placement", r.mark());
        let placement =
            DirectoryPlacement::try_from(crate::trace::tf!("placement", r, r.varint()?))?;
        if placement != DirectoryPlacement::Front {
            session.v4_input(r.pos)?;
            crate::trace::exit_addr(r.mark());
            return Ok(Self {
                placement,
                encoded_len: r.pos,
                reserved_bytes: 0,
                front: None,
                front_offset: 0,
            });
        }
        let capacity = crate::trace::tf!("front directory capacity", r, r.varint()?);
        if capacity == 0 {
            return Err(Error::Corrupt("empty V4 directory reservation capacity"));
        }
        let capacity_usize = usize::try_from(capacity).map_err(|_| Error::TooLarge)?;
        let total = r.pos.checked_add(capacity_usize).ok_or(Error::TooLarge)?;
        let slot = r.bytes(capacity_usize)?;
        session.v4_input(total)?;
        let mut content = ByteReader::new(slot);
        let length = crate::trace::tf!("active directory bytes", content, content.var_usize()?);
        let front_offset = total - capacity_usize + content.pos;
        let body = content.bytes(length)?;
        for padding in slot[content.pos..].chunks(64 * 1024) {
            session.adapter_work(padding.len() as u128 + 1)?;
            if padding.iter().any(|&byte| byte != 0) {
                return Err(Error::Corrupt("V4 directory reservation padding"));
            }
        }
        if content.pos < slot.len() {
            crate::trace::trace_enter_addr(
                "padding",
                "V4:6",
                "reserved zero padding",
                content.mark(),
            );
            crate::trace::exit_addr(r.mark());
        }
        crate::trace::exit_addr(r.mark());
        Ok(Self {
            placement,
            encoded_len: total,
            reserved_bytes: capacity,
            front: (!body.is_empty()).then_some(body),
            front_offset,
        })
    }
}

/// Append a placement prefix without altering existing bytes on admission or
/// allocation failure. Front capacity includes its canonical index-length field,
/// directory and zero padding. None/Tail require zero capacity and no index.
/// Seekable adapters may write a front reservation in bounded chunks instead of
/// collecting it; the wire is identical. Forward-only adapters use Tail.
pub fn append_directory_prefix(
    out: &mut Vec<u8>,
    placement: DirectoryPlacement,
    capacity: u64,
    front: Option<&[u8]>,
    max_prefix_bytes: u64,
) -> Result<(), Error> {
    let prefix = varint_len(placement.wire_value());
    let total = if placement == DirectoryPlacement::Front {
        let used = front.map_or(0, <[u8]>::len) as u64;
        let used = used
            .checked_add(varint_len(used) as u64)
            .ok_or(Error::TooLarge)?;
        if capacity == 0 || used > capacity {
            return Err(Error::InvalidInput(
                "V4 directory exceeds front reservation",
            ));
        }
        (prefix as u64)
            .checked_add(varint_len(capacity) as u64)
            .and_then(|n| n.checked_add(capacity))
            .ok_or(Error::TooLarge)?
    } else {
        if capacity != 0 || front.is_some() {
            return Err(Error::InvalidInput(
                "V4 directory data without front placement",
            ));
        }
        prefix as u64
    };
    if total > max_prefix_bytes {
        return Err(Error::TooLarge);
    }
    let total = usize::try_from(total).map_err(|_| Error::TooLarge)?;
    let end = out.len().checked_add(total).ok_or(Error::TooLarge)?;
    out.try_reserve_exact(total).map_err(|_| Error::TooLarge)?;
    put_varint(out, placement.wire_value());
    if placement == DirectoryPlacement::Front {
        put_varint(out, capacity);
        let bytes = front.unwrap_or_default();
        put_varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
        out.resize(end, 0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode_resources::DecodePolicy;
    use alloc::vec;

    #[test]
    fn placement_is_one_varint_and_front_slot_is_canonical() {
        for (mode, expected) in [
            (DirectoryPlacement::None, vec![0]),
            (DirectoryPlacement::Tail, vec![1]),
        ] {
            let mut out = Vec::new();
            append_directory_prefix(&mut out, mode, 0, None, 1).unwrap();
            assert_eq!(out, expected);
            let session = DecodeSession::new(DecodePolicy::default());
            let prefix = DirectoryPrefix::read(&out, &session).unwrap();
            assert_eq!(prefix.placement, mode);
            assert_eq!(prefix.encoded_len, 1);
        }
        let mut out = Vec::new();
        append_directory_prefix(&mut out, DirectoryPlacement::Front, 8, Some(&[4, 5, 6]), 10)
            .unwrap();
        assert_eq!(out, [2, 8, 3, 4, 5, 6, 0, 0, 0, 0]);
        let session = DecodeSession::new(DecodePolicy::default());
        let prefix = DirectoryPrefix::read(&out, &session).unwrap();
        assert_eq!(prefix.front, Some(&[4, 5, 6][..]));
        assert_eq!(prefix.front_offset, 3);
        assert_eq!(prefix.encoded_len, out.len());
    }

    #[test]
    fn empty_slot_is_provisional_and_rewrite_keeps_object_start_fixed() {
        let mut initial = Vec::new();
        append_directory_prefix(&mut initial, DirectoryPlacement::Front, 256, None, 1024).unwrap();
        let session = DecodeSession::new(DecodePolicy::default());
        let prefix = DirectoryPrefix::read(&initial, &session).unwrap();
        assert!(prefix.front.is_none());
        let mut final_prefix = Vec::new();
        append_directory_prefix(
            &mut final_prefix,
            DirectoryPlacement::Front,
            256,
            Some(&[7; 128]),
            1024,
        )
        .unwrap();
        let final_prefix = DirectoryPrefix::read(&final_prefix, &session).unwrap();
        assert_eq!(prefix.encoded_len, final_prefix.encoded_len);
        assert_eq!(final_prefix.front.unwrap(), &[7; 128]);
        assert_ne!(prefix.front_offset, final_prefix.front_offset); // Minimal varint grows in the slot.
    }

    #[test]
    fn padding_lengths_limits_and_truncation_fail_without_allocation_or_output_mutation() {
        let session = DecodeSession::new(DecodePolicy::default());
        for bad in [
            &[3][..],
            &[0x80, 0],
            &[2, 0],
            &[2, 3, 4, 1, 2],
            &[2, 2, 0, 1],
        ] {
            assert!(DirectoryPrefix::read(bad, &session).is_err());
        }
        let session = DecodeSession::new(DecodePolicy::default().with_max_input_bytes(3));
        assert!(matches!(
            DirectoryPrefix::read(&[2, 8, 0, 0, 0, 0, 0, 0, 0, 0], &session),
            Err(Error::TooLarge)
        ));
        let mut out = vec![99];
        for (capacity, limit) in [(1, 1024), (8, 1)] {
            assert!(
                append_directory_prefix(
                    &mut out,
                    DirectoryPlacement::Front,
                    capacity,
                    Some(&[1, 2]),
                    limit
                )
                .is_err()
            );
            assert_eq!(out, [99]);
        }
        let cancelled = DecodeSession::with_cancel(DecodePolicy::default(), || true);
        assert!(matches!(
            DirectoryPrefix::read(&[0], &cancelled),
            Err(Error::Cancelled)
        ));
    }
}
