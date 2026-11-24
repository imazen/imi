//! Compact restart states for a single run-level or packed rANS record.
//! (Packed mode is retired; only run-level files carry compact records.)
//!
//! The enclosing stream supplies the record length and active lane count.
//! Tag 0 stores the original record; tag 1 uses implicit `RANS_L` states;
//! tag 2 supplies a common little-endian u32 state. Both compact tags then
//! store a little-endian exception bitmap (`ceil(lanes / 8)` bytes), explicit
//! states in lane order for set bits, and the unchanged 16-bit rANS payload.
//! Unused bitmap bits must be zero. Decoding accepts valid nonminimal forms.
//! Expansion needs at most 61 extra bytes, independent of image dimensions.

#[cfg(feature = "encode")]
use alloc::vec::Vec;

use crate::Error;
use crate::rans::{NL, RANS_L};

fn check_lanes(lanes: usize) -> Result<(), Error> {
    if !(1..=NL).contains(&lanes) {
        return Err(Error::Corrupt("compact rANS lane count"));
    }
    Ok(())
}

fn word(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(Error::Corrupt("short compact rANS states"))?;
    let value = u32::from_le_bytes(value.try_into().unwrap());
    if value < RANS_L {
        return Err(Error::Corrupt("bad initial state"));
    }
    Ok(value)
}

fn raw_states(bytes: &[u8], lanes: usize) -> Result<[u32; NL], Error> {
    check_lanes(lanes)?;
    if bytes.len() < lanes * 4 || !(bytes.len() - lanes * 4).is_multiple_of(2) {
        return Err(Error::Corrupt("short lane stream"));
    }
    let mut states = [RANS_L; NL];
    for (lane, state) in states.iter_mut().enumerate().take(lanes) {
        *state = word(bytes, lane * 4)?;
    }
    Ok(states)
}

#[cfg(feature = "encode")]
fn plan(bytes: &[u8], lanes: usize) -> Result<(u8, u32, usize), Error> {
    let states = raw_states(bytes, lanes)?;
    let states = &states[..lanes];
    let mask_len = lanes.div_ceil(8);
    let mut tag = 0;
    let mut common = RANS_L;
    let mut head_len = 1 + lanes * 4;
    let implicit_len = 1 + mask_len + 4 * states.iter().filter(|&&s| s != RANS_L).count();
    if implicit_len < head_len {
        tag = 1;
        head_len = implicit_len;
    }
    for &candidate in states {
        let len = 5 + mask_len + 4 * states.iter().filter(|&&s| s != candidate).count();
        if len < head_len {
            tag = 2;
            common = candidate;
            head_len = len;
        }
    }
    Ok((tag, common, head_len))
}

/// Exact encoded length without copying the entropy payload.
#[cfg(feature = "encode")]
pub(crate) fn packed_len(bytes: &[u8], lanes: usize) -> Result<usize, Error> {
    let (_, _, head) = plan(bytes, lanes)?;
    head.checked_add(bytes.len() - 4 * lanes)
        .ok_or(Error::TooLarge)
}

/// Selects the shortest complete record. Ties prefer raw, then implicit
/// `RANS_L`, then the first modal value in lane order. Work is bounded by
/// 16 lanes; no histogram allocation or dictionary is needed.
#[cfg(feature = "encode")]
pub(crate) fn pack(bytes: &[u8], lanes: usize) -> Result<Vec<u8>, Error> {
    let (tag, common, head_len) = plan(bytes, lanes)?;
    let states = raw_states(bytes, lanes)?;
    let states = &states[..lanes];
    let mask_len = lanes.div_ceil(8);
    let payload = &bytes[lanes * 4..];
    let len = head_len.checked_add(payload.len()).ok_or(Error::TooLarge)?;
    let mut out = Vec::new();
    out.try_reserve_exact(len).map_err(|_| Error::TooLarge)?;
    out.push(tag);
    if tag == 0 {
        out.extend_from_slice(bytes);
        return Ok(out);
    }
    if tag == 2 {
        out.extend_from_slice(&common.to_le_bytes());
    }
    let mut mask = 0u16;
    for (lane, &state) in states.iter().enumerate() {
        if state != common {
            mask |= 1 << lane;
        }
    }
    out.extend_from_slice(&mask.to_le_bytes()[..mask_len]);
    for &state in states {
        if state != common {
            out.extend_from_slice(&state.to_le_bytes());
        }
    }
    out.extend_from_slice(payload);
    Ok(out)
}

