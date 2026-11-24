//! Extract latest archive entries. Sequences become independent lossless images
//! by default; --keep-sequences copies each complete archive blob unchanged.
use imij::planes::{ReadAt, SeekReader};
use imij::{FrameDecoder, FrameHeader, FrameKind, ImageInfo, ImiEncoderConfig, Limits};
use imij_set::Reader;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
Usage: extract ARCHIVE NEW_DIRECTORY [--effort 0..10] [--keep-sequences]

Extract the latest entries into a new directory. Existing images and opaque
blobs keep their bytes. Sequences become standalone lossless images.

  --keep-sequences   Keep each complete sequence byte-for-byte
  --effort N         Encoding effort when flattening sequences (default 0)
  -h, --help         Show this help
";

const MAX_ENTRY: usize = 512 * 1024 * 1024;
const PIXEL_LIMITS: Limits = {
    let mut limits = Limits::DEFAULT;
    limits.max_bytes = 256 * 1024 * 1024;
    limits
};

fn codec(error: imij::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn output(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    // The destination directory is exclusive to this invocation. Failed writes
    // retain the partial file, never a successfully named output.
    let partial = path.with_extension("partial");
    let mut file = File::create_new(&partial)?;
    write(&mut file)?;
    file.flush()?;
    drop(file);
    fs::rename(partial, path)
}
fn read_bounded(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let length = usize::try_from(file.metadata()?.len()).map_err(io::Error::other)?;
    if length > max {
        return Err(invalid("compressed image exceeds extraction limit"));
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(io::Error::other)?;
    bytes.resize(length, 0);
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn frame_path(directory: &Path, key: u64, index: usize) -> PathBuf {
    directory.join(format!("{key}-{index}.imij"))
}

/// Resolve references through the existing renderer. Only the current raster
/// is retained here; the caller reloads any earlier standalone output needed.
fn materialize(
    bytes: &[u8],
    limits: &Limits,
    config: &ImiEncoderConfig,
    mut reference: impl FnMut(usize) -> io::Result<Vec<u8>>,
    mut emit: impl FnMut(FrameHeader, &ImageInfo, &[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let mut decoder = FrameDecoder::with_limits(bytes, limits).map_err(codec)?;
    // Plane attachments need their own indexed remux. Retaining the complete
    // sequence is supported; flattening must not silently drop those planes.
    if bytes.len() >= 37 && &bytes[bytes.len() - 37..bytes.len() - 33] == b"sLOC" {
        return Err(invalid(
            "sequence plane attachments require --keep-sequences",
        ));
    }
    while let Some(header) = decoder.next_frame().map_err(codec)? {
        let info = if decoder.kind() == FrameKind::Animation {
            decoder.info().clone()
        } else {
            decoder
                .frame_info()
                .ok_or_else(|| invalid("missing page description"))?
        };
        let size = info
            .checked_buffer_len()
            .ok_or_else(|| invalid("raster size overflow"))?;
        if size as u64 > limits.max_bytes
            || u64::from(info.width) * u64::from(info.height) > limits.max_pixels
        {
            return Err(invalid("raster exceeds extraction limit"));
        }
        let mut pixels = if let Some(index) = decoder.reference_index() {
            let encoded = reference(index)?;
            let (reference_info, pixels) =
                imij::decode_with_limits(&encoded, limits).map_err(codec)?;
            if reference_info != info || pixels.len() != size {
                return Err(invalid("reference pixel description differs"));
            }
            pixels
        } else {
            let mut pixels = Vec::new();
            pixels.try_reserve_exact(size).map_err(io::Error::other)?;
            pixels.resize(size, 0);
            pixels
        };
        decoder
            .decode_into(&mut pixels, info.row_bytes(), 1)
            .map_err(codec)?;
        let metadata = decoder
            .frame_metadata()
            .ok_or_else(|| invalid("missing image metadata"))?;
        // A lossy or near-lossless source is rendered once and then encoded
        // losslessly: no second lossy generation, color conversion or alpha loss.
        let encoded = config
            .encode_samples(
                imij::ImageRef::new(&pixels, info.row_bytes(), &info),
                metadata,
            )
            .map_err(codec)?;
        emit(header, &info, &encoded)?;
    }
    Ok(())
}

fn extract<S: ReadAt>(
    reader: &mut Reader<S>,
    directory: &Path,
    keep: bool,
    config: &ImiEncoderConfig,
) -> io::Result<()> {
    fs::create_dir(directory)?;
    let mut manifest = File::create_new(directory.join("images.tsv.partial"))?;
    writeln!(
        manifest,
        "key\timage\tfile\tkind\tduration_num\tduration_den\tloop_count"
    )?;
    let mut listing = reader.listing()?;
    while let Some(entry) = listing.next(reader)? {
        let key = entry.key();
        // Keep archive labels/typed attributes without interpreting a filename
        // as a path, or overwriting authoritative per-image metadata.
        output(&directory.join(format!("{key}.kvmd")), |out| {
            out.write_all(&entry.metadata().to_bytes().map_err(codec)?)
        })?;
        let mut signature = [0; 8];
        let n = entry.len().min(8) as usize;
        reader
            .entry_source(&entry)?
            .read_exact_at(0, &mut signature[..n])
            .map_err(codec)?;
        let image = n == 8 && signature == *b"IMIJ\r\n\x87\n";
        if keep || !image {
            let name = format!("{key}.{}", if image { "imij" } else { "bin" });
            output(&directory.join(&name), |out| reader.copy_to(&entry, out))?;
            writeln!(manifest, "{key}\t-\t{name}\tentry\t-\t-\t-")?;
            continue;
        }
        let bytes = reader.read_entry_bounded(&entry, MAX_ENTRY)?;
        let decoder = FrameDecoder::with_limits(&bytes, &PIXEL_LIMITS).map_err(codec)?;
        let kind = decoder.kind();
        let loops = decoder.loop_count();
        drop(decoder);
        if kind == FrameKind::Still {
            // Already standalone: preserve every encoded byte and attachment.
            let name = format!("{key}.imij");
            output(&directory.join(&name), |out| out.write_all(&bytes))?;
            writeln!(manifest, "{key}\t0\t{name}\tStill\t0\t1\t0")?;
            continue;
        }
        materialize(
            &bytes,
            &PIXEL_LIMITS,
            config,
            |index| read_bounded(&frame_path(directory, key, index), MAX_ENTRY),
            |header, _, encoded| {
                let path = frame_path(directory, key, header.index);
                output(&path, |out| out.write_all(encoded))?;
                writeln!(
                    manifest,
                    "{key}\t{}\t{key}-{}.imij\t{kind:?}\t{}\t{}\t{loops}",
                    header.index, header.index, header.duration.num, header.duration.den
                )
            },
        )?;
    }
    manifest.flush()?;
    drop(manifest);
    fs::rename(
        directory.join("images.tsv.partial"),
        directory.join("images.tsv"),
    )
}

struct Options {
    source: PathBuf,
    directory: PathBuf,
    keep: bool,
    config: ImiEncoderConfig,
}

fn options(args: impl IntoIterator<Item = std::ffi::OsString>) -> io::Result<Options> {
    let mut args = args.into_iter();
    let source = args
        .next()
        .ok_or_else(|| invalid("archive path required"))?
        .into();
    let directory = args
        .next()
        .ok_or_else(|| invalid("new output directory required"))?
        .into();
    let mut keep = false;
    let mut effort = None;
    while let Some(flag) = args.next() {
        if flag == "--keep-sequences" && !keep {
            keep = true;
        } else if flag == "--effort" && effort.is_none() {
            let value = args
                .next()
                .and_then(|s| s.to_str().and_then(|v| v.parse::<f32>().ok()))
                .filter(|v| v.is_finite() && (0.0..=10.0).contains(v))
                .ok_or_else(|| invalid("effort must be finite and in 0..=10"))?;
            effort = Some(value);
        } else {
            return Err(invalid("unknown or repeated extraction option"));
        }
    }
    if keep && effort.is_some() {
        return Err(invalid(
            "--effort applies to standalone extraction, not --keep-sequences",
        ));
    }
    Ok(Options {
        source,
        directory,
        keep,
        config: {
            let mut cfg = imij::ImiEncoderConfig::default();
            cfg = cfg.with_effort(effort.unwrap_or(0.0));
            cfg = cfg.with_threads(1);
            cfg
        },
    })
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args == ["-h"] || args == ["--help"] {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let options = match options(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("extract: {error}\n{HELP}");
            return ExitCode::from(2);
        }
    };
    let result = (|| {
        let mut reader = Reader::open(SeekReader::new(File::open(&options.source)?))?;
        extract(
            &mut reader,
            &options.directory,
            options.keep,
            &options.config,
        )
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("extract {}: {error}", options.source.display());
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[path = "extract_tests/mod.rs"]
mod tests;
