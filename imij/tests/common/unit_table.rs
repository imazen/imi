//! Test-side writers of the unit table forms (SPEC 2.7), built by editing
//! the bytes of a file the library wrote with the hint (or with none, for
//! one unit): head record 12 reserved and filled, left empty or marked check
//! tail, the 3-byte check-tail record of a writer that cannot seek, and the
//! trailing `uIDX` chunk. Independent of the library's own writer path
//! (`lab::set_unit_table_form`), which `tests/unit_table.rs` compares with
//! these in lab builds.

use super::{
    H_UNIT_RESERVE, H_UNIT_TABLE, chunks, data_payload, extensions, payload, payload_head,
    put_chunk, put_varint, varint, with_extensions, with_head_record,
};

/// How a file stores its unit table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// Head record 10 (as the library writes it).
    Hint,
    /// Check tail (a 3-byte head record 12) and a trailing index.
    Trailing,
    /// Head record 12 at this width, filled.
    Reserved(u8),
    /// Head record 12 at this width, left empty.
    Empty(u8),
    /// Head record 12 at this width, marked check tail, and a trailing index.
    Tail(u8),
}

/// The spans of the main records of `file`'s payload (each with the side
/// record before it), in record order.
pub fn spans(file: &[u8]) -> Vec<u64> {
    let h = imij::header(file).unwrap();
    let d = &file[data_payload(file)];
    let p = payload(d, &h.info, h.coding == imij::Coding::Lossy);
    let mut out = Vec::new();
    let mut start = None;
    for (id, whole, _) in &p.units {
        let s = *start.get_or_insert(whole.start);
        if id & 1 == 1 {
            out.push((whole.end - s) as u64);
            start = None;
        }
    }
    out
}

/// The trailing unit index chunk for `spans` of a payload of `data_len`
/// bytes.
pub fn index_chunk(spans: &[u64], data_len: u64) -> Vec<u8> {
    let mut payload = Vec::new();
    for &e in spans {
        put_varint(&mut payload, e);
    }
    let n = payload.len() as u64 + 20;
    let mut len_field = Vec::new();
    put_varint(&mut len_field, n + 1);
    let chunk_len = 4 + len_field.len() as u64 + n;
    payload.extend_from_slice(&data_len.to_le_bytes());
    payload.extend_from_slice(&chunk_len.to_le_bytes());
    payload.extend_from_slice(b"uIDX");
    let mut out = Vec::new();
    put_chunk(&mut out, b"uIDX", &payload);
    out
}

/// The `DATA` payload range of record `id` of the head of `file`.
pub fn head_record(file: &[u8], id: u64) -> Option<std::ops::Range<usize>> {
    let h = imij::header(file).unwrap();
    let range = data_payload(file);
    let p = payload_head(
        &file[range.clone()],
        &h.info,
        h.coding == imij::Coding::Lossy,
    );
    p.head_record(id)
        .map(|r| range.start + r.start..range.start + r.end)
}

/// `file` with its `DATA` payload's head record list holding `rec` as
/// record 12 in place of records 10 and 12, then its DATA locator settled as
/// the library's writers settle it.
pub fn with_reservation(file: &[u8], rec: Option<&[u8]>) -> Vec<u8> {
    let h = imij::header(file).unwrap();
    let lossy = h.coding == imij::Coding::Lossy;
    let range = data_payload(file);
    let d = &file[range.clone()];
    let p = payload_head(d, &h.info, lossy);
    let d = with_head_record(d, &p, H_UNIT_TABLE, None);
    let p = payload_head(&d, &h.info, lossy);
    let d = with_head_record(&d, &p, H_UNIT_RESERVE, rec);
    let (_, at, end) = chunks(file).into_iter().find(|c| &c.0 == b"DATA").unwrap();
    let mut out = file[..at].to_vec();
    put_chunk(&mut out, b"DATA", &d);
    out.extend_from_slice(&file[end..]);
    relocate(&out)
}

