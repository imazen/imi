//! Bounded seeded LZ4 packing for metadata and small descriptor/index records.
//!
//! This is a length-delimited IMI record, not an LZ4 frame:
//! - `0, decoded_len:canonical_ULEB128, raw_bytes`.
//! - `1, seed_id:u8 (=1), decoded_len:canonical_ULEB128, LZ4_block`.
//!
//! Method 1 fixes the standard LZ4 block grammar and immutable seed below.
//! The seed precedes output as external history; offsets are 1..=65535.
//! Final sequences contain literals only, at least five except a sole short
//! sequence; the last match starts at least twelve output bytes before EOF.
//! We additionally require a zero final token match nibble. Unknown methods,
//! unknown seeds, noncanonical lengths, trailing data and wrong output lengths
//! are errors. There is no recursive compression, dictionary download or checksum.
//!
//! Official grammar: <https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md>.
//! Implementation is original safe Rust, not copied from a reference codec.
//! Seed 1 is a hand-written collection of public metadata syntax/field names,
//! not a corpus-trained dictionary. Its bytes are part of the wire contract;
//! changing them requires a new seed ID. SHA-256: 3136211e1ae05153d1b854e5bed190c95d0cadf7a2d56504e35b889a3a4c3dde.
//!
//! Encoding uses an 8 KiB stack table of four recent candidates per hash and
//! greedy matches. At most four matches are tested per input position; a chosen
//! match advances over all its bytes. Search and update work are therefore
//! linear with bounded constants. Decoder validation is allocation-free, then
//! decoding takes O(input + output) work and one fallibly allocated output.
//! The caller sets the decoded-byte cap. Compressed output is selected only
//! when its COMPLETE record is smaller than the original unwrapped input.
//! Otherwise method 0 preserves bytes; callers can keep their existing raw
//! chunk when even that framing would be unnecessary. No performance claim.

use alloc::vec::Vec;

use crate::Error;

const SEED_ID: u8 = 1;
pub(crate) const SEED: &[u8] = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="">
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
xmlns:tiff="http://ns.adobe.com/tiff/1.0/" xmlns:exif="http://ns.adobe.com/exif/1.0/">
<dc:creator><rdf:Seq><rdf:li></rdf:li></rdf:Seq></dc:creator>
<dc:description><rdf:Alt><rdf:li xml:lang="x-default"></rdf:li></rdf:Alt></dc:description>
xmp:CreateDate xmp:ModifyDate xmp:MetadataDate xmp:CreatorTool photoshop:ICCProfile
tiff:ImageWidth tiff:ImageLength tiff:Orientation tiff:Make tiff:Model
exif:PixelXDimension exif:PixelYDimension exif:ColorSpace exif:DateTimeOriginal
</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>
acspmntrRGB XYZ descwtptbkptrXYZgXYZbXYZrTRCgTRCbTRCcprtchadmlucparaXYZ curv
channels sample width height stride offset length dataWindow displayWindow
alpha depth confidence selection normal velocity object_id color_primaries
transfer_characteristics matrix_coefficients full_range orientation
"#;

fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    out.try_reserve(bytes.len()).map_err(|_| Error::TooLarge)?;
    out.extend_from_slice(bytes);
    Ok(())
}

fn put_len(out: &mut Vec<u8>, len: usize) -> Result<(), Error> {
    out.try_reserve(10).map_err(|_| Error::TooLarge)?;
    crate::put_varint(out, len as u64);
    Ok(())
}

fn take_byte(bytes: &[u8], pos: &mut usize) -> Result<u8, Error> {
    let value = *bytes
        .get(*pos)
        .ok_or(Error::Corrupt("short metadata pack"))?;
    *pos += 1;
    Ok(value)
}

fn decoded_len(bytes: &[u8], pos: &mut usize) -> Result<usize, Error> {
    let mut value = 0u64;
    for index in 0..10 {
        let byte = take_byte(bytes, pos)?;
        if index == 9 && byte > 1 {
            return Err(Error::Corrupt("metadata pack length overflow"));
        }
        value |= u64::from(byte & 127) << (index * 7);
        if byte < 128 {
            if index != 0 && byte == 0 {
                return Err(Error::Corrupt("noncanonical metadata pack length"));
            }
            return usize::try_from(value).map_err(|_| Error::TooLarge);
        }
    }
    Err(Error::Corrupt("metadata pack length overflow"))
}

