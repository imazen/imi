#![doc = include_str!("help.txt")]
#![forbid(unsafe_code)]

mod args;
mod bench;
mod convert;
mod fixtures;
mod preview;
mod profile;
mod sha256;
#[path = "../../../imij/examples/support/validate_cli.rs"]
mod validate_cli;

use std::io::{Read, Write};
use std::process::ExitCode;
use std::time::Instant;

use imij::{Channels, Cicp, ImageInfo, Metadata, SampleType};

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(argv: &[String]) -> Result<(), String> {
    let cmd = argv.first().map(String::as_str).unwrap_or("");
    if matches!(cmd, "help" | "-h" | "--help") {
        print!("{}", include_str!("help.txt"));
        return Ok(());
    }
    if argv.len() == 2
        && matches!(argv[1].as_str(), "-h" | "--help")
        && matches!(
            cmd,
            "encode"
                | "convert-raw"
                | "trace"
                | "decode-rgba"
                | "decode-raw"
                | "profile-decode"
                | "emit-fixtures"
                | "bench-fixtures"
        )
    {
        print!("{}", include_str!("help.txt"));
        return Ok(());
    }
    match cmd {
        "validate" => validate_cli::run(&argv[1..]),
        "encode" => encode_cmd(&argv[1..]),
        "convert-raw" => convert_raw_cmd(&argv[1..]),
        "trace" => trace_cmd(),
        "decode-rgba" => decode_rgba_cmd(),
        "decode-raw" => decode_raw_cmd(),
        "profile-decode" => profile::run(&argv[1..]),
        "emit-fixtures" => fixtures::emit(&argv[1..]),
        "bench-fixtures" => bench::emit(&argv[1..]),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: imij-wasm <encode|convert-raw|validate|trace|decode-rgba|decode-raw|profile-decode|emit-fixtures|bench-fixtures>; use --help"
        .into()
}

fn stdin_bytes() -> Result<Vec<u8>, String> {
    let mut v = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut v)
        .map_err(|e| format!("stdin: {e}"))?;
    Ok(v)
}

fn stdout_bytes(v: &[u8]) -> Result<(), String> {
    let mut out = std::io::stdout().lock();
    out.write_all(v)
        .and_then(|()| out.flush())
        .map_err(|e| format!("stdout: {e}"))
}

/// The centered `w` x `h` region (clamped to the image), tightly packed.
fn center_crop(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    w: u32,
    h: u32,
) -> (Vec<u8>, usize, ImageInfo) {
    let (w, h) = (w.min(info.width), h.min(info.height));
    let (x0, y0) = ((info.width - w) / 2, (info.height - h) / 2);
    let mut out = info.clone();
    out.width = w;
    out.height = h;
    let px = info.row_bytes() / info.width as usize;
    let rb = out.row_bytes();
    let mut v = Vec::with_capacity(rb * h as usize);
    for y in y0..y0 + h {
        let start = y as usize * stride + x0 as usize * px;
        v.extend_from_slice(&pixels[start..start + rb]);
    }
    (v, rb, out)
}

/// Input decode + conversion pipeline shared by `encode` and `convert-raw`.
struct Converted {
    pixels: Vec<u8>,
    stride: usize,
    info: ImageInfo,
    meta: Metadata,
    src_desc: String,
    src_len: usize,
}

fn load_and_convert(a: &args::EncodeArgs, input: Vec<u8>) -> Result<Converted, String> {
    let (pixels, stride, info, meta, src_desc) = match &a.raw {
        Some(r) => {
            let info = ImageInfo::new(r.w, r.h, r.channels, r.sample);
            let len = info
                .checked_buffer_len()
                .ok_or("raw geometry does not fit usize")?;
            if input.len() != len {
                return Err(format!(
                    "raw input is {} bytes, expected {len} for {}x{} {:?} {:?}",
                    input.len(),
                    r.w,
                    r.h,
                    r.channels,
                    r.sample
                ));
            }
            (
                input,
                info.row_bytes(),
                info,
                Metadata::default(),
                format!("raw:{}x{}:{:?}:{:?}", r.w, r.h, r.channels, r.sample),
            )
        }
        None => {
            let out = zenpng::decode(&input, &Default::default(), &enough::Unstoppable)
                .map_err(|e| format!("png: {e:?}"))?;
            let d = out.pixels.descriptor();
            let channels = match d.layout() as u8 {
                L_GRAY => Channels::Gray,
                L_GRAY_ALPHA => Channels::GrayAlpha,
                L_RGB => Channels::Rgb,
                L_RGBA => Channels::Rgba,
                other => return Err(format!("png: unsupported channel layout {other}")),
            };
            let sample = match d.channel_type() as u8 {
                T_U8 => SampleType::U8,
                T_U16 => SampleType::U16,
                other => return Err(format!("png: unsupported channel type {other}")),
            };
            let mut info = ImageInfo::new(out.info.width, out.info.height, channels, sample);
            if let Some(c) = out.info.cicp {
                info.cicp = Cicp {
                    color_primaries: c.color_primaries,
                    transfer_characteristics: c.transfer_characteristics,
                    matrix_coefficients: c.matrix_coefficients,
                    full_range: c.full_range,
                };
            }
            let meta = Metadata {
                icc: out.info.icc_profile,
                exif: out.info.exif,
                xmp: out.info.xmp,
                ..Metadata::default()
            };
            (
                out.pixels.copy_to_contiguous_bytes(),
                info.row_bytes(),
                info,
                meta,
                format!(
                    "png:{}x{}:{:?}:{:?}",
                    out.info.width, out.info.height, channels, sample
                ),
            )
        }
    };
    let (pixels, stride, info) = match a.crop {
        Some((w, h)) => center_crop(&pixels, stride, &info, w, h),
        None => (pixels, stride, info),
    };
    let src_len = pixels.len();

    let (pixels, info) = convert::convert(&pixels, stride, info, a)?;
    let mut info = info;
    if let Some(bits) = &a.sbits {
        // A single value applies to every channel (the UI form offers one
        // number); a list must name each channel of the *converted* format.
        let bits = if bits.len() == 1 {
            vec![bits[0]; info.channels.count()]
        } else {
            bits.clone()
        };
        info = info
            .with_significant_bits(&bits)
            .map_err(|e| format!("sbits: {e}"))?;
    }
    info.alpha_mode = if a.premultiplied {
        imij::AlphaMode::Premultiplied
    } else {
        imij::AlphaMode::Straight
    };
    if let Some(cicp) = a.cicp {
        info.cicp = cicp;
    }
    Ok(Converted {
        pixels,
        stride: info.row_bytes(),
        info,
        meta,
        src_desc,
        src_len,
    })
}

