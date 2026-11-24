//! Shared CLI entry point; kept outside the library's public API.
use imij::planes::SeekReader;
use imij::validation::ValidationLimits;
use std::fs::{File, OpenOptions};

pub const HELP: &str = "validate INPUT.imi NEW-SCRATCH-FILE [OPTIONS]

Fully decode a v3 file and every recognized nested image. Verify all declared
file, pixel and reference checksums. There is no structure-only mode.
Native JPEG/components are retired from v3 and cannot be upgraded.

Input uses positional reads; stdin is not supported. Scratch must be a new file
and is never overwritten or automatically removed. Only referenced images and
animation composition require raster scratch. Retained references remain until
the call ends. Failure can leave partial scratch; success prints JSON counters.
Unknown ancillary extensions remain opaque. Missing checksums remain absent.

Options (unsigned decimal byte/pixel counts; zero is allowed):
  --max-file-bytes N       Maximum bytes per encoded image (default 1099511627776)
  --max-scratch-bytes N    Maximum appended scratch bytes (default 1099511627776)
  --max-decoded-pixels N   Maximum total decoded pixels (default 1099511627776)
  --max-working-bytes N    Decoder planned working-storage cap (not process RSS)
  -h, --help              Show this help
";

struct Options<'a> {
    input: &'a str,
    scratch: &'a str,
    limits: ValidationLimits,
}
fn parse(args: &[String]) -> Result<Option<Options<'_>>, String> {
    if args.len() == 1 && matches!(args[0].as_str(), "-h" | "--help") {
        return Ok(None);
    }
    if args.len() < 2 || args[0].starts_with('-') || args[1].starts_with('-') {
        return Err(format!("usage: {HELP}"));
    }
    let mut limits = ValidationLimits::default();
    let mut seen = Vec::new();
    let mut rest = args[2..].iter();
    while let Some(flag) = rest.next() {
        if seen.contains(&flag.as_str()) {
            return Err(format!("duplicate option: {flag}"));
        }
        seen.push(flag.as_str());
        if !matches!(
            flag.as_str(),
            "--max-file-bytes"
                | "--max-scratch-bytes"
                | "--max-decoded-pixels"
                | "--max-working-bytes"
        ) {
            return Err(format!("unknown validation option: {flag}"));
        }
        let value = rest
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        let n = value
            .parse::<u64>()
            .map_err(|_| format!("{flag} requires an unsigned decimal integer"))?;
        match flag.as_str() {
            "--max-file-bytes" => limits.max_file_bytes = n,
            "--max-scratch-bytes" => limits.max_scratch_bytes = n,
            "--max-decoded-pixels" => limits.max_decoded_pixels = n,
            "--max-working-bytes" => limits.decode = limits.decode.with_max_working_bytes(n),
            _ => unreachable!(),
        }
    }
    Ok(Some(Options {
        input: &args[0],
        scratch: &args[1],
        limits,
    }))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let Some(options) = parse(args)? else {
        print!("{HELP}");
        return Ok(());
    };
    let mut source = SeekReader::new(File::open(options.input).map_err(|e| format!("input: {e}"))?);
    let mut scratch = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(options.scratch)
        .map_err(|e| format!("new scratch file: {e}"))?;
    let r = imij::validation::validate_v3(&mut source, &mut scratch, &options.limits)
        .map_err(|e| format!("validation: {e}"))?;
    println!(
        "{{\"images\":{},\"decoded_pixels\":{},\"file_checksums\":{},\"pixel_checksums\":{},\"reference_checksums\":{},\"opaque_chunks\":{},\"scratch_bytes\":{}}}",
        r.images,
        r.decoded_pixels,
        r.file_checksums,
        r.pixel_checksums,
        r.reference_checksums,
        r.opaque_chunks,
        r.scratch_bytes
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn help_and_limits_are_explicit() {
        assert!(parse(&args(&["--help"])).unwrap().is_none());
        let values = args(&[
            "image.imi",
            "scratch",
            "--max-scratch-bytes",
            "0",
            "--max-decoded-pixels",
            "123",
        ]);
        let parsed = parse(&values).unwrap().unwrap();
        assert_eq!(parsed.limits.max_scratch_bytes, 0);
        assert_eq!(parsed.limits.max_decoded_pixels, 123);
    }
    #[test]
    fn malformed_options_and_partial_validation_are_rejected_before_io() {
        for values in [
            vec![],
            vec!["image.imi"],
            vec!["image.imi", "scratch", "--structure-only"],
            vec!["image.imi", "scratch", "--max-file-bytes"],
            vec!["image.imi", "scratch", "--max-file-bytes", "-1"],
            vec![
                "image.imi",
                "scratch",
                "--max-file-bytes",
                "18446744073709551616",
            ],
            vec![
                "image.imi",
                "scratch",
                "--max-file-bytes",
                "1",
                "--max-file-bytes",
                "2",
            ],
        ] {
            assert!(parse(&args(&values)).is_err(), "{values:?}");
        }
    }
}
