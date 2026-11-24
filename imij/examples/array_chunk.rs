//! Lossless scalar chunk driver for the NumPy array-profile witness.
use imij::planes::{PlaneSetLimits, PlaneSetReader, ReadAt, Rect, SeekReader};
use imij::region::RegionReader;
use imij::{Channels, Error, ImageInfo, ImageRef, ImiEncoderConfig, Metadata, SampleType};
use std::error::Error as StdError;
use std::fs;
use std::io::{self, BufRead};
use std::str::FromStr;

struct Counted {
    data: SeekReader<fs::File>,
    reads: usize,
    bytes: usize,
}

impl ReadAt for &mut Counted {
    fn len(&mut self) -> Result<u64, Error> {
        self.data.len()
    }

    fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads += 1;
        self.bytes += out.len();
        self.data.read_exact_at(at, out)
    }
}

fn number<T: FromStr>(value: &str) -> Result<T, Box<dyn StdError>>
where
    T::Err: StdError + 'static,
{
    Ok(value.parse()?)
}

fn count(path: &str) -> io::Result<Counted> {
    Ok(Counted {
        data: SeekReader::new(fs::File::open(path)?),
        reads: 0,
        bytes: 0,
    })
}

fn dispatch(command: &[&str]) -> Result<(), Box<dyn StdError>> {
    match command {
        ["encode", dtype, width, height, input, output] => {
            let dtype = match *dtype {
                "<u8" => SampleType::U64,
                "<i8" => SampleType::I64,
                "<f4" => SampleType::F32,
                _ => return Err("unknown dtype".into()),
            };
            let info = ImageInfo::new(number(width)?, number(height)?, Channels::Scalar, dtype);
            let pixels = fs::read(input)?;
            let file = ImiEncoderConfig::lossless()
                .encode_samples(ImageRef::packed(&pixels, &info), &Metadata::default())?;
            assert_eq!(imij::header(&file)?.version, 4);
            fs::write(output, file)?;
            println!("ok");
        }
        ["read", input, output, rectangles @ ..] => {
            let (rectangles, remainder) = rectangles.as_chunks::<4>();
            if !remainder.is_empty() {
                return Err("incomplete rectangle".into());
            }
            let mut source = count(input)?;
            let mut reader = RegionReader::open(&mut source)?;
            let bytes_per_sample = reader.info().bytes_per_pixel();
            let mut pixels = Vec::new();
            for [x, y, width, height] in rectangles {
                let r = Rect::new(number(x)?, number(y)?, number(width)?, number(height)?);
                let stride = r.width() as usize * bytes_per_sample;
                let start = pixels.len();
                pixels.resize(start + stride * r.height() as usize, 0);
                reader.decode_region(r, &mut pixels[start..], stride)?;
            }
            fs::write(output, pixels)?;
            println!("{} {}", source.reads, source.bytes);
        }
        ["view", input, id, width, height, output] => {
            let length = fs::metadata(input)?.len();
            let mut source = count(input)?;
            let mut reader =
                PlaneSetReader::open_with_length(&mut source, length, PlaneSetLimits::default())?;
            let rect = Rect::new(0, 0, number(width)?, number(height)?);
            let id = number(id)?;
            let ranges = reader.view_region_ranges(id, rect)?;
            let stride = rect.width() as usize * 3;
            let mut pixels = vec![0; stride * rect.height() as usize];
            reader.decode_view_region(id, rect, &mut pixels, stride)?;
            fs::write(output, pixels)?;
            print!("{} {}", source.reads, source.bytes);
            for range in ranges {
                print!(" {} {}", range.start, range.end);
            }
            println!();
        }
        _ => return Err("invalid command".into()),
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn StdError>> {
    for line in io::stdin().lock().lines() {
        dispatch(&line?.split_whitespace().collect::<Vec<_>>())?;
    }
    Ok(())
}
