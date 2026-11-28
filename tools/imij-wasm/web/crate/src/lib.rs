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
#[cfg(any(
    feature = "full",
    feature = "animations",
    feature = "threads",
    not(feature = "polyfill")
))]
mod buffers;
#[cfg(any(feature = "full", feature = "polyfill"))]
mod encode_img;
#[cfg(any(feature = "full", not(feature = "polyfill")))]
mod native_pyramid;
#[cfg(any(feature = "full", feature = "polyfill"))]
#[path = "../../../src/preview.rs"]
mod preview;
#[cfg(any(feature = "full", not(feature = "polyfill")))]
mod ranges;
#[cfg(feature = "streaming-evaluation")]
mod streaming;
#[cfg(any(feature = "full", feature = "polyfill"))]
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

fn color_space_name(cicp: &imij::Cicp) -> &'static str {
    if cicp.color_primaries == 12
        && cicp.transfer_characteristics == 13
        && cicp.matrix_coefficients == 0
        && cicp.full_range
    {
        "display-p3"
    } else {
        "srgb"
    }
}

fn set_info(out: &js_sys::Object, info: &imij::ImageInfo) {
    set(out, "width", info.width);
    set(out, "height", info.height);
    set(out, "channels", format!("{:?}", info.channels));
    set(out, "sample", format!("{:?}", info.sample));
    set(out, "bytesPerPixel", info.bytes_per_pixel() as u32);
    let cicp = js_sys::Object::new();
    set(&cicp, "primaries", u32::from(info.cicp.color_primaries));
    set(
        &cicp,
        "transfer",
        u32::from(info.cicp.transfer_characteristics),
    );
    set(&cicp, "matrix", u32::from(info.cicp.matrix_coefficients));
    set(&cicp, "fullRange", info.cicp.full_range);
    set(out, "cicp", cicp);
    set(out, "colorSpace", color_space_name(&info.cicp));
}

/// Runtime capability summary for this compiled Wasm bundle.
#[wasm_bindgen]
pub fn capabilities() -> js_sys::Object {
    let out = js_sys::Object::new();
    let profile = if cfg!(feature = "full") {
        "full"
    } else if cfg!(feature = "animations") {
        "polyfill-anim"
    } else if cfg!(feature = "polyfill") {
        "polyfill"
    } else {
        "explorer"
    };
    set(&out, "profile", profile);
    set(
        &out,
        "stills",
        cfg!(any(feature = "full", feature = "polyfill")),
    );
    set(
        &out,
        "animations",
        cfg!(any(feature = "full", feature = "animations")),
    );
    set(
        &out,
        "gigapixel",
        cfg!(any(feature = "full", not(feature = "polyfill"))),
    );
    set(&out, "archives", cfg!(feature = "full"));
    set(
        &out,
        "maxBitsPerChannel",
        if cfg!(feature = "extended") {
            64u32
        } else {
            16u32
        },
    );
    set(&out, "lossy", true);
    set(&out, "lossless", true);
    set(&out, "alpha", true);
    set(&out, "displayP3", true);
    out
}

/// Header fields of a stream: `{ width, height, channels, sample, bytesPerPixel }`.
/// No pixels are decoded.
#[cfg(any(feature = "full", feature = "polyfill"))]
#[wasm_bindgen]
pub fn info(data: &[u8]) -> Result<js_sys::Object, JsError> {
    let (info, _meta) = imij::probe(data).map_err(js_err)?;
    let out = js_sys::Object::new();
    set_info(&out, &info);
    Ok(out)
}

/// Decode into a fresh tightly packed buffer (native-endian samples), with up
/// to `threads` decoder workers on the `threads` build.
#[cfg(all(feature = "polyfill", not(feature = "full"), not(feature = "threads")))]
fn decode_owned(data: &[u8], _threads: u32) -> Result<(imij::ImageInfo, Vec<u8>), JsError> {
    let mut session =
        imij::decode_resources::DecodeSession::new(imij::decode_resources::DecodePolicy::default());
    session.decode(data).map_err(js_err)
}