/// Restores initial states on the stack and borrows the unchanged entropy
/// payload. No record copy or allocation is needed, including compact forms.
/// Payload validity beyond word alignment is checked by the rANS decoder.
pub(crate) fn states(bytes: &[u8], lanes: usize) -> Result<([u32; NL], &[u8]), Error> {
    check_lanes(lanes)?;
    let (&tag, remainder) = bytes
        .split_first()
        .ok_or(Error::Corrupt("short compact rANS record"))?;
    if tag == 0 {
        return Ok((raw_states(remainder, lanes)?, &remainder[lanes * 4..]));
    }
    let (common, mut pos) = match tag {
        1 => (RANS_L, 1),
        2 => (word(bytes, 1)?, 5),
        _ => return Err(Error::Corrupt("compact rANS tag")),
    };
    let mask_len = lanes.div_ceil(8);
    let mask_bytes = bytes
        .get(pos..pos + mask_len)
        .ok_or(Error::Corrupt("short compact rANS bitmap"))?;
    let mask = u16::from_le_bytes([mask_bytes[0], *mask_bytes.get(1).unwrap_or(&0)]);
    if u32::from(mask) >> lanes != 0 {
        return Err(Error::Corrupt("compact rANS bitmap tail"));
    }
    pos += mask_len;
    let mut states = [RANS_L; NL];
    states[..lanes].fill(common);
    for (lane, state) in states.iter_mut().enumerate().take(lanes) {
        if mask & (1 << lane) != 0 {
            *state = word(bytes, pos)?;
            pos += 4;
        }
    }
    let payload = &bytes[pos..];
    if !payload.len().is_multiple_of(2) {
        return Err(Error::Corrupt("odd compact rANS payload"));
    }
    Ok((states, payload))
}

