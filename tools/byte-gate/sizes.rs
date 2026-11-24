//! Bytes report: encoded size of a small grid per image, with and without
//! metadata, plus the hash of the decoded pixels, so `compare.py` reports
//! the size change of a container or grammar change and checks that no
//! decoded pixel moved. Built per revision by `run.sh` (`GATE_PROGRAM=
//! sizes.rs GATE_FEATURES=default`); images as for the gate,
//! `<name>_<width>_<height>.rgb8` (the report used 64 px and 4096 px sets,
//! `README.md`).
use imij::{Channels, ImiEncoderConfig, ImageInfo, LossyColor, Metadata, SampleType};

fn fnv(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &x in b {
        h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Metadata of the "meta" cells: an ICC profile, an Exif block, an XMP
/// packet and orientation 6, the same bytes on both revisions.
fn metadata() -> Metadata {
    // A 1 KiB stand-in profile: its content is opaque to the codec.
    let icc: Vec<u8> = (0..1024u32).map(|i| (i * 31 % 251) as u8).collect();
    let mut exif = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
    exif.resize(64, 0);
    let xmp = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></x:xmpmeta>"#.to_vec();
    Metadata {
        icc: Some(icc),
        exif: Some(exif),
        xmp: Some(xmp),
        orientation: 6,
        ..Metadata::default()
    }
}

fn main() {
    let meta = metadata();
    let none = Metadata::default();
    for path in std::env::args().skip(1) {
        let stem = std::path::Path::new(&path).file_stem().unwrap().to_string_lossy().to_string();
        let mut parts = stem.rsplitn(3, '_');
        let h: u32 = parts.next().unwrap().parse().unwrap();
        let w: u32 = parts.next().unwrap().parse().unwrap();
        let name = parts.next().unwrap().to_string();
        let rgb = std::fs::read(&path).unwrap();
        assert_eq!(rgb.len(), (w * h * 3) as usize);
        let rgba: Vec<u8> = rgb
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], p[1] / 2 + 64])
            .collect();
        let row = |name: &str, label: &str, file: &[u8]| {
            let px = imij::decode(file).unwrap().1;
            println!("{name}\t{w}x{h}\t{label}\t{}\t{:016x}\t{:016x}", file.len(), fnv(file), fnv(&px));
        };
        for (label, alpha, m) in [
            ("ll", false, &none),
            ("ll-meta", false, &meta),
            ("ll-rgba", true, &none),
        ] {
            let (px, ch) = if alpha { (&rgba, Channels::Rgba) } else { (&rgb, Channels::Rgb) };
            let info = ImageInfo::new(w, h, ch, SampleType::U8);
            let f = imij::ImiEncoderConfig::default().encode_samples(imij::ImageRef::new(px, info.row_bytes(), &info), m).unwrap();
            row(&name, label, &f);
        }
        for (label, q, color, alpha, m) in [
            ("lossy-q10", 10.0f32, LossyColor::Xyb, false, &none),
            ("lossy-q30", 30.0, LossyColor::Xyb, false, &none),
            ("lossy-q60", 60.0, LossyColor::Xyb, false, &none),
            ("lossy-q90", 90.0, LossyColor::Xyb, false, &none),
            ("lossy-q60-meta", 60.0, LossyColor::Xyb, false, &meta),
            ("lossy-q60-ycocg", 60.0, LossyColor::YCoCg, false, &none),
            ("lossy-q60-rgba", 60.0, LossyColor::Xyb, true, &none),
        ] {
            let (px, ch) = if alpha { (&rgba, Channels::Rgba) } else { (&rgb, Channels::Rgb) };
            let info = ImageInfo::new(w, h, ch, SampleType::U8);
            let cfg = imij::ImiEncoderConfig::lossy(q).tune_lossy(|t| t.color = color);
            let f = cfg.encode_samples(imij::ImageRef::new(px, info.row_bytes(), &info), m).unwrap();
            row(&name, label, &f);
        }
    }
}