/// Decode into a fresh tightly packed buffer (native-endian samples), with up
/// to `threads` decoder workers on the `threads` build.
#[cfg(any(feature = "full", all(feature = "polyfill", feature = "threads")))]
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
#[cfg(any(feature = "full", feature = "polyfill"))]
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
#[cfg(any(feature = "full", not(feature = "polyfill")))]
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
#[cfg(any(feature = "full", feature = "polyfill"))]
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

#[cfg(any(feature = "full", feature = "polyfill"))]
fn decode_preview_pixels<'a>(
    info: &imij::ImageInfo,
    pixels: &'a [u8],
) -> Result<(usize, std::borrow::Cow<'a, [u8]>), imij::Error> {
    if info.sample == imij::SampleType::U8 && info.channels == imij::Channels::Rgb {
        return Ok((3, std::borrow::Cow::Borrowed(pixels)));
    }
    if info.sample == imij::SampleType::U8
        && info.channels == imij::Channels::Rgba
        && info.alpha_mode == imij::AlphaMode::Straight
    {
        return Ok((4, std::borrow::Cow::Borrowed(pixels)));
    }
    let rgba = to_rgba8(info, pixels)?;
    Ok((4, std::borrow::Cow::Owned(rgba)))
}

/// Decode an IMI stream and re-encode directly to an uncompressed BMP (`Uint8Array`)
/// inside Wasm (24-bit BGR for opaque RGB8, 32-bit BGRA when alpha is present).
/// When `embed_icc` is true and the image is Display-P3, writes a `BITMAPV5HEADER`
/// with `LCS_PROFILE_EMBEDDED` and the canonical Display-P3 ICC profile.
#[cfg(any(feature = "full", feature = "polyfill"))]
#[wasm_bindgen(js_name = decodeToBmp)]
pub fn decode_to_bmp(
    data: &[u8],
    threads: u32,
    embed_icc: bool,
) -> Result<js_sys::Object, JsError> {
    let t0 = now_ms();
    let (info, pixels) = decode_owned(data, threads)?;
    let t1 = now_ms();
    let (channels, preview) = decode_preview_pixels(&info, &pixels).map_err(js_err)?;
    let bmp = encode_img::encode_bmp(
        &preview,
        info.width,
        info.height,
        channels,
        encode_img::is_display_p3(&info.cicp),
        embed_icc,
    )
    .map_err(js_err)?;
    let t2 = now_ms();
    let out = js_sys::Object::new();
    set_info(&out, &info);
    set(&out, "bytes", js_sys::Uint8Array::from(&bmp[..]));
    set(&out, "mime", "image/bmp");
    set(&out, "threads", threads.max(1));
    set(&out, "decodeMs", t1 - t0);
    set(&out, "encodeMs", t2 - t1);
    set(&out, "ms", t2 - t0);
    Ok(out)
}

/// Decode an IMI stream and re-encode directly to an uncompressed store-mode PNG
/// (`DEFLATE BTYPE=00` stored blocks) with `cICP` and optional Display-P3 `iCCP`.
#[cfg(any(feature = "full", feature = "polyfill"))]
#[wasm_bindgen(js_name = decodeToPng)]
pub fn decode_to_png(
    data: &[u8],
    threads: u32,
    include_iccp: bool,
) -> Result<js_sys::Object, JsError> {
    let t0 = now_ms();
    let (info, pixels) = decode_owned(data, threads)?;
    let t1 = now_ms();
    let (channels, preview) = decode_preview_pixels(&info, &pixels).map_err(js_err)?;
    let png = encode_img::encode_store_png(
        &preview,
        info.width,
        info.height,
        channels,
        info.cicp,
        include_iccp,
    )
    .map_err(js_err)?;
    let t2 = now_ms();
    let out = js_sys::Object::new();
    set_info(&out, &info);
    set(&out, "bytes", js_sys::Uint8Array::from(&png[..]));
    set(&out, "mime", "image/png");
    set(&out, "threads", threads.max(1));
    set(&out, "decodeMs", t1 - t0);
    set(&out, "encodeMs", t2 - t1);
    set(&out, "ms", t2 - t0);
    Ok(out)
}

