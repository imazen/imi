//! Real RGB fixtures with selectable outer tile geometry for I/O design studies.
//! Each tile uses the unchanged production codec at effort 10. No new wire syntax.
use imij::{Channels, ImageInfo, Metadata, SampleType};
use imij_set::{IndexPlacement, Reader, Summary, Writer};
use std::{fs::File, io::Write, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn main() -> Result<()> {
    let a = std::env::args().skip(1).collect::<Vec<_>>();
    if a.len() != 8 && a.len() != 9 {
        return Err("access_fixture RAW WIDTH HEIGHT TILE_W TILE_H lossless|QUALITY OUTPUT_DIR interleaved|commit [ordinals]".into());
    }
    let unit_ordinals = match a.get(8).map(String::as_str) {
        None => false,
        Some("ordinals") => true,
        Some(_) => return Err("unknown option (expected ordinals)".into()),
    };
    let w: u32 = a[1].parse()?;
    let h: u32 = a[2].parse()?;
    let tw: u32 = a[3].parse()?;
    let th: u32 = a[4].parse()?;
    let bytes = u64::from(w) * u64::from(h) * 3;
    if w == 0 || h == 0 || tw == 0 || th == 0 || bytes > 64 * 1024 * 1024 {
        return Err("positive geometry and explicit 64 MiB source limit required".into());
    }
    if std::fs::metadata(&a[0])?.len() != bytes {
        return Err("source file extent".into());
    }
    let raw = std::fs::read(&a[0])?;
    if raw.len() as u64 != bytes {
        return Err("source raster extent".into());
    }
    let quality = if a[5] == "lossless" {
        None
    } else {
        let q: f32 = a[5].parse()?;
        if !q.is_finite() || !(0.0..=100.0).contains(&q) {
            return Err("quality range".into());
        }
        Some(q)
    };
    let placement = match a[7].as_str() {
        "interleaved" => IndexPlacement::Interleaved,
        "commit" => IndexPlacement::AtCommit,
        _ => return Err("index placement".into()),
    };
    let out = Path::new(&a[6]);
    std::fs::create_dir(out)?;
    let mut writer =
        Writer::new(File::create_new(out.join("image.imiset"))?).with_index_placement(placement)?;
    let mut reconstructed = vec![0; raw.len()];
    let mut key = 0;
    let mut payload_bytes = 0;
    for y in (0..h).step_by(th as usize) {
        for x in (0..w).step_by(tw as usize) {
            let cw = (w - x).min(tw);
            let ch = (h - y).min(th);
            let info = ImageInfo::new(cw, ch, Channels::Rgb, SampleType::U8);
            let stride = cw as usize * 3;
            let mut pixels = vec![0; stride * ch as usize];
            for row in 0..ch as usize {
                let from = ((y as usize + row) * w as usize + x as usize) * 3;
                pixels[row * stride..(row + 1) * stride].copy_from_slice(&raw[from..from + stride]);
            }
            let encoded = if let Some(q) = quality {
                let cfg = imij::ImiEncoderConfig::lossy(90.0)
                    .with_quality(imij::LossyQuality::JpegQuality(q))
                    .with_effort(10.)
                    .with_decode_speed(imij::DecodeSpeed::Fast)
                    .with_unit_ordinals(unit_ordinals)
                    .with_threads(1);
                cfg.encode_samples(
                    imij::ImageRef::new(&pixels, stride, &info),
                    &Metadata::default(),
                )?
            } else {
                let mut cfg = imij::ImiEncoderConfig::default().with_unit_ordinals(unit_ordinals);
                cfg = cfg.with_effort(10.);
                cfg = cfg.with_threads(1);
                cfg.encode_samples(
                    imij::ImageRef::new(&pixels, stride, &info),
                    &Metadata::default(),
                )?
            };
            let (decoded_info, decoded) = imij::decode(&encoded)?;
            if decoded_info != info || (quality.is_none() && decoded != pixels) {
                return Err("fixture geometry/lossless pixels".into());
            }
            if cw == w && ch == h {
                File::create_new(out.join("image.imi"))?.write_all(&encoded)?;
            }
            for row in 0..ch as usize {
                let to = ((y as usize + row) * w as usize + x as usize) * 3;
                reconstructed[to..to + stride]
                    .copy_from_slice(&decoded[row * stride..(row + 1) * stride]);
            }
            let mut summary = Summary::default();
            summary.width = cw;
            summary.height = ch;
            payload_bytes += encoded.len() as u64;
            writer.insert(
                key,
                encoded.len() as u64,
                0,
                summary,
                &mut encoded.as_slice(),
            )?;
            key += 1;
        }
    }
    writer.finish()?.0.flush()?;
    File::create_new(out.join("decoded.rgb"))?.write_all(&reconstructed)?;
    let mut reader = Reader::open(imij::planes::SeekReader::new(File::open(
        out.join("image.imiset"),
    )?))?;
    let mut entries = File::create_new(out.join("entries.tsv"))?;
    writeln!(entries, "key\tx\ty\twidth\theight\toffset\tlength\txxh3")?;
    let mut listing = reader.listing()?;
    let mut count = 0;
    while let Some(entry) = listing.next(&mut reader)? {
        if entry.key() != count {
            return Err("fixture archive order".into());
        }
        reader.copy_to(&entry, &mut std::io::sink())?;
        writeln!(
            entries,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:016x}",
            count,
            count % u64::from(w.div_ceil(tw)) * u64::from(tw),
            count / u64::from(w.div_ceil(tw)) * u64::from(th),
            entry.summary().width,
            entry.summary().height,
            entry.offset(),
            entry.len(),
            entry.checksum()
        )?;
        count += 1;
    }
    if count != key {
        return Err("fixture archive count".into());
    }
    println!(
        "entries={key} payload_bytes={payload_bytes} archive_bytes={} decoded_xxh3={:016x}",
        std::fs::metadata(out.join("image.imiset"))?.len(),
        xxhash_rust::xxh3::xxh3_64(&reconstructed)
    );
    Ok(())
}