/// Materializes the legacy record for exact-restoration tests only. The
/// production decoder consumes `states` directly without heap scratch.
#[cfg(all(test, feature = "encode"))]
fn unpack<'a>(bytes: &'a [u8], lanes: usize, scratch: &'a mut Vec<u8>) -> Result<&'a [u8], Error> {
    let (states, payload) = states(bytes, lanes)?;
    if bytes[0] == 0 {
        return Ok(&bytes[1..]);
    }
    let len = (lanes * 4)
        .checked_add(payload.len())
        .ok_or(Error::TooLarge)?;
    scratch.clear();
    scratch.try_reserve(len).map_err(|_| Error::TooLarge)?;
    for state in &states[..lanes] {
        scratch.extend_from_slice(&state.to_le_bytes());
    }
    scratch.extend_from_slice(payload);
    Ok(scratch)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use alloc::vec;

    fn raw(states: &[u32], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for state in states {
            out.extend_from_slice(&state.to_le_bytes());
        }
        out.extend_from_slice(payload);
        out
    }

    fn roundtrip(states: &[u32], payload: &[u8]) -> Vec<u8> {
        let bytes = raw(states, payload);
        let packed = pack(&bytes, states.len()).unwrap();
        assert!(packed.len() <= bytes.len() + 1);
        let (restored, body) = super::states(&packed, states.len()).unwrap();
        assert_eq!(&restored[..states.len()], states);
        assert!(restored[states.len()..].iter().all(|&s| s == RANS_L));
        assert_eq!(body, payload);
        assert_eq!(
            body.as_ptr(),
            packed[packed.len() - payload.len()..].as_ptr()
        );
        let mut scratch = vec![0xaa; 123];
        assert_eq!(unpack(&packed, states.len(), &mut scratch).unwrap(), bytes);
        assert_eq!(pack(&bytes, states.len()).unwrap(), packed);
        packed
    }

    #[test]
    fn every_lane_count_and_exception_position() {
        for lanes in 1..=NL {
            for payload in [&[][..], &[0x12, 0x34, 0xff, 0x00][..]] {
                for common in [RANS_L, RANS_L + 1, 0x1234_5678, u32::MAX] {
                    let mut states = vec![common; lanes];
                    roundtrip(&states, payload);
                    for lane in 0..lanes {
                        states[lane] = common ^ 0x0123;
                        roundtrip(&states, payload);
                        states[lane] = common;
                    }
                }
                let states: Vec<_> = (0..lanes).map(|lane| RANS_L + lane as u32 + 1).collect();
                assert_eq!(roundtrip(&states, payload)[0], 0);
            }
        }
    }

    #[test]
    fn exact_wire_forms_and_zero_copy_escape() {
        assert_eq!(roundtrip(&[RANS_L; 16], &[]), [1, 0, 0]);
        let mut states = [RANS_L; 16];
        states[9] = 0x1234_5678;
        assert_eq!(
            roundtrip(&states, &[0xab, 0xcd]),
            [1, 0, 2, 0x78, 0x56, 0x34, 0x12, 0xab, 0xcd]
        );
        assert_eq!(
            roundtrip(&[0x1234_5678; 16], &[]),
            [2, 0x78, 0x56, 0x34, 0x12, 0, 0]
        );
        let mut bytes = vec![0];
        bytes.extend_from_slice(&raw(&[RANS_L, u32::MAX], &[1, 2]));
        let mut scratch = vec![42];
        let restored = unpack(&bytes, 2, &mut scratch).unwrap();
        assert_eq!(restored.as_ptr(), bytes[1..].as_ptr());
        assert_eq!(scratch, [42]);
    }

    #[test]
    fn malformed_records_are_rejected_before_scratch_changes() {
        let mut bad = vec![vec![], vec![3], vec![255], vec![1], vec![2, 0, 0, 1]];
        // Explicit state below RANS_L, missing state, and odd payload.
        bad.extend([
            vec![1, 1, 0, 0, 0, 0],
            vec![1, 1],
            vec![1, 0, 9],
            vec![2, 0, 0, 0, 0, 0],
        ]);
        bad.push(vec![0, 0, 0, 0, 0]);
        bad.push(vec![0, 0, 0, 1, 0, 9]);
        for bytes in bad {
            let mut scratch = vec![42];
            assert!(unpack(&bytes, 1, &mut scratch).is_err(), "{bytes:?}");
            assert_eq!(scratch, [42]);
        }
        for lanes in 1..NL {
            if lanes.is_multiple_of(8) {
                continue;
            }
            let mut bytes = vec![1];
            let mask = (1u16 << lanes).to_le_bytes();
            bytes.extend_from_slice(&mask[..lanes.div_ceil(8)]);
            assert!(unpack(&bytes, lanes, &mut Vec::new()).is_err());
        }
        for lanes in [0, NL + 1, usize::MAX] {
            assert!(unpack(&[1, 0, 0], lanes, &mut Vec::new()).is_err());
            assert!(pack(&[], lanes).is_err());
        }
    }

    #[test]
    fn every_state_header_truncation_is_rejected() {
        for lanes in 1..=NL {
            // Nonminimal compact forms deliberately exercise every mask bit
            // and all explicit states, including valid maximum u32 states.
            for tag in 0..=2 {
                let mut bytes = vec![tag];
                if tag == 2 {
                    bytes.extend_from_slice(&RANS_L.to_le_bytes());
                }
                if tag != 0 {
                    let mask = ((1u32 << lanes) - 1) as u16;
                    bytes.extend_from_slice(&mask.to_le_bytes()[..lanes.div_ceil(8)]);
                }
                bytes.extend_from_slice(&raw(&vec![u32::MAX; lanes], &[]));
                for end in 0..bytes.len() {
                    assert!(unpack(&bytes[..end], lanes, &mut Vec::new()).is_err());
                }
                assert_eq!(
                    unpack(&bytes, lanes, &mut Vec::new()).unwrap(),
                    raw(&vec![u32::MAX; lanes], &[])
                );
            }
        }
    }
}