/// Re-encode tightly packed RGB8 or RGBA8 pixels into an uncompressed BMP.
#[cfg(any(feature = "full", feature = "polyfill"))]
#[wasm_bindgen(js_name = encodeBmp)]
pub fn encode_bmp_wasm(
    pixels: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    display_p3: bool,
    embed_icc: bool,
) -> Result<js_sys::Uint8Array, JsError> {
    let bmp = encode_img::encode_bmp(
        pixels,
        width,
        height,
        channels as usize,
        display_p3,
        embed_icc,
    )
    .map_err(js_err)?;
    Ok(js_sys::Uint8Array::from(&bmp[..]))
}

/// Re-encode tightly packed RGB8 or RGBA8 pixels into an uncompressed store-mode PNG.
/// `cicp` is `[primaries, transfer, matrix, full_range]` (4 bytes; defaults to sRGB if empty).
#[cfg(any(feature = "full", feature = "polyfill"))]
#[wasm_bindgen(js_name = encodeStorePng)]
pub fn encode_store_png_wasm(
    pixels: &[u8],
    width: u32,
    height: u32,
    channels: u32,
    cicp: &[u8],
    include_iccp: bool,
) -> Result<js_sys::Uint8Array, JsError> {
    let cicp = if cicp.len() == 4 {
        imij::Cicp {
            color_primaries: cicp[0],
            transfer_characteristics: cicp[1],
            matrix_coefficients: cicp[2],
            full_range: cicp[3] != 0,
        }
    } else if cicp.is_empty() {
        imij::Cicp::SRGB
    } else {
        return Err(js_err(imij::Error::InvalidInput(
            "cicp must be 0 or 4 bytes",
        )));
    };
    let png =
        encode_img::encode_store_png(pixels, width, height, channels as usize, cicp, include_iccp)
            .map_err(js_err)?;
    Ok(js_sys::Uint8Array::from(&png[..]))
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
#[cfg(any(feature = "full", feature = "animations"))]
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

/// Decode every frame of an animation or multi-page stream into concatenated
/// 8-bit RGBA preview frames (`rgba: Uint8ClampedArray`), plus per-frame
/// dimensions (`frameWidths`, `frameHeights`) and delays (`durationsMs`).
/// Supports U8, U16, and F16 frames with alpha and Display-P3 CICP metadata.
#[cfg(any(feature = "full", feature = "animations"))]
#[wasm_bindgen(js_name = decodeFramesRgba)]
pub fn decode_frames_rgba(data: &[u8], threads: u32) -> Result<js_sys::Object, JsError> {
    let t0 = now_ms();
    let (info, rgba, widths, heights, durations_ms, color_spaces, kind) =
        decode_frames_rgba_owned(data, threads).map_err(js_err)?;
    let rgba_len = u32::try_from(rgba.len()).map_err(|_| js_err(imij::Error::TooLarge))?;
    let out = js_sys::Object::new();
    set_info(&out, &info);
    let array = js_sys::Uint8ClampedArray::new_with_length(rgba_len);
    array.copy_from(&rgba);
    set(&out, "rgba", array);
    set(&out, "frameCount", durations_ms.len() as u32);
    set(&out, "frameWidths", js_sys::Uint32Array::from(&widths[..]));
    set(
        &out,
        "frameHeights",
        js_sys::Uint32Array::from(&heights[..]),
    );
    set(
        &out,
        "durationsMs",
        js_sys::Uint32Array::from(&durations_ms[..]),
    );
    let cs_array = js_sys::Array::new_with_length(color_spaces.len() as u32);
    for (i, cs) in color_spaces.iter().enumerate() {
        cs_array.set(i as u32, JsValue::from_str(cs));
    }
    set(&out, "frameColorSpaces", cs_array);
    set(&out, "kind", format!("{kind:?}"));
    set(&out, "threads", threads.max(1));
    set(&out, "ms", now_ms() - t0);
    Ok(out)
}

#[cfg(any(feature = "full", feature = "animations"))]
type FramePixels = (imij::ImageInfo, Vec<u8>, String, imij::FrameKind);

#[cfg(any(feature = "full", feature = "animations"))]
type RgbaFramesResult = (
    imij::ImageInfo,
    Vec<u8>,
    Vec<u32>,
    Vec<u32>,
    Vec<u32>,
    Vec<&'static str>,
    imij::FrameKind,
);

#[cfg(any(feature = "full", feature = "animations"))]
const MAX_RGBA_FRAMES: usize = 65_536;

#[cfg(any(feature = "full", feature = "animations"))]
fn decode_frames_rgba_owned(data: &[u8], threads: u32) -> Result<RgbaFramesResult, imij::Error> {
    use imij::FrameKind;
    let threads = threads.max(1) as usize;
    let mut decoder = imij::FrameDecoder::with_limits(data, &imij::Limits::DEFAULT)?;
    let info = decoder.info().clone();
    let (mut stride, size) = buffers::layout(&info, 0)?;
    let mut canvas = buffers::filled(size, 0)?;
    let mut raw_history = Vec::new();
    let mut history_ranges: Vec<core::ops::Range<usize>> = Vec::new();
    let mut rgba_all = Vec::new();
    let mut widths = Vec::new();
    let mut heights = Vec::new();
    let mut durations_ms = Vec::new();
    let mut color_spaces = Vec::new();
    while let Some(header) = decoder.next_frame()? {
        let frame_info = if decoder.kind() == FrameKind::Animation {
            info.clone()
        } else {
            let page = decoder.frame_info().unwrap().clone();
            let (page_stride, page_size) = buffers::layout(&page, 0)?;
            stride = page_stride;
            canvas = buffers::filled(page_size, 0)?;
            page
        };
        if let Some(index) = decoder.reference_index() {
            let reference = &raw_history[history_ranges[index].clone()];
            let copy_len = canvas.len().min(reference.len());
            canvas[..copy_len].copy_from_slice(&reference[..copy_len]);
        }
        decoder.decode_into(&mut canvas, stride, threads)?;
        let rgba = to_rgba8(&frame_info, &canvas)?;
        buffers::reserve_result(&mut rgba_all, rgba.len())?;
        rgba_all.extend_from_slice(&rgba);
        buffers::try_push_bounded(&mut widths, frame_info.width, MAX_RGBA_FRAMES)?;
        buffers::try_push_bounded(&mut heights, frame_info.height, MAX_RGBA_FRAMES)?;
        let dur_ms = if header.duration.den > 0 && header.duration.num > 0 {
            let den = u64::from(header.duration.den);
            let num_ms = header
                .duration
                .num
                .saturating_mul(1000)
                .saturating_add(den / 2);
            (num_ms / den).clamp(10, 60_000) as u32
        } else if decoder.kind() == FrameKind::Animation {
            100
        } else {
            0
        };
        buffers::try_push_bounded(&mut durations_ms, dur_ms, MAX_RGBA_FRAMES)?;
        buffers::try_push_bounded(
            &mut color_spaces,
            color_space_name(&frame_info.cicp),
            MAX_RGBA_FRAMES,
        )?;
        let hist_start = raw_history.len();
        buffers::reserve_result(&mut raw_history, canvas.len())?;
        raw_history.extend_from_slice(&canvas);
        buffers::try_push_bounded(
            &mut history_ranges,
            hist_start..raw_history.len(),
            MAX_RGBA_FRAMES,
        )?;
    }
    Ok((
        info,
        rgba_all,
        widths,
        heights,
        durations_ms,
        color_spaces,
        decoder.kind(),
    ))
}

#[cfg(any(feature = "full", feature = "animations"))]
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

#[cfg(any(feature = "full", feature = "polyfill"))]
fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(all(test, any(feature = "full", feature = "polyfill")))]
mod polyfill_16bpp_alpha_tests {
    use super::{decode_owned, decode_preview_pixels, encode_img};
    use imij::{
        Channels, ImageInfo, ImageRef, ImiEncoderConfig, LossyQuality, Metadata, SampleType,
    };