fn length(bytes: &[u8], pos: &mut usize, nibble: u8, limit: usize) -> Result<usize, Error> {
    let mut n = usize::from(nibble);
    if n > limit {
        return Err(Error::Corrupt("metadata pack output length"));
    }
    if nibble == 15 {
        loop {
            let extra = take_byte(bytes, pos)?;
            n = n.checked_add(usize::from(extra)).ok_or(Error::TooLarge)?;
            if n > limit {
                return Err(Error::Corrupt("metadata pack output length"));
            }
            if extra != 255 {
                break;
            }
        }
    }
    Ok(n)
}

/// Validates every sequence before invoking its output callback. Running once
/// with a no-op checks the complete stream before any declared-size allocation.
fn walk(
    block: &[u8],
    expected: usize,
    mut emit: impl FnMut(&[u8], Option<(usize, usize)>),
) -> Result<(), Error> {
    let mut pos = 0;
    let mut produced = 0usize;
    let mut had_match = false;
    loop {
        let token = take_byte(block, &mut pos)?;
        let literals = length(block, &mut pos, token >> 4, expected - produced)?;
        let end = pos.checked_add(literals).ok_or(Error::TooLarge)?;
        let literal_bytes = block
            .get(pos..end)
            .ok_or(Error::Corrupt("short metadata literals"))?;
        pos = end;
        produced += literals;
        if pos == block.len() {
            if produced != expected
                || token & 15 != 0
                || (literals < 5 && (had_match || expected >= 5))
            {
                return Err(Error::Corrupt("metadata pack terminal sequence"));
            }
            emit(literal_bytes, None);
            return Ok(());
        }
        let lo = take_byte(block, &mut pos)?;
        let hi = take_byte(block, &mut pos)?;
        let offset = usize::from(u16::from_le_bytes([lo, hi]));
        let history = SEED.len().checked_add(produced).ok_or(Error::TooLarge)?;
        if offset == 0 || offset > history {
            return Err(Error::Corrupt("metadata pack match offset"));
        }
        let remaining = expected - produced;
        if remaining < 12 {
            return Err(Error::Corrupt("metadata pack match near end"));
        }
        let matched = length(block, &mut pos, token & 15, remaining - 4)? + 4;
        produced += matched;
        had_match = true;
        emit(literal_bytes, Some((offset, matched)));
    }
}

fn unpack_header(bytes: &[u8]) -> Result<(u8, usize, usize), Error> {
    let mut pos = 0;
    let method = take_byte(bytes, &mut pos)?;
    match method {
        0 => {}
        1 if take_byte(bytes, &mut pos)? == SEED_ID => {}
        1 => return Err(Error::Unsupported("metadata pack dictionary")),
        _ => return Err(Error::Unsupported("metadata pack method")),
    }
    let expected = decoded_len(bytes, &mut pos)?;
    Ok((method, expected, pos))
}

/// Declared expansion for admission; grammar validation still occurs in unpack.
pub(crate) fn unpacked_len(bytes: &[u8]) -> Result<usize, Error> {
    Ok(unpack_header(bytes)?.1)
}

/// Validates one complete packed record under a decoded cap without
/// expanding it: every check [`unpack`] makes.
pub(crate) fn check(bytes: &[u8], max_len: usize) -> Result<(), Error> {
    checked(bytes, max_len).map(drop)
}

/// The record's method, decoded length and block, once its syntax is valid.
fn checked(bytes: &[u8], max_len: usize) -> Result<(u8, usize, &[u8]), Error> {
    let (method, expected, pos) = unpack_header(bytes)?;
    if expected > max_len || (method == 1 && expected > usize::MAX - SEED.len()) {
        return Err(Error::TooLarge);
    }
    let block = &bytes[pos..];
    if method == 0 {
        if block.len() != expected {
            return Err(Error::Corrupt("metadata raw length"));
        }
    } else {
        walk(block, expected, |_, _| {})?;
    }
    Ok((method, expected, block))
}

