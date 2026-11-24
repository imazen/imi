//! Bounded import of an uncompressed, merged RGB8 PSB raster for browser demos.
//! The embedded ICC is authoritative; output is explicitly converted to sRGB.
#![forbid(unsafe_code)]
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use zenpixels_convert::cms::PluggableCms;
use zenpixels_convert::cms_moxcms::MoxCms;
use zenpixels_convert::{ColorProfileSource, ConvertOptions, NamedProfile, PixelFormat};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn u32be(b: &[u8]) -> u32 {
    u32::from_be_bytes(b.try_into().unwrap())
}
fn load(file: &mut File, n: usize) -> Result<Vec<u8>> {
    let mut b = vec![0; n];
    file.read_exact(&mut b)?;
    Ok(b)
}
fn header(file: &mut File) -> Result<(u32, u32, u64, Vec<u8>)> {
    let h = load(file, 26)?;
    if &h[..6] != b"8BPS\0\x02"
        || h[6..12] != [0; 6]
        || h[12..14] != [0, 3]
        || h[22..26] != [0, 8, 0, 3]
    {
        return Err("requires merged PSB v2 RGB8 with three channels".into());
    }
    let (height, width) = (u32be(&h[14..18]), u32be(&h[18..22]));
    if width == 0 || height == 0 || width > 300000 || height > 300000 {
        return Err("PSB dimensions".into());
    }
    let color = u32be(&load(file, 4)?);
    file.seek(SeekFrom::Current(i64::from(color)))?;
    let n = u32be(&load(file, 4)?) as usize;
    if n > 16 << 20 {
        return Err("resource section exceeds bounded importer".into());
    }
    let resources = load(file, n)?;
    let mut at = 0;
    let mut icc = None;
    while at < resources.len() {
        if resources.len() - at < 7 || &resources[at..at + 4] != b"8BIM" {
            return Err("PSB resource framing".into());
        }
        let id = u16::from_be_bytes(resources[at + 4..at + 6].try_into()?);
        at += 6;
        let name = usize::from(resources[at]);
        at += (name + 2) & !1;
        let size = u32be(resources.get(at..at + 4).ok_or("resource size")?) as usize;
        at += 4;
        let end = at.checked_add(size).ok_or("resource extent")?;
        let data = resources.get(at..end).ok_or("resource data")?;
        if id == 1039 {
            if icc.is_some() {
                return Err("duplicate ICC".into());
            }
            icc = Some(data.to_vec());
        }
        at = end.checked_add(size & 1).ok_or("resource padding extent")?;
    }
    if at != resources.len() {
        return Err("resource padding".into());
    }
    let layers = u64::from_be_bytes(load(file, 8)?.try_into().unwrap());
    let offset = file
        .stream_position()?
        .checked_add(layers)
        .ok_or("layer extent")?;
    file.seek(SeekFrom::Start(offset))?;
    if load(file, 2)? != [0, 0] {
        return Err("only uncompressed PSB composite supported".into());
    }
    let offset = file.stream_position()?;
    if offset.checked_add(u64::from(width) * u64::from(height) * 3) != Some(file.metadata()?.len())
    {
        return Err("PSB composite extent".into());
    }
    Ok((width, height, offset, icc.ok_or("embedded ICC required")?))
}
fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() != 4 {
        return Err("psb_srgb SOURCE.psb FRESH.rgb FRESH.icc".into());
    }
    let mut source = File::open(&a[1])?;
    let (width, height, offset, icc) = header(&mut source)?;
    let mut transform = MoxCms
        .build_source_transform(
            ColorProfileSource::Icc(&icc),
            ColorProfileSource::Named(NamedProfile::Srgb),
            PixelFormat::Rgb8,
            PixelFormat::Rgb8,
            &ConvertOptions::permissive(),
        )
        .ok_or("CMS declined")??;
    let mut profile = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&a[3])?;
    profile.write_all(&icc)?;
    let mut out = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&a[2])?,
    );
    let mut plane = vec![0; width as usize];
    let mut input = vec![0; width as usize * 3];
    let mut output = input.clone();
    for y in 0..height {
        for channel in 0..3u64 {
            source.seek(SeekFrom::Start(
                offset
                    + channel * u64::from(width) * u64::from(height)
                    + u64::from(y) * u64::from(width),
            ))?;
            source.read_exact(&mut plane)?;
            for (x, &v) in plane.iter().enumerate() {
                input[x * 3 + channel as usize] = v;
            }
        }
        transform.transform_row(&input, &mut output, width);
        out.write_all(&output)?;
        if y % 4096 == 0 {
            eprintln!("converted {y}/{height} rows");
        }
    }
    out.flush()?;
    println!("{width}x{height}: embedded ICC -> sRGB, RGB8 relative-colorimetric display raster");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<u8> {
        let mut b = b"8BPS\0\x02\0\0\0\0\0\0\0\x03".to_vec();
        b.extend(2u32.to_be_bytes());
        b.extend(3u32.to_be_bytes());
        b.extend([0, 8, 0, 3]);
        b.extend(0u32.to_be_bytes());
        let mut r = b"8BIM\x04\x0f\0\0".to_vec();
        r.extend(3u32.to_be_bytes());
        r.extend(b"icc\0");
        b.extend((r.len() as u32).to_be_bytes());
        b.extend(r);
        b.extend(0u64.to_be_bytes());
        b.extend([0, 0]);
        b.extend(0..18);
        b
    }
    #[test]
    fn bounded_raw_psb_and_rejections() {
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join("tmp");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("psb-parser-test-{}", std::process::id()));
        let bytes = fixture();
        std::fs::write(&path, &bytes).unwrap();
        let (w, h, offset, icc) = header(&mut File::open(&path).unwrap()).unwrap();
        assert_eq!((w, h, offset, icc), (3, 2, 60, b"icc".to_vec()));
        for (at, value) in [(5, 1), (13, 4), (23, 16), (25, 4), (59, 1)] {
            let mut bad = bytes.clone();
            bad[at] = value;
            std::fs::write(&path, bad).unwrap();
            assert!(
                header(&mut File::open(&path).unwrap()).is_err(),
                "accepted mutation at {at}"
            );
        }
        let mut oversized = bytes.clone();
        oversized[42..46].copy_from_slice(&u32::MAX.to_be_bytes());
        std::fs::write(&path, oversized).unwrap();
        assert!(header(&mut File::open(&path).unwrap()).is_err());
        for n in [0, 25, 35, 48, 59, 77] {
            std::fs::write(&path, &bytes[..n]).unwrap();
            assert!(header(&mut File::open(&path).unwrap()).is_err());
        }
        std::fs::remove_file(path).unwrap();
    }
}
