//! Chunk walking and record lists (SPEC sections 1, 1.1).

use crate::bits::Reader;
use crate::error::{Error, Result};

pub(crate) const SIGNATURE: [u8; 8] = [0x49, 0x4D, 0x49, 0x4A, 0x0D, 0x0A, 0x87, 0x0A];

pub(crate) const KNOWN_TAGS: [&[u8; 4]; 17] = [
    b"dWHI", b"DATA", b"iCCP", b"eXIf", b"xMPp", b"mDCv", b"cLLI", b"pRVW", b"gMAP", b"aNIM",
    b"fRAM", b"cSUM", b"cALB", b"aXES", b"cORD", b"sMDT", b"cATG",
];

fn is_once(tag: &[u8; 4]) -> bool {
    matches!(
        tag,
        b"dWHI"
            | b"DATA"
            | b"iCCP"
            | b"eXIf"
            | b"xMPp"
            | b"mDCv"
            | b"cLLI"
            | b"gMAP"
            | b"aNIM"
            | b"cSUM"
    )
}

// SPEC 1.1: one positive finite binary32 luminance, with bounded framing.
fn check_diffuse_white(data: &[u8], at: usize, framed_len: u64) -> Result<()> {
    if framed_len != 5 {
        return Err(Error::corrupt("bad dWHI length"));
    }
    let end = at
        .checked_add(4)
        .ok_or_else(|| Error::corrupt("dWHI bounds"))?;
    let bytes = data
        .get(at..end)
        .ok_or_else(|| Error::corrupt("truncated dWHI"))?;
    let bits = u32::from_be_bytes(bytes.try_into().unwrap());
    // Inspect IEEE sign/exponent/magnitude independently of the reference parser.
    if bits & 0x8000_0000 != 0 || bits == 0 || bits & 0x7f80_0000 == 0x7f80_0000 {
        return Err(Error::corrupt("bad dWHI value"));
    }
    Ok(())
}

fn valid_tag(tag: &[u8; 4]) -> bool {
    tag.iter()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_lowercase())
}

/// The chunk-level view of a file.
pub(crate) struct Packets<'a> {
    /// The DATA payload: start, and end when its chunk length is closed.
    pub(crate) data: Option<(usize, Option<usize>)>,
    pub(crate) anim: Option<&'a [u8]>,
    pub(crate) frames: Vec<&'a [u8]>,
    /// Offset where walking stopped (the caller continues from here after
    /// resolving an open DATA chunk).
    pub(crate) resumed_at: usize,
    pub(crate) seen: Vec<[u8; 4]>,
}

/// Walk chunks from `start`. Stops when an open (length 0) DATA chunk is hit.
pub(crate) fn walk_chunks(data: &[u8], start: usize) -> Result<Packets<'_>> {
    let mut packets = Packets {
        data: None,
        anim: None,
        frames: Vec::new(),
        resumed_at: start,
        seen: Vec::new(),
    };
    let mut r = Reader::new(data);
    r.pos = start;
    while r.pos < data.len() {
        if r.pos + 4 > data.len() {
            return Err(Error::corrupt("truncated chunk tag"));
        }
        let tag: [u8; 4] = data[r.pos..r.pos + 4].try_into().unwrap();
        r.pos += 4;
        if !valid_tag(&tag) {
            return Err(Error::corrupt("invalid chunk tag"));
        }
        let length = r.varint("chunk length")?;
        if tag == *b"dWHI" {
            check_diffuse_white(data, r.pos, length)?;
        }
        let known = KNOWN_TAGS.iter().any(|known| **known == tag);
        if !known {
            if tag[0].is_ascii_lowercase() {
                if length == 0 {
                    return Err(Error::corrupt("open ancillary chunk"));
                }
                r.pos += length as usize - 1;
                if r.pos > data.len() {
                    return Err(Error::corrupt("chunk overruns the file"));
                }
                continue;
            }
            return Err(Error::unsupported(format!(
                "unknown critical chunk {:?}",
                core::str::from_utf8(&tag).unwrap_or("????")
            )));
        }
        if is_once(&tag) && packets.seen.contains(&tag) {
            return Err(Error::corrupt("chunk appears twice"));
        }
        packets.seen.push(tag);
        if tag == *b"DATA" {
            if packets.data.is_some() {
                return Err(Error::corrupt("chunk appears twice"));
            }
            if length == 0 {
                packets.data = Some((r.pos, None));
                packets.resumed_at = r.pos;
                return Ok(packets);
            }
            let end = r.pos + length as usize - 1;
            if end > data.len() {
                return Err(Error::corrupt("DATA overruns the file"));
            }
            packets.data = Some((r.pos, Some(end)));
            r.pos = end;
        } else if tag == *b"aNIM" {
            if length == 0 {
                return Err(Error::corrupt("open aNIM chunk"));
            }
            let end = r.pos + length as usize - 1;
            if end > data.len() {
                return Err(Error::corrupt("aNIM overruns the file"));
            }
            packets.anim = Some(&data[r.pos..end]);
            r.pos = end;
        } else if tag == *b"fRAM" {
            if length == 0 {
                return Err(Error::corrupt("open fRAM chunk"));
            }
            let end = r.pos + length as usize - 1;
            if end > data.len() {
                return Err(Error::corrupt("fRAM overruns the file"));
            }
            packets.frames.push(&data[r.pos..end]);
            r.pos = end;
        } else {
            if length == 0 {
                return Err(Error::corrupt("open chunk"));
            }
            r.pos += length as usize - 1;
            if r.pos > data.len() {
                return Err(Error::corrupt("chunk overruns the file"));
            }
        }
    }
    Ok(packets)
}

