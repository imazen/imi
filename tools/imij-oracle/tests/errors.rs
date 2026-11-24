//! Error-class coverage for hand-built streams (SPEC: `Unsupported` vs
//! `Corrupt`). No sealed fixture is used; every stream is assembled here.

use imij_oracle::Error;

const SIG: [u8; 8] = [0x49, 0x4D, 0x49, 0x4A, 0x0D, 0x0A, 0x87, 0x0A];

fn file(header: &[u8]) -> Vec<u8> {
    let mut out = SIG.to_vec();
    out.push(header.len() as u8);
    out.extend_from_slice(header);
    out
}

fn class(err: Error) -> &'static str {
    err.class()
}

#[test]
fn bad_signature_is_corrupt() {
    assert_eq!(
        class(imij_oracle::decode(&[0u8; 16]).unwrap_err()),
        "Corrupt"
    );
}

#[test]
fn header_version_one_is_unsupported() {
    let err = imij_oracle::decode(&file(&[1, 0, 1, 1])).unwrap_err();
    assert_eq!(class(err), "Unsupported");
}

#[test]
fn unknown_present_bit_is_unsupported() {
    // present = 0x200 (varint 80 04): bit 9 is not defined.
    let err = imij_oracle::decode(&file(&[2, 0x80, 0x04, 1, 1])).unwrap_err();
    assert_eq!(class(err), "Unsupported");
}

#[test]
fn unknown_feature_bit_is_unsupported() {
    // present = 4 (features written), features = 0x20000 (bit 17).
    let err = imij_oracle::decode(&file(&[2, 4, 0x80, 0x80, 0x08, 1, 1])).unwrap_err();
    assert_eq!(class(err), "Unsupported");
}

#[test]
fn lossy_features_are_unsupported() {
    // Bit 14 (`channel_layouts`) and bit 16 (`lossy`, declared by a
    // lossless outer header whose later pages are lossy): known, not
    // implemented by this lossless decoder.
    for mask in [[0x80u8, 0x80, 0x01], [0x80, 0x80, 0x04]] {
        let header = [&[2u8, 4][..], &mask[..], &[1, 1]].concat();
        let err = imij_oracle::decode(&file(&header)).unwrap_err();
        assert_eq!(class(err), "Unsupported", "{mask:02x?}");
    }
}

#[test]
fn lossy_coding_is_unsupported() {
    // present = 1 (coding written), coding = 1 (lossy XYB).
    let err = imij_oracle::decode(&file(&[2, 1, 1, 1, 1])).unwrap_err();
    assert_eq!(class(err), "Unsupported");
}

#[test]
fn missing_data_chunk_is_corrupt() {
    let err = imij_oracle::decode(&file(&[2, 0, 1, 1])).unwrap_err();
    assert_eq!(class(err), "Corrupt");
}

#[test]
fn unknown_critical_chunk_is_unsupported() {
    let mut stream = file(&[2, 0, 1, 1]);
    stream.extend_from_slice(b"ABCD");
    stream.push(2); // length+1 = 2 -> one payload byte
    stream.push(0);
    let err = imij_oracle::decode(&stream).unwrap_err();
    assert_eq!(class(err), "Unsupported");
}

#[test]
fn truncated_header_length_is_corrupt() {
    let mut stream = SIG.to_vec();
    stream.push(0xFF); // a varint that never ends
    let err = imij_oracle::decode(&stream).unwrap_err();
    assert_eq!(class(err), "Corrupt");
}

#[test]
fn bad_cicp_full_range_is_corrupt() {
    // present = 64 (cicp written); full_range (the fourth byte) = 2 is an
    // unknown value of a closed field (SPEC 0.3).
    let err = imij_oracle::decode(&file(&[2, 64, 1, 13, 0, 2, 1, 1])).unwrap_err();
    assert_eq!(class(err), "Corrupt");
}

#[test]
fn non_minimal_varint_is_corrupt() {
    // A header whose width varint is written non-minimally (0x81 0x00).
    let err = imij_oracle::decode(&file(&[2, 0, 0x81, 0x00, 1])).unwrap_err();
    assert_eq!(class(err), "Corrupt");
}
