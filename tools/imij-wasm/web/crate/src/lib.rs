//! WebAssembly bindings of the imij decoder for the browser.
//!
//! One module instance holds one decoder host; streams are inspected with
//! [`info`] and decoded with [`decode`] (raw samples, byte-identical to the
//! native decoder) or [`decode_rgba`] (8-bit RGBA preview for canvases);
//! [`decode_region`] / [`decode_region_rgba`] decode one rectangle.
//! Meant to live in a Web Worker: a decode blocks its thread for its duration.
//!
//! Two builds of this one source: the default single-threaded SIMD128 build,
//! which runs anywhere WebAssembly does, and the `threads` build (rayon on Web
//! Workers over a shared memory through wasm-bindgen-rayon), which needs a
//! cross-origin isolated page. Both produce the same pixels; the `threads`
//! build is what makes `imij`'s native stripe/group schedule run on the web.
//!
//! `threads` is a per-call worker budget: on the `threads` build it caps the
//! rayon workers one decode may use (0/absent means the whole initialized
//! pool); on `simd` every call is serial regardless.

#![forbid(unsafe_code)]

use wasm_bindgen::prelude::*;

#[cfg(feature = "full")]
mod archive;
mod buffers;
mod native_pyramid;
#[cfg(feature = "full")]
#[path = "../../../src/preview.rs"]
mod preview;
mod ranges;
#[cfg(feature = "streaming-evaluation")]
mod streaming;
#[cfg(feature = "full")]
use preview::to_rgba8;

#[cfg(feature = "threads")]
pub use wasm_bindgen_rayon::init_thread_pool;

