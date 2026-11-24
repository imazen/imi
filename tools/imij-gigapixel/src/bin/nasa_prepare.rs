//! Import adjacent NASA Blue Marble RGB tiles into one indexed IMI image.
//! Source PNGs and exact raw rasters remain separate, immutable inputs.
use enough::Unstoppable;
use imij::v4::{NumericFileEncoder, ObjectIntegrity, SpatialConfig, TileLimits, TileOrder};
use imij::{Channels, ImageInfo, Metadata, SampleType};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};

fn fresh(path: &str) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .open(path)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("png") if a.len() == 4 => {
            let encoded = std::fs::read(&a[2])?;
            let decoded = zenpng::decode(
                &encoded,
                &zenpng::PngDecodeConfig::strict()
                    .with_max_pixels(500_000_000)
                    .with_max_memory(8 << 30),
                &Unstoppable,
            )?;
            assert_eq!(decoded.info.bit_depth, 8);
            assert_eq!(decoded.info.color_type, 2);
            let raw = decoded.pixels.copy_to_contiguous_bytes();
            fresh(&a[3])?.write_all(&raw)?;
            println!("RGB8 {} {} {}", decoded.info.width, decoded.info.height, raw.len());
        }
        Some("grid") if a.len() == 10 => {
            let side: u32 = a[2].parse()?;
            let columns: u32 = a[3].parse()?;
            let rows: u32 = a[4].parse()?;
            if side == 0 || columns == 0 || rows == 0 { return Err("zero geometry".into()); }
            let width = side.checked_mul(2).ok_or("width overflow")?;
            let mut tiles = Vec::new();
            for path in &a[5..9] {
                let file = File::open(path)?;
                if file.metadata()?.len() != u64::from(side) * u64::from(side) * 3 {
                    return Err("source tile RGB extent".into());
                }
                tiles.push(file);
            }
            let info = ImageInfo::new(width, width, Channels::Rgb, SampleType::U8);
            let config = imij::ImiEncoderConfig::default().with_row_groups(Some(imij::RowGroups::new(columns, rows, 16 << 20))).with_quick(imij::QuickBitSmarts::Off);
            let index = fresh(&format!("{}.index", a[9]))?;
            let count = u64::from(width.div_ceil(columns)) * u64::from(width.div_ceil(rows));
            let mut encoder = NumericFileEncoder::new(info, &Metadata::default(), &config,
                SpatialConfig { tile_columns: 2, tile_rows: 9, order: TileOrder::Morton,
                    integrity: ObjectIntegrity::ObjectsAndTiles,
                    limits: TileLimits { max_object_bytes: 1 << 20, max_tile_bytes: 16 << 20 } },
                BufWriter::new(fresh(&a[9])?), index,
                count.checked_mul(128).and_then(|n| n.checked_add(4096)).ok_or("index extent")?, u64::MAX)?;
            let mut completed = 0;
            for _ in 0..encoder.layout().tile_count() {
            encoder.begin_tile()?;
            while let Some(g) = encoder.next_group() {
                let stride = g.width as usize * 3;
                let mut raw = vec![0; stride * g.height as usize];
                for row in 0..g.height {
                    let yy = g.y + row;
                    let mut x = g.x;
                    while x < g.x + g.width {
                        let tile = ((yy / side) * 2 + x / side) as usize;
                        let take = (side - x % side).min(g.x + g.width - x);
                        let at = (u64::from(yy % side) * u64::from(side) + u64::from(x % side)) * 3;
                        tiles[tile].seek(SeekFrom::Start(at))?;
                        let start = row as usize * stride + (x - g.x) as usize * 3;
                        tiles[tile].read_exact(&mut raw[start..start + take as usize * 3])?;
                        x += take;
                    }
                }
                encoder.push_group(&raw, stride)?;
                completed += 1;
                if completed % 1024 == 0 { eprintln!("groups {completed}/{count}"); }
            }
            encoder.end_tile()?;
            }
            let (mut output, _) = encoder.finish()?;
            output.flush()?;
            println!("indexed RGB8 {width} {width} groups={count}");
        }
        _ => return Err("usage: nasa_prepare png INPUT.png FRESH.rgb | grid SIDE COLUMNS ROWS TL.rgb TR.rgb BL.rgb BR.rgb FRESH.imi".into()),
    }
    Ok(())
}
