//! Generate a procedural grayscale fixture group by group, without a full raster.
//! This is a validation/memory test input, not a compression benchmark corpus.
use imij::stream_encode::GroupEncoder;
use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, RowGroups, SampleType};
use std::io::{BufWriter, Write};

#[path = "../tests/common/wire.rs"]
#[allow(dead_code)]
mod wire;

// Test-only writer: inject an independently calculated pSUM after the header.
// GroupEncoder writes no locator/checksum and emits its whole header first.
struct PixelSumWriter<W> {
    inner: W,
    digest: Option<[u8; 16]>,
}
impl<W: Write> Write for PixelSumWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Some(digest) = self.digest.take() {
            let end = wire::header_end(bytes);
            self.inner.write_all(&bytes[..end])?;
            self.inner.write_all(b"pSUM\x16\x01\x01\x10\x00\x01")?;
            self.inner.write_all(&digest)?;
            self.inner.write_all(&bytes[end..])?;
        } else {
            self.inner.write_all(bytes)?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

// SPEC 1.4.1 oracle uses the independent xxhash-rust dev dependency.
fn expected_digest(width: u32, height: u32) -> [u8; 16] {
    let mut hash = xxhash_rust::xxh3::Xxh3::new();
    hash.update(b"IMIJ/pSUM/v1\0\0\0\0");
    hash.update(&width.to_le_bytes());
    hash.update(&height.to_le_bytes());
    hash.update(&[Channels::Gray as u8, SampleType::U8 as u8]);
    hash.update(&0u64.to_le_bytes());
    let mut row = vec![0; width as usize];
    for y in 0..height {
        for (x, byte) in row.iter_mut().enumerate() {
            *byte = (y / 17 + (x as u32 / 1024 * 1024) / 23) as u8;
        }
        hash.update(&row);
    }
    hash.digest128().to_le_bytes()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: validation_fixture NEW-FILE WIDTH HEIGHT")?;
    let width: u32 = args.next().ok_or("missing width")?.parse()?;
    let height: u32 = args.next().ok_or("missing height")?.parse()?;
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    let out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let info = ImageInfo::new(width, height, Channels::Gray, SampleType::U8);
    let config = ImiEncoderConfig::quick()
        .with_checksum(false)
        .with_row_groups(RowGroups::new(1024, 128, 64 << 20));
    let sink = PixelSumWriter {
        inner: BufWriter::new(out),
        digest: Some(expected_digest(width, height)),
    };
    let mut encoder = GroupEncoder::new(sink, info, &Metadata::default(), &config)?;
    while let Some(group) = encoder.next_group() {
        let mut pixels = vec![0; group.width as usize * group.height as usize];
        for (y, row) in pixels.chunks_exact_mut(group.width as usize).enumerate() {
            row.fill(((group.y + y as u32) / 17 + group.x / 23) as u8);
        }
        encoder.write_group(&pixels, group.width as usize)?;
    }
    encoder.finish()?.flush()?;
    println!(
        "generated: width={width} height={height} pixels={}",
        u64::from(width) * u64::from(height)
    );
    Ok(())
}