fn js_err(e: impl core::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

fn set(obj: &js_sys::Object, key: &str, value: impl Into<JsValue>) {
    // Setting a data property on a fresh plain object cannot fail.
    let _ = js_sys::Reflect::set(obj, &JsValue::from_str(key), &value.into());
}

/// `"threads"` or `"simd"`: which build this is.
#[wasm_bindgen(js_name = buildMode)]
pub fn build_mode() -> String {
    if cfg!(feature = "threads") {
        "threads".into()
    } else {
        "simd".into()
    }
}

/// SIMD tier the decoder resolved to (`wasm128` on wasm with `simd128`).
#[wasm_bindgen(js_name = simdTier)]
pub fn simd_tier() -> String {
    imij::simd_tier().into()
}

/// Size of the initialized rayon pool (`threads` build only): the worker count
/// one decode may use. 1 before `initThreadPool`, or in the `simd` build.
#[cfg(feature = "threads")]
#[wasm_bindgen(js_name = rayonThreads)]
pub fn rayon_threads() -> u32 {
    rayon::current_num_threads() as u32
}

fn set_info(out: &js_sys::Object, info: &imij::ImageInfo) {
    set(out, "width", info.width);
    set(out, "height", info.height);
    set(out, "channels", format!("{:?}", info.channels));
    set(out, "sample", format!("{:?}", info.sample));
    set(out, "bytesPerPixel", info.bytes_per_pixel() as u32);
}

/// Header fields of a stream: `{ width, height, channels, sample, bytesPerPixel }`.
/// No pixels are decoded.
#[cfg(feature = "full")]
#[wasm_bindgen]
pub fn info(data: &[u8]) -> Result<js_sys::Object, JsError> {
    let (info, _meta) = imij::probe(data).map_err(js_err)?;
    let out = js_sys::Object::new();
    set_info(&out, &info);
    Ok(out)
}

/// Decode into a fresh tightly packed buffer (native-endian samples), with up
/// to `threads` decoder workers on the `threads` build.
#[cfg(feature = "full")]
fn decode_owned(data: &[u8], threads: u32) -> Result<(imij::ImageInfo, Vec<u8>), JsError> {
    let (info, _meta) = imij::probe(data).map_err(js_err)?;
    let (_, total) = buffers::layout(&info, 0).map_err(js_err)?;
    let mut out = buffers::filled(total, 0).map_err(js_err)?;
    imij::decode_into_threads_with_limits(
        data,
        &mut out,
        info.row_bytes(),
        threads.max(1) as usize,
        &imij::Limits::DEFAULT,
    )
    .map_err(js_err)?;
    Ok((info, out))
}

/// Decode a codestream: `{ width, height, channels, sample, bytesPerPixel,
/// pixels: Uint8Array, threads, ms }`. `pixels` holds the decoder's own
/// sample bytes (native-endian, tightly packed) — byte-identical to the
/// native decoder and to the frozen `.raw` parity fixtures.
#[cfg(feature = "full")]
#[wasm_bindgen]
pub fn decode(data: &[u8], threads: u32) -> Result<js_sys::Object, JsError> {
    let t0 = now_ms();
    let (info, pixels) = decode_owned(data, threads)?;
    let out = js_sys::Object::new();
    set_info(&out, &info);
    set(&out, "pixels", js_sys::Uint8Array::from(&pixels[..]));
    set(&out, "threads", threads.max(1));
    set(&out, "ms", now_ms() - t0);
    Ok(out)
}

/// Hash a transport accelerator with the same XXH3-128 byte order as IMI.
#[wasm_bindgen]
pub fn xxh3_128(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(32);
    for byte in imij::HashAlgorithm::Xxh3_128.digest(data).as_bytes() {
        write!(hex, "{byte:02x}").expect("writing to String");
    }
    hex
}

/// Decode one Explorer V4 tile and require a verified internal pixel checksum.
#[cfg(feature = "full")]
#[wasm_bindgen(js_name = decodeSealedTile)]
pub fn decode_sealed_tile(data: &[u8]) -> Result<js_sys::Object, JsError> {
    let header = imij::header(data).map_err(js_err)?;
    let info = header.info;
    if header.version != 4
        || info.width > 512
        || info.height > 256
        || info.channels != imij::Channels::Rgb
        || info.sample != imij::SampleType::U8
    {
        return Err(js_err(imij::Error::InvalidInput("Explorer V4 RGB8 tile")));
    }
    let mut decoder =
        imij::StreamDecoder::with_limits(data, &imij::Limits::DEFAULT).map_err(js_err)?;
    let (stride, total) = buffers::layout(&info, 0).map_err(js_err)?;
    let mut pixels = buffers::filled(total, 0).map_err(js_err)?;
    for row in pixels.chunks_exact_mut(stride) {
        if !decoder.next_row(row).map_err(js_err)? {
            return Err(js_err(imij::Error::Corrupt("incomplete Explorer tile")));
        }
    }
    if decoder.pixel_checksum_status() != imij::PixelChecksumStatus::Verified {
        return Err(js_err(imij::Error::Corrupt(
            "unverified Explorer tile pixels",
        )));
    }
    let out = js_sys::Object::new();
    set_info(&out, &info);
    set(&out, "pixels", js_sys::Uint8Array::from(&pixels[..]));
    Ok(out)
}

/// [`decode`] plus an 8-bit RGBA preview (`{ ..., rgba: Uint8ClampedArray }`)
/// ready for `new ImageData(rgba, width, height)`. The preview conversion is
/// the same one `imij-wasm decode-rgba` uses (see `tools/imij-wasm`).
#[cfg(feature = "full")]
#[wasm_bindgen(js_name = decodeRgba)]
pub fn decode_rgba(data: &[u8], threads: u32) -> Result<js_sys::Object, JsError> {
    let t0 = now_ms();
    let (info, pixels) = decode_owned(data, threads)?;
    let out = js_sys::Object::new();
    set_info(&out, &info);
    let rgba = to_rgba8(&info, &pixels).map_err(js_err)?;
    let array = js_sys::Uint8ClampedArray::new_with_length(rgba.len() as u32);
    array.copy_from(&rgba);
    set(&out, "rgba", array);
    set(&out, "threads", threads.max(1));
    set(&out, "ms", now_ms() - t0);
    Ok(out)
}

/// Decode `width` x `height` pixels at (`x`, `y`) of the primary image into a
/// fresh tightly packed buffer, decoding only the units the rectangle
/// intersects (the native region reader, `Limits::DEFAULT` per region).
#[cfg(feature = "full")]
fn decode_region_owned(
    data: &[u8],
    rect: [u32; 4],
    threads: u32,
) -> Result<(imij::ImageInfo, Vec<u8>, String), JsError> {
    let [x, y, width, height] = rect;
    let mut reader = imij::region::RegionReader::open_with_limits(
        imij::planes::SliceReader::new(data),
        &imij::Limits::DEFAULT,
    )
    .map_err(js_err)?;
    let mut info = reader.info().clone();
    let stride = (width as usize)
        .checked_mul(info.bytes_per_pixel())
        .ok_or_else(|| js_err(imij::Error::TooLarge))?;
    let total = stride
        .checked_mul(height as usize)
        .ok_or_else(|| js_err(imij::Error::TooLarge))?;
    let mut out = buffers::filled(total, 0).map_err(js_err)?;
    reader
        .decode_region_threads(
            imij::planes::Rect::new(x, y, width, height),
            &mut out,
            stride,
            threads.max(1) as usize,
        )
        .map_err(js_err)?;
    // A crop leaves the whole-image pixel checksum Pending; only a region
    // covering the whole primary image can verify it.
    let checksum = format!("{:?}", reader.pixel_checksum_status());
    info.width = width;
    info.height = height;
    Ok((info, out, checksum))
}

/// Decode one rectangle: `{ x, y, width, height, channels, sample,
/// bytesPerPixel, pixels: Uint8Array, pixelChecksum, threads, ms }`. `pixels`
/// equals the same rows of [`decode`]'s output; `pixelChecksum` is the
/// whole-image checksum status (`Pending` for a crop).
#[cfg(feature = "full")]
#[wasm_bindgen(js_name = decodeRegion)]
pub fn decode_region(
    data: &[u8],
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    threads: u32,
) -> Result<js_sys::Object, JsError> {
    let t0 = now_ms();
    let (info, pixels, checksum) = decode_region_owned(data, [x, y, width, height], threads)?;
    let out = js_sys::Object::new();
    set_info(&out, &info);
    set(&out, "x", x);
    set(&out, "y", y);
    set(&out, "pixels", js_sys::Uint8Array::from(&pixels[..]));
    set(&out, "pixelChecksum", checksum);
    set(&out, "threads", threads.max(1));
    set(&out, "ms", now_ms() - t0);
    Ok(out)
}

/// [`decode_region`] as an 8-bit RGBA preview (`{ ..., rgba }`), the same
/// conversion as [`decode_rgba`].
#[cfg(feature = "full")]
#[wasm_bindgen(js_name = decodeRegionRgba)]
pub fn decode_region_rgba(
    data: &[u8],
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    threads: u32,
) -> Result<js_sys::Object, JsError> {
    let t0 = now_ms();
    let (info, pixels, checksum) = decode_region_owned(data, [x, y, width, height], threads)?;
    let out = js_sys::Object::new();
    set_info(&out, &info);
    set(&out, "x", x);
    set(&out, "y", y);
    let rgba = to_rgba8(&info, &pixels).map_err(js_err)?;
    let array = js_sys::Uint8ClampedArray::new_with_length(rgba.len() as u32);
    array.copy_from(&rgba);
    set(&out, "rgba", array);
    set(&out, "pixelChecksum", checksum);
    set(&out, "threads", threads.max(1));
    set(&out, "ms", now_ms() - t0);
    Ok(out)
}

/// Frames/pages decode: `{ width, height, channels, sample, pixels, headers,
/// kind, ms }`. `pixels` concatenates each frame's rows (animation frames use
/// the canvas geometry; pages their own), matching
/// `imij/examples/parity/extra.rs::frame_pixels`. `padding` inserts unused
/// bytes at each row end to prove strided output works; it is not part of
/// `pixels`.
#[cfg(feature = "full")]
#[wasm_bindgen(js_name = decodeFrames)]
pub fn decode_frames(data: &[u8], threads: u32, padding: u32) -> Result<js_sys::Object, JsError> {
    let t0 = now_ms();
    let (info, pixels, headers, kind) =
        decode_frames_owned(data, threads, padding).map_err(js_err)?;
    let out = js_sys::Object::new();
    set_info(&out, &info);
    set(&out, "pixels", js_sys::Uint8Array::from(&pixels[..]));
    set(&out, "headers", headers);
    set(&out, "kind", format!("{kind:?}"));
    set(&out, "threads", threads.max(1));
    set(&out, "ms", now_ms() - t0);
    Ok(out)
}

#[cfg(feature = "full")]
type FramePixels = (imij::ImageInfo, Vec<u8>, String, imij::FrameKind);

#[cfg(feature = "full")]
fn decode_frames_owned(
    data: &[u8],
    threads: u32,
    padding: u32,
) -> Result<FramePixels, imij::Error> {
    use imij::FrameKind;
    let threads = threads.max(1) as usize;
    let padding = padding as usize;
    let mut decoder = imij::FrameDecoder::with_limits(data, &imij::Limits::DEFAULT)?;
    let info = decoder.info().clone();
    let bpp = info.bytes_per_pixel();
    let (mut stride, size) = buffers::layout(&info, padding)?;
    let mut canvas = buffers::filled(size, 0xa5)?;
    let mut pixels = Vec::new();
    let mut headers = String::new();
    // The concatenated result already owns every reconstructed raster. Keep
    // only its ranges for backward references, not a second raster history.
    let mut history: Vec<core::ops::Range<usize>> = Vec::new();
    while let Some(header) = decoder.next_frame()? {
        let bpp = if decoder.kind() == FrameKind::Animation {
            bpp
        } else {
            decoder.frame_info().unwrap().bytes_per_pixel()
        };
        let (w, h) = if decoder.kind() != FrameKind::Animation {
            let page = decoder.frame_info().unwrap();
            let (page_stride, size) = buffers::layout(&page, padding)?;
            stride = page_stride;
            canvas = buffers::filled(size, 0xa5)?;
            (header.width, header.height)
        } else {
            (info.width, info.height)
        };
        if let Some(index) = decoder.reference_index() {
            let reference = &pixels[history[index].clone()];
            let row_bytes = w as usize * bpp;
            for y in 0..h as usize {
                canvas[y * stride..y * stride + row_bytes]
                    .copy_from_slice(&reference[y * row_bytes..(y + 1) * row_bytes]);
            }
        }
        let start = pixels.len();
        let append = (w as usize)
            .checked_mul(bpp)
            .and_then(|row| row.checked_mul(h as usize))
            .ok_or(imij::Error::TooLarge)?;
        buffers::reserve_result(&mut pixels, append)?;
        decoder.decode_into(&mut canvas, stride, threads)?;
        for row in canvas.chunks_exact(stride).take(h as usize) {
            let rb = w as usize * bpp;
            pixels.extend_from_slice(&row[..rb]);
            if !row[rb..].iter().all(|&v| v == 0xa5) {
                return Err(imij::Error::Corrupt("frame padding was written"));
            }
        }
        history.push(start..pixels.len());
        headers.push_str(&format!(
            "{},{},{},{},{},{},{},{},{:?};",
            header.index,
            header.x0,
            header.y0,
            header.width,
            header.height,
            header.duration.num,
            header.duration.den,
            header.uses_previous,
            header.coding
        ));
    }
    Ok((info, pixels, headers, decoder.kind()))
}

#[cfg(feature = "full")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(all(test, feature = "full"))]
mod sequence_tests {
    use super::decode_frames_owned;
    use imij::{
        Channels, FrameConfig, ImageInfo, ImageReference, SampleType, SequenceEncoder,
        SequenceItem, SequenceKind,
    };

    #[test]
    fn padded_sequences_reuse_returned_earlier_rasters() {
        for kind in [SequenceKind::Pages, SequenceKind::Animation] {
            for sample in [SampleType::U8, SampleType::F32] {
                let info = ImageInfo::new(33, 17, Channels::Rgba, sample);
                let first: Vec<u8> = (0..info.checked_buffer_len().unwrap())
                    .map(|i| (i * 149 + i / 97) as u8)
                    .collect();
                let second: Vec<u8> = first.iter().map(|b| b ^ 0x69).collect();
                let mut third = first.clone();
                third[41] ^= 3;
                let frames = [first, second, third];
                let mut encoder = SequenceEncoder::new(
                    kind,
                    FrameConfig::new({
                        let mut cfg = imij::ImiEncoderConfig::default();
                        cfg = cfg.with_effort(0.0);
                        cfg = cfg.with_threads(1);
                        cfg
                    }),
                );
                for (i, pixels) in frames.iter().enumerate() {
                    let mut item = SequenceItem::new(pixels, info.row_bytes(), &info);
                    if i == 2 {
                        item = item.with_reference(ImageReference {
                            index: 0,
                            pixels: &frames[0],
                            stride: info.row_bytes(),
                        });
                    }
                    encoder.push(item).unwrap();
                }
                let encoded = imij::add_pixel_checksum(encoder.finish().unwrap()).unwrap();
                for padding in [0, 13] {
                    let (_, actual, headers, _) =
                        decode_frames_owned(&encoded, 1, padding).unwrap();
                    assert_eq!(actual, frames.concat());
                    assert_eq!(headers.matches(';').count(), 3);
                }
            }
        }
    }
}