/// `file` with its DATA locator (header record 4) written as the library's
/// writers write it: present when the head has record 10 or record 12.
pub fn relocate(file: &[u8]) -> Vec<u8> {
    let h = imij::header(file).unwrap();
    let range = data_payload(file);
    let p = payload_head(
        &file[range.clone()],
        &h.info,
        h.coding == imij::Coding::Lossy,
    );
    let mut recs: Vec<(u64, Vec<u8>)> = extensions(file).into_iter().filter(|e| e.0 != 4).collect();
    if p.head_record(H_UNIT_TABLE).is_some() || p.head_record(H_UNIT_RESERVE).is_some() {
        let all = chunks(file);
        let data = all.iter().position(|c| &c.0 == b"DATA").unwrap();
        let mut loc = Vec::new();
        put_varint(&mut loc, (range.start - all[data].1 + p.head_end) as u64);
        put_varint(&mut loc, data as u64);
        for c in &all[..data] {
            put_varint(&mut loc, (c.2 - c.1) as u64);
        }
        let at = recs.partition_point(|e| e.0 < 4);
        recs.insert(at, (4, loc));
    }
    with_extensions(file, &recs)
}

/// `file` (the library's hint form, or no table for one unit) with its
/// unit table stored as `form`, `DATA` bounded.
pub fn with_form(file: &[u8], form: Form) -> Vec<u8> {
    let spans = spans(file);
    let reserved = |w: u8| vec![0u8; 2 + spans.len() * w as usize];
    match form {
        Form::Hint => file.to_vec(),
        Form::Trailing => append_index(&with_reservation(file, Some(&[1]))),
        Form::Empty(w) => with_reservation(file, Some(&reserved(w))),
        Form::Reserved(w) => {
            let mut f = with_reservation(file, Some(&reserved(w)));
            let r = head_record(&f, H_UNIT_RESERVE).unwrap();
            let mut filled = vec![2, w];
            for &e in &spans {
                filled.extend_from_slice(&e.to_le_bytes()[..w as usize]);
            }
            assert_eq!(filled.len(), r.len(), "spans fit {w} bytes");
            f[r].copy_from_slice(&filled);
            f
        }
        Form::Tail(w) => {
            let mut f = with_reservation(file, Some(&reserved(w)));
            let r = head_record(&f, H_UNIT_RESERVE).unwrap();
            f[r.start] = 1;
            append_index(&f)
        }
    }
}

/// `file` with the trailing index of its payload appended as its last chunk.
pub fn append_index(file: &[u8]) -> Vec<u8> {
    let d = data_payload(file);
    [file, &index_chunk(&spans(file), d.len() as u64)].concat()
}

/// `file` with its last chunk (a trailing index) replaced by `chunk`.
pub fn with_last_chunk(file: &[u8], chunk: &[u8]) -> Vec<u8> {
    let (_, at, _) = *chunks(file).last().unwrap();
    [&file[..at], chunk].concat()
}

/// `file` with its `DATA` chunk open (length field 0), the chunks after it
/// kept, and its locator settled.
pub fn reopened(file: &[u8]) -> Vec<u8> {
    let (_, at, _) = chunks(file).into_iter().find(|c| &c.0 == b"DATA").unwrap();
    let k = varint(&file[at + 4..]).1;
    let mut out = file[..at + 4].to_vec();
    out.push(0);
    out.extend_from_slice(&file[at + 4 + k..]);
    // The locator's prefix counts the length field.
    let prefix_delta = k - 1;
    let recs: Vec<(u64, Vec<u8>)> = extensions(&out)
        .into_iter()
        .map(|(id, rec)| {
            if id != 4 {
                return (id, rec);
            }
            let (prefix, n) = varint(&rec);
            let mut loc = Vec::new();
            put_varint(&mut loc, prefix - prefix_delta as u64);
            loc.extend_from_slice(&rec[n..]);
            (id, loc)
        })
        .collect();
    with_extensions(&out, &recs)
}