/// Continue walking chunks after an open DATA chunk ends at `start`.
pub(crate) fn resume_chunks<'a>(
    data: &'a [u8],
    start: usize,
    packets: &mut Packets<'a>,
) -> Result<()> {
    let mut r = Reader::new(data);
    r.pos = start;
    while r.pos < data.len() {
        if r.pos + 4 > data.len() {
            return Err(Error::corrupt("truncated chunk tag"));
        }
        let tag: [u8; 4] = data[r.pos..r.pos + 4].try_into().unwrap();
        r.pos += 4;
        if !valid_tag(&tag) {
            return Err(Error::corrupt("invalid chunk tag"));
        }
        let length = r.varint("chunk length")?;
        if tag == *b"dWHI" {
            check_diffuse_white(data, r.pos, length)?;
        }
        let known = KNOWN_TAGS.iter().any(|known| **known == tag);
        if !known {
            if tag[0].is_ascii_lowercase() {
                if length == 0 {
                    return Err(Error::corrupt("open ancillary chunk"));
                }
                r.pos += length as usize - 1;
                if r.pos > data.len() {
                    return Err(Error::corrupt("chunk overruns the file"));
                }
                continue;
            }
            return Err(Error::unsupported("unknown critical chunk"));
        }
        if is_once(&tag) && packets.seen.contains(&tag) {
            return Err(Error::corrupt("chunk appears twice"));
        }
        packets.seen.push(tag);
        if tag == *b"fRAM" {
            if length == 0 {
                return Err(Error::corrupt("open fRAM chunk"));
            }
            let end = r.pos + length as usize - 1;
            if end > data.len() {
                return Err(Error::corrupt("fRAM overruns the file"));
            }
            packets.frames.push(&data[r.pos..end]);
            r.pos = end;
        } else {
            if length == 0 {
                return Err(Error::corrupt("open chunk"));
            }
            r.pos += length as usize - 1;
            if r.pos > data.len() {
                return Err(Error::corrupt("chunk overruns the file"));
            }
        }
    }
    Ok(())
}

/// Skip a frame record list (SPEC 1.3): even ids are skipped, odd ones are
/// `Unsupported`.
pub(crate) fn skip_record_list(data: &[u8]) -> Result<()> {
    let mut r = Reader::new(data);
    let mut previous: Option<u64> = None;
    while r.pos < data.len() {
        let id = r.varint("frame record id")?;
        let len = r.varint("frame record len")?;
        if let Some(prev) = previous
            && id <= prev
        {
            return Err(Error::corrupt("frame record order"));
        }
        previous = Some(id);
        if r.pos + len as usize > data.len() {
            return Err(Error::corrupt("frame record overruns its list"));
        }
        r.pos += len as usize;
        if id % 2 == 1 {
            return Err(Error::unsupported(format!("frame record id {id}")));
        }
    }
    Ok(())
}
