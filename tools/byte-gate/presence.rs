//! Presence-bit write frequencies over the byte-gate grid (SPEC 1.0 and
//! 2.8; the integration decision 1 of format version 2): for every encode, the header's
//! `present` varint, its feature mask and, for lossy files, the lossy head's
//! `present` varint, one line each (`presence.py` tallies them). Built per
//! revision by `run.sh` (`GATE_PROGRAM=presence.rs GATE_FEATURES=default`);
//! reads version 2 files only.
include!("grid.rs");

fn varint(b: &[u8], at: &mut usize) -> u64 {
    let mut v = 0u64;
    for i in 0..10 {
        let x = b[*at];
        *at += 1;
        v |= u64::from(x & 127) << (7 * i);
        if x < 128 {
            break;
        }
    }
    v
}

/// (header present, feature mask, lossy present or "-") of a file.
fn presence(f: &[u8]) -> (u64, u64, String) {
    let mut at = 8;
    let hl = varint(f, &mut at) as usize;
    let h = &f[at..at + hl];
    let end = at + hl;
    assert_eq!(h[0], 2, "header version");
    let mut i = 1;
    let present = varint(h, &mut i);
    let features = if present & 4 != 0 { varint(h, &mut i) } else { 0 };
    let lossy = present & 1 != 0 && h[i] != 0;
    let mut lp = "-".to_string();
    if lossy {
        // Walk the chunks to DATA; its payload starts with the lossy head.
        let mut c = end;
        loop {
            let tag = &f[c..c + 4];
            c += 4;
            let n = varint(f, &mut c);
            if tag == b"DATA" {
                let mut d = c + 1; // stripe_rows
                varint(f, &mut d); // group_cols
                lp = varint(f, &mut d).to_string();
                break;
            }
            c += n as usize - 1;
        }
    }
    (present, features, lp)
}

fn main() {
    for path in std::env::args().skip(1) {
        grid(&path, |key, out| {
            let (p, f, l) = presence(&out);
            println!("{key}\t{p}\t{f}\t{l}");
        });
    }
}