fn encode_cmd(argv: &[String]) -> Result<(), String> {
    let a = args::EncodeArgs::parse(argv)?;
    let input = stdin_bytes()?;
    let c = load_and_convert(&a, input)?;

    let t = Instant::now();
    let out = a
        .mode
        .clone()
        .with_threads(1)
        .encode_samples(imij::ImageRef::new(&c.pixels, c.stride, &c.info), &c.meta)
        .map_err(|e| format!("encode: {e}"))?;
    let ms = t.elapsed().as_secs_f64() * 1000.0;

    stdout_bytes(&out)?;
    eprintln!(
        "{{\"input\":\"{}\",\"format\":\"{:?}x{:?}\",\"w\":{},\"h\":{},\"in_bytes\":{},\"bitmap_bytes\":{},\"out_bytes\":{},\"ratio\":{:.4},\"encode_ms\":{ms:.2},\"mode\":\"{}\"}}",
        c.src_desc,
        c.info.channels,
        c.info.sample,
        c.info.width,
        c.info.height,
        c.src_len,
        c.pixels.len(),
        out.len(),
        out.len() as f64 / c.src_len.max(1) as f64,
        match a.mode.coder() {
            imij::Coder::Quick(_) => "quick",
            imij::Coder::Lossless => "lossless",
            imij::Coder::Lossy(_) => "lossy",
            _ => "unknown",
        },
    );
    Ok(())
}

fn trace_cmd() -> Result<(), String> {
    let data = stdin_bytes()?;
    let json = imij::lab::codestream_trace(&data);
    stdout_bytes(json.as_bytes())
}

/// The converted bitmap exactly as `encode` would hand it to the encoder —
/// lets the parity harness check lossless decode == converted pixels.
fn convert_raw_cmd(argv: &[String]) -> Result<(), String> {
    // Same conversion flags; --lossless/--lossy/--quick is parsed and ignored.
    let mut argv = argv.to_vec();
    if !argv
        .iter()
        .any(|a| a == "--lossless" || a == "--lossy" || a == "--quick")
    {
        argv.push("--lossless".into());
    }
    let a = args::EncodeArgs::parse(&argv)?;
    let input = stdin_bytes()?;
    let c = load_and_convert(&a, input)?;
    eprintln!(
        "{{\"format\":\"{:?}x{:?}\",\"w\":{},\"h\":{}}}",
        c.info.channels, c.info.sample, c.info.width, c.info.height
    );
    stdout_bytes(&c.pixels)
}

fn decode_rgba_cmd() -> Result<(), String> {
    let data = stdin_bytes()?;
    let (info, pixels) = imij::decode(&data).map_err(|e| format!("decode: {e}"))?;
    let rgba = convert::to_rgba8(&info, &pixels)?;
    eprintln!(
        "{{\"format\":\"{:?}x{:?}\",\"w\":{},\"h\":{},\"note\":\"{}\"}}",
        info.channels,
        info.sample,
        info.width,
        info.height,
        convert::TONING_NOTE
    );
    stdout_bytes(&rgba)
}

/// Decoded pixels verbatim: `W H CHANNELS SAMPLE` on stderr, bytes on stdout.
fn decode_raw_cmd() -> Result<(), String> {
    let data = stdin_bytes()?;
    let (info, pixels) = imij::decode(&data).map_err(|e| format!("decode: {e}"))?;
    eprintln!(
        "{{\"format\":\"{:?}x{:?}\",\"w\":{},\"h\":{}}}",
        info.channels, info.sample, info.width, info.height
    );
    stdout_bytes(&pixels)
}

// zenpng's pixel descriptor enums live in zenpixels, which is not a direct
// dependency; the enums are `#[repr(u8)]` fieldless, so compare the u8
// discriminants instead of naming the types.
const L_GRAY: u8 = 1;
const L_GRAY_ALPHA: u8 = 2;
const L_RGB: u8 = 3;
const L_RGBA: u8 = 4;
const T_U8: u8 = 1;
const T_U16: u8 = 2;