    #[test]
    fn stills_up_to_16bpp_lossy_lossless_and_alpha_roundtrip() {
        let cases = [
            (Channels::Rgb, SampleType::U8, true),
            (Channels::Rgb, SampleType::U8, false),
            (Channels::Rgba, SampleType::U8, true),
            (Channels::Rgba, SampleType::U8, false),
            (Channels::GrayAlpha, SampleType::U8, false),
            (Channels::Gray, SampleType::U16, false),
            (Channels::Rgb, SampleType::U16, true),
            (Channels::Rgb, SampleType::U16, false),
            (Channels::Rgba, SampleType::U16, true),
            (Channels::Rgba, SampleType::U16, false),
            (Channels::Rgba, SampleType::F16, false),
        ];
        for (ch, st, is_lossy) in cases {
            let (w, h) = (32u32, 24u32);
            let mut info = ImageInfo::new(w, h, ch, st);
            info.cicp = imij::Cicp {
                color_primaries: 12,
                transfer_characteristics: 13,
                matrix_coefficients: 0,
                full_range: true,
            };
            let nc = ch.count();
            let mut raw = Vec::new();
            for y in 0..h {
                for x in 0..w {
                    for c in 0..nc {
                        let is_alpha = ch.has_alpha() && c == nc - 1;
                        match st {
                            SampleType::U8 => {
                                let v = if is_alpha {
                                    (x * 8).min(255) as u8
                                } else {
                                    ((x * 7 + y * 5 + c as u32 * 40) & 255) as u8
                                };
                                raw.push(v);
                            }
                            SampleType::U16 => {
                                let v: u16 = if is_alpha {
                                    (x as u16) * 2048
                                } else {
                                    ((x * 2000 + y * 1500 + c as u32 * 9000) & 0xffff) as u16
                                };
                                raw.extend_from_slice(&v.to_ne_bytes());
                            }
                            SampleType::F16 => {
                                // Exact IEEE-754 f16 bits for 0.0, 0.25, 0.5, 0.75, 1.0
                                let lut: [u16; 5] = [0x0000, 0x3400, 0x3800, 0x3a00, 0x3c00];
                                let idx = if is_alpha {
                                    (x as usize) % 5
                                } else {
                                    (x as usize + y as usize + c) % 5
                                };
                                raw.extend_from_slice(&lut[idx].to_ne_bytes());
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
            let cfg = if is_lossy {
                ImiEncoderConfig::lossy(LossyQuality::Ssim2(80.0)).with_threads(1)
            } else {
                ImiEncoderConfig::lossless()
                    .with_effort(0.0)
                    .with_threads(1)
            };
            let encoded = cfg
                .encode_samples(ImageRef::packed(&raw, &info), &Metadata::default())
                .unwrap();
            let (dinfo, dpx) = decode_owned(&encoded, 1).unwrap();
            assert_eq!(dinfo.width, w);
            assert_eq!(dinfo.height, h);
            assert_eq!(dinfo.channels, ch);
            assert_eq!(dinfo.sample, st);
            assert_eq!(dinfo.cicp, info.cicp);
            if !is_lossy {
                assert_eq!(dpx, raw, "lossless pixel mismatch for {ch:?}/{st:?}");
            }
            let (out_ch, preview) = decode_preview_pixels(&dinfo, &dpx).unwrap();
            if ch.has_alpha() {
                assert_eq!(out_ch, 4);
                // Verify alpha variation survived preview conversion (not flattened to 255!)
                let alphas: Vec<u8> = preview.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
                assert!(alphas.iter().any(|&a| a < 100) && alphas.iter().any(|&a| a > 180));
            }
            let png =
                encode_img::encode_store_png(&preview, w, h, out_ch, dinfo.cicp, true).unwrap();
            assert_eq!(&png[0..8], b"\x89PNG\r\n\x1a\n");
            let bmp = encode_img::encode_bmp(&preview, w, h, out_ch, true, true).unwrap();
            assert_eq!(&bmp[0..2], b"BM");
        }
    }
}

#[cfg(all(test, any(feature = "full", feature = "animations")))]
mod sequence_tests {
    use super::{decode_frames_owned, decode_frames_rgba_owned};
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
                if sample == SampleType::U8 {
                    let (_, rgba, ws, hs, durs, css, _) =
                        decode_frames_rgba_owned(&encoded, 1).unwrap();
                    assert_eq!(ws, [33, 33, 33]);
                    assert_eq!(hs, [17, 17, 17]);
                    assert_eq!(durs.len(), 3);
                    assert_eq!(css, ["srgb", "srgb", "srgb"]);
                    assert_eq!(rgba, frames.concat());
                }
            }
        }
    }
}
