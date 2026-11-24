//! Local preparation through the production archive migrator.
#![forbid(unsafe_code)]
use std::{
    fs::File,
    io::{BufWriter, Write},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() == 3 && a[1] == "xxh3" {
        let bytes = std::fs::read(&a[2])?;
        println!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
        return Ok(());
    }
    if a.len() == 7 && a[1] == "pack-rgb8" {
        let info = imij::ImageInfo::new(
            a[3].parse()?,
            a[4].parse()?,
            imij::Channels::Rgb,
            imij::SampleType::U8,
        );
        let pixels = std::fs::read(&a[2])?;
        if pixels.len() != info.checked_buffer_len().ok_or("RGB extent overflow")? {
            return Err("RGB source extent differs".into());
        }
        let config = imij::ImiEncoderConfig::lossless()
            .with_effort(2.5)
            .with_threads(1);
        let legacy = config.encode_legacy_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &Default::default(),
        )?;
        let current = imij::upgrade_image(&legacy, &Default::default())?;
        let mut summary = imij_set::Summary::default();
        summary.width = info.width;
        summary.height = info.height;
        for (path, bytes) in [(&a[5], &legacy), (&a[6], &current)] {
            let mut writer = imij_set::Writer::new(BufWriter::new(File::create_new(path)?));
            writer.insert(1, bytes.len() as u64, 0, summary, &mut bytes.as_slice())?;
            let (writer, _) = writer.finish()?;
            writer.get_ref().sync_all()?;
        }
        return Ok(());
    }
    if a.len() == 5 && a[1] == "pixels" {
        let mut reader = imij_set::Reader::open(imij::planes::SeekReader::new(File::open(&a[2])?))?;
        let entry = reader.get(a[3].parse()?)?.ok_or("missing pixel source")?;
        let encoded = reader.read_entry_bounded(&entry, 64 * 1024 * 1024)?;
        let (info, pixels) = imij::decode(&encoded)?;
        File::create_new(&a[4])?.write_all(&pixels)?;
        println!(
            "{} {} {} {}",
            info.width,
            info.height,
            info.row_bytes(),
            pixels.len()
        );
        return Ok(());
    }
    if a.len() != 4 || !matches!(a[1].as_str(), "upgrade" | "reindex") {
        return Err("usage: imij-archive-network upgrade|reindex SOURCE FRESH_DESTINATION".into());
    }
    let mut reader = imij_set::Reader::open(imij::planes::SeekReader::new(File::open(&a[2])?))?;
    let destination = BufWriter::new(File::create_new(&a[3])?);
    if a[1] == "reindex" {
        let entry = reader.get(1)?.ok_or("missing I01 group")?;
        let bytes = reader.read_entry_bounded(&entry, 64 * 1024 * 1024)?;
        let mut metadata = entry.metadata().clone();
        let mut writer = imij_set::Writer::new(destination);
        writer
            .insert_sequence_index(0, &bytes, &imij::Limits::DEFAULT)?
            .write_metadata(&mut metadata)?;
        writer.insert_with_metadata(
            1,
            bytes.len() as u64,
            entry.head_size(),
            entry.summary(),
            &metadata,
            &mut bytes.as_slice(),
        )?;
        let (writer, _) = writer.finish()?;
        writer.get_ref().sync_all()?;
        return Ok(());
    }
    let (writer, _) = imij_set::upgrade(&mut reader, destination, &Default::default())?;
    writer.get_ref().sync_all()?;
    Ok(())
}