#[cfg(all(test, feature = "std", feature = "encode"))]
std::thread_local! {
    /// Records this thread expanded (tests of readers that must not).
    pub(crate) static UNPACKED: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

/// Decodes one complete packed record, enforcing a caller-owned decoded cap.
/// Invalid compressed syntax is rejected before output allocation.
pub(crate) fn unpack(bytes: &[u8], max_len: usize) -> Result<Vec<u8>, Error> {
    let (method, expected, block) = checked(bytes, max_len)?;
    expand_block(method, expected, block)
}

/// [`unpack`] of a record whose syntax [`check`] already accepted (the
/// chunk rules check every `mCMP` record a decoder reads): one walk, the
/// expansion, instead of a second validation first.
pub(crate) fn expand(bytes: &[u8], max_len: usize) -> Result<Vec<u8>, Error> {
    debug_assert!(
        check(bytes, max_len).is_ok(),
        "expand needs a checked record"
    );
    let (method, expected, pos) = unpack_header(bytes)?;
    if expected > max_len || (method == 1 && expected > usize::MAX - SEED.len()) {
        return Err(Error::TooLarge);
    }
    let block = &bytes[pos..];
    if method == 0 && block.len() != expected {
        return Err(Error::Corrupt("metadata raw length"));
    }
    expand_block(method, expected, block)
}

/// Expands a record's block (`method`, `expected` decoded bytes).
fn expand_block(method: u8, expected: usize, block: &[u8]) -> Result<Vec<u8>, Error> {
    #[cfg(all(test, feature = "std", feature = "encode"))]
    UNPACKED.with(|n| n.set(n.get() + 1));
    let mut out = Vec::new();
    out.try_reserve_exact(expected)
        .map_err(|_| Error::TooLarge)?;
    if method == 0 {
        out.extend_from_slice(block);
    } else {
        walk(block, expected, |literals, matched| {
            out.extend_from_slice(literals);
            if let Some((offset, count)) = matched {
                // Sequential copies deliberately permit overlapping matches,
                // including a match that crosses from dictionary into output.
                let start = SEED.len() + out.len() - offset;
                for source in start..start + count {
                    let byte = if source < SEED.len() {
                        SEED[source]
                    } else {
                        out[source - SEED.len()]
                    };
                    out.push(byte);
                }
            }
        })?;
    }
    Ok(out)
}

fn extend_length(out: &mut Vec<u8>, mut n: usize) {
    while n >= 255 {
        out.push(255);
        n -= 255;
    }
    out.push(n as u8);
}

fn emit_sequence(
    out: &mut Vec<u8>,
    literals: &[u8],
    matched: Option<(usize, usize)>,
    limit: usize,
) -> Result<bool, Error> {
    let lit_extra = if literals.len() >= 15 {
        (literals.len() - 15) / 255 + 1
    } else {
        0
    };
    let match_len = matched.map_or(0, |(_, n)| n - 4);
    let match_extra = if match_len >= 15 {
        (match_len - 15) / 255 + 1
    } else {
        0
    };
    let size = 1usize
        .checked_add(lit_extra)
        .and_then(|v| v.checked_add(literals.len()))
        .and_then(|v| {
            v.checked_add(if matched.is_some() {
                2 + match_extra
            } else {
                0
            })
        })
        .ok_or(Error::TooLarge)?;
    if out.len().checked_add(size).ok_or(Error::TooLarge)? >= limit {
        return Ok(false);
    }
    out.try_reserve(size).map_err(|_| Error::TooLarge)?;
    out.push((literals.len().min(15) as u8) << 4 | match_len.min(15) as u8);
    if literals.len() >= 15 {
        extend_length(out, literals.len() - 15);
    }
    out.extend_from_slice(literals);
    if let Some((offset, _)) = matched {
        out.extend_from_slice(&(offset as u16).to_le_bytes());
        if match_len >= 15 {
            extend_length(out, match_len - 15);
        }
    }
    Ok(true)
}

fn history(input: &[u8], at: usize) -> u8 {
    if at < SEED.len() {
        SEED[at]
    } else {
        input[at - SEED.len()]
    }
}

fn hash(input: &[u8], at: usize) -> usize {
    let word = u32::from_le_bytes(core::array::from_fn(|i| history(input, at + i)));
    (word.wrapping_mul(2_654_435_761) >> 23) as usize
}

fn insert(table: &mut [[u32; 4]; 512], input: &[u8], at: usize) {
    let bucket = &mut table[hash(input, at)];
    bucket.copy_within(..3, 1);
    bucket[0] = at as u32;
}

fn compressed(input: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let mut table = [[u32::MAX; 4]; 512];
    for at in 0..SEED.len() - 3 {
        insert(&mut table, input, at);
    }
    let mut out = Vec::new();
    append(&mut out, &[1, SEED_ID])?;
    put_len(&mut out, input.len())?;
    let (mut at, mut anchor) = (0usize, 0usize);
    while at <= input.len() - 12 {
        let absolute = SEED.len() + at;
        let candidates = table[hash(input, absolute)];
        let (mut best_len, mut best_offset) = (0, 0);
        for previous in candidates {
            if previous == u32::MAX {
                continue;
            }
            let previous = previous as usize;
            let offset = absolute - previous;
            if offset > u16::MAX as usize {
                continue;
            }
            let mut n = 0;
            while n < input.len() - at - 5 && history(input, previous + n) == input[at + n] {
                n += 1;
            }
            if n >= 4 && n > best_len {
                best_len = n;
                best_offset = offset;
            }
        }
        if best_len < 4 {
            insert(&mut table, input, absolute);
            at += 1;
            continue;
        }
        if !emit_sequence(
            &mut out,
            &input[anchor..at],
            Some((best_offset, best_len)),
            input.len(),
        )? {
            return Ok(None);
        }
        let end = at + best_len;
        for p in at..end {
            insert(&mut table, input, SEED.len() + p);
        }
        at = end;
        anchor = end;
    }
    if !emit_sequence(&mut out, &input[anchor..], None, input.len())? {
        return Ok(None);
    }
    Ok(Some(out))
}

/// Deterministic seeded compression with a complete-record raw fallback.
pub(crate) fn pack(input: &[u8]) -> Result<Vec<u8>, Error> {
    if input.len() >= 12
        && input.len() <= u32::MAX as usize - SEED.len()
        && let Some(out) = compressed(input)?
    {
        return Ok(out);
    }
    pack_stored(input)
}

/// [`pack`]'s stored form: its size depends only on the input's length.
pub(crate) fn pack_stored(input: &[u8]) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    append(&mut out, &[0])?;
    put_len(&mut out, input.len())?;
    append(&mut out, input)?;
    Ok(out)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use alloc::vec;

    fn compressed_record(expected: usize, block: &[u8]) -> Vec<u8> {
        let mut out = vec![1, SEED_ID];
        put_len(&mut out, expected).unwrap();
        out.extend_from_slice(block);
        out
    }

    fn check(input: &[u8]) -> Vec<u8> {
        let encoded = pack(input).unwrap();
        assert_eq!(unpack(&encoded, input.len()).unwrap(), input);
        assert_eq!(pack(input).unwrap(), encoded);
        if !input.is_empty() {
            assert_eq!(unpack(&encoded, input.len() - 1), Err(Error::TooLarge));
        }
        if encoded[0] == 1 {
            assert!(encoded.len() < input.len());
        }
        encoded
    }

    #[test]
    fn binary_text_float_and_window_roundtrips() {
        for size in [
            0, 1, 4, 5, 11, 12, 13, 15, 19, 255, 270, 4096, 65535, 65536, 131073,
        ] {
            check(&vec![0; size]);
            check(&(0..size).map(|i| (i % 251) as u8).collect::<Vec<_>>());
            let mut state = 0x3141_5926u32;
            let random: Vec<_> = (0..size)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect();
            let packed = check(&random);
            if size < 4096 {
                assert_eq!(packed[0], 0);
            }
        }
        let floats: Vec<_> = [
            0u32,
            0x8000_0000,
            0x7f80_0000,
            0xff80_0000,
            0x7fc0_0001,
            0x7fff_ffff,
            0x3f80_0000,
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .cycle()
        .take(8192)
        .collect();
        check(&floats);
        let xmp = SEED.repeat(3);
        assert_eq!(check(&xmp)[0], 1);
    }

    #[test]
    fn independently_constructed_seed_and_overlap_blocks() {
        let mut block = vec![0x0f];
        block.extend_from_slice(&(SEED.len() as u16).to_le_bytes());
        block.extend_from_slice(&[45, 0x50]);
        block.extend_from_slice(b"tail!");
        let mut expected = SEED[..64].to_vec();
        expected.extend_from_slice(b"tail!");
        assert_eq!(
            unpack(&compressed_record(69, &block), 69).unwrap(),
            expected
        );
        let block = [0x1c, b'A', 1, 0, 0x50, b't', b'a', b'i', b'l', b'!'];
        let mut expected = vec![b'A'; 17];
        expected.extend_from_slice(b"tail!");
        assert_eq!(
            unpack(&compressed_record(22, &block), 22).unwrap(),
            expected
        );
        // A dictionary match can cross its end into output already produced by
        // the same overlapping match. Seed ends in newline, so offset1 repeats it.
        let block = [0x0f, 1, 0, 1, 0x50, b't', b'a', b'i', b'l', b'!'];
        let mut expected = vec![b'\n'; 20];
        expected.extend_from_slice(b"tail!");
        assert_eq!(
            unpack(&compressed_record(25, &block), 25).unwrap(),
            expected
        );
    }

    #[test]
    fn every_truncation_and_appended_junk_is_rejected() {
        for original in [b"short raw".to_vec(), SEED.repeat(2), vec![0; 8192]] {
            let packed = check(&original);
            for end in 0..packed.len() {
                assert!(unpack(&packed[..end], original.len()).is_err(), "cut {end}");
            }
            for suffix in [&[0][..], &[1, 2][..], &[0x50, 1, 2, 3, 4, 5][..]] {
                let mut extra = packed.clone();
                extra.extend_from_slice(suffix);
                assert!(unpack(&extra, original.len()).is_err());
            }
        }
    }

    #[test]
    fn rejects_bad_ids_lengths_offsets_and_terminal_rules() {
        for bytes in [
            vec![],
            vec![2, 0],
            vec![1, 2, 0, 0],
            vec![1, 0, 0, 0],
            vec![0, 0x80, 0],
            vec![0, 0x81, 0, 7],
            vec![0, 1],
            vec![0, 0, 7],
            vec![1, 1, 0],
        ] {
            assert!(unpack(&bytes, 100).is_err(), "{bytes:?}");
        }
        let mut overflow = vec![0];
        overflow.extend_from_slice(&[255; 10]);
        assert!(unpack(&overflow, usize::MAX).is_err());
        for block in [
            &[0, 0, 0, 0x50, 1, 2, 3, 4, 5][..],     // zero offset
            &[0, 255, 255, 0x50, 1, 2, 3, 4, 5][..], // before dictionary
            &[0x0f, 1, 0, 255, 0][..],               // output expansion past declared length
            &[0xf0, 255, 255][..],                   // unterminated/excessive literals
            &[0, 1][..],                             // truncated offset
            &[0x0f, 1, 0][..],                       // missing match extension
            &[0x01][..],                             // final token has unused match bits
        ] {
            assert!(
                unpack(&compressed_record(32, block), 32).is_err(),
                "{block:?}"
            );
        }
        assert!(unpack(&compressed_record(9, &[0, 1, 0, 0x50, 1, 2, 3, 4, 5]), 9).is_err());
        assert!(unpack(&compressed_record(0, &[0x01]), 0).is_err());
        assert_eq!(unpack(&compressed_record(0, &[0]), 0).unwrap(), [0u8; 0]);
        for n in 1..=5 {
            let mut block = vec![(n as u8) << 4];
            block.extend_from_slice(&b"hello"[..n]);
            assert_eq!(
                unpack(&compressed_record(n, &block), n).unwrap(),
                &b"hello"[..n]
            );
        }
    }

    #[test]
    fn mutated_records_stay_bounded_without_panics() {
        let input = SEED.repeat(2);
        let packed = pack(&input).unwrap();
        for at in 0..packed.len() {
            for bit in 0..8 {
                let mut bytes = packed.clone();
                bytes[at] ^= 1 << bit;
                if let Ok(output) = unpack(&bytes, input.len()) {
                    // Valid mutations need not preserve bytes: this format
                    // promises bounded decoding, not an integrity checksum.
                    assert!(output.len() <= input.len());
                }
            }
        }
    }
}
