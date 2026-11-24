//! C ABI for the ASP.NET server (`dotnet/`): one encode session per request,
//! driven the way a .NET host drives zencodec through P/Invoke. Pixels stay
//! in .NET memory (pinned) and are read in place; encoded bytes are read
//! from the session's buffer, which .NET copies into its `PipeWriter`.
//!
//! This module is the bench's only `unsafe` code: building slices from the
//! pointers .NET passes. Each function documents what it requires.
#![allow(unsafe_code)]

use crate::{Codec, RGB8};
use zencodec::encode::DynEncoder;
use zenpixels::PixelSlice;

/// One encode: the encoder, then (after `finish` or a one-shot encode) the
/// remaining bytes.
pub struct Session {
    encoder: Option<Box<dyn DynEncoder>>,
    tail: Vec<u8>,
    tail_sent: usize,
}

/// Create a session for `codec` (UTF-8, `codec_len` bytes) at `width`.
/// Returns null for an unknown codec.
///
/// # Safety
/// `codec` must point to `codec_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_new(codec: *const u8, codec_len: usize, width: u32) -> *mut Session {
    // SAFETY: the caller guarantees `codec_len` readable bytes.
    let name = unsafe { std::slice::from_raw_parts(codec, codec_len) };
    let Some(codec) = std::str::from_utf8(name).ok().and_then(Codec::parse) else {
        return std::ptr::null_mut();
    };
    match codec.encoder(width) {
        Ok(enc) => Box::into_raw(Box::new(Session {
            encoder: Some(enc),
            tail: Vec::new(),
            tail_sent: 0,
        })),
        Err(_) => std::ptr::null_mut(),
    }
}

/// `begin`: the lag in rows when output streams, -1 when it arrives at
/// finish, -2 on error.
///
/// # Safety
/// `s` must come from [`sh_new`] and not be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_begin(s: *mut Session, width: u32, height: u32) -> i64 {
    // SAFETY: the caller guarantees a live session.
    let s = unsafe { &mut *s };
    match s.encoder.as_mut().map(|e| e.begin(width, height, RGB8)) {
        Some(Ok(t)) => t.lag_rows().map_or(-1, i64::from),
        _ => -2,
    }
}

/// Push `rows` rows of RGB8 (`stride` bytes apart, `len` bytes in all) of a
/// `width`-wide image. 0 on success, -1 on error.
///
/// # Safety
/// `s` as for [`sh_begin`]; `pixels` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_push(
    s: *mut Session,
    pixels: *const u8,
    len: usize,
    width: u32,
    rows: u32,
    stride: usize,
) -> i32 {
    // SAFETY: the caller guarantees a live session and `len` bytes.
    let (s, px) = unsafe { (&mut *s, std::slice::from_raw_parts(pixels, len)) };
    let Ok(slice) = PixelSlice::new(px, width, rows, stride, RGB8) else {
        return -1;
    };
    match s.encoder.as_mut().map(|e| e.push_rows(slice)) {
        Some(Ok(())) => 0,
        _ => -1,
    }
}

/// The bytes ready to send: their count, with `*out` set to the first.
/// Valid until the next call on this session.
///
/// # Safety
/// `s` as for [`sh_begin`]; `out` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_pending(s: *mut Session, out: *mut *const u8) -> usize {
    // SAFETY: the caller guarantees a live session and a writable `out`.
    let s = unsafe { &mut *s };
    let bytes: &[u8] = match &s.encoder {
        Some(e) => e.pending_output(),
        None => &s.tail[s.tail_sent..],
    };
    // SAFETY: as above.
    unsafe { *out = bytes.as_ptr() };
    bytes.len()
}

/// Mark the first `n` pending bytes as sent.
///
/// # Safety
/// `s` as for [`sh_begin`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_consume(s: *mut Session, n: usize) {
    // SAFETY: the caller guarantees a live session.
    let s = unsafe { &mut *s };
    match s.encoder.as_mut() {
        Some(e) => e.consume_output(n),
        None => s.tail_sent = (s.tail_sent + n).min(s.tail.len()),
    }
}

/// `finish`: the remaining bytes become pending. 0 on success, -1 on error.
///
/// # Safety
/// `s` as for [`sh_begin`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_finish(s: *mut Session) -> i32 {
    // SAFETY: the caller guarantees a live session.
    let s = unsafe { &mut *s };
    match s.encoder.take().map(|e| e.finish()) {
        Some(Ok(out)) => {
            s.tail = out.into_vec();
            s.tail_sent = 0;
            0
        }
        _ => -1,
    }
}

/// One-shot `encode` of a `width` x `height` RGB8 image; the file becomes
/// pending. 0 on success, -1 on error.
///
/// # Safety
/// `s` as for [`sh_begin`]; `pixels` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_encode(
    s: *mut Session,
    pixels: *const u8,
    len: usize,
    width: u32,
    height: u32,
    stride: usize,
) -> i32 {
    // SAFETY: the caller guarantees a live session and `len` bytes.
    let (s, px) = unsafe { (&mut *s, std::slice::from_raw_parts(pixels, len)) };
    let Ok(slice) = PixelSlice::new(px, width, height, stride, RGB8) else {
        return -1;
    };
    match s.encoder.take().map(|e| e.encode(slice)) {
        Some(Ok(out)) => {
            s.tail = out.into_vec();
            s.tail_sent = 0;
            0
        }
        _ => -1,
    }
}

/// Free a session.
///
/// # Safety
/// `s` must come from [`sh_new`] and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_free(s: *mut Session) {
    if !s.is_null() {
        // SAFETY: the caller hands back ownership.
        drop(unsafe { Box::from_raw(s) });
    }
}

/// Decode the photo at `path` (UTF-8) to RGB8 into `out` (`cap` bytes).
/// Returns the bytes needed (call with `cap` 0 to size the buffer) and sets
/// the dimensions, or 0 on error.
///
/// # Safety
/// `path` must point to `path_len` readable bytes, `out` to `cap` writable
/// bytes (or be null with `cap` 0), `width`/`height` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sh_load(
    path: *const u8,
    path_len: usize,
    out: *mut u8,
    cap: usize,
    width: *mut u32,
    height: *mut u32,
) -> usize {
    // SAFETY: the caller guarantees `path_len` readable bytes.
    let path = unsafe { std::slice::from_raw_parts(path, path_len) };
    let Ok(path) = std::str::from_utf8(path) else {
        return 0;
    };
    let src = match path {
        p if p.starts_with("noise:") => {
            let (w, h) = p[6..].split_once('x').unwrap_or(("1024", "1024"));
            crate::Source::noise(w.parse().unwrap_or(1024), h.parse().unwrap_or(1024))
        }
        p => match crate::Source::load(std::path::Path::new(p)) {
            Ok(s) => s,
            Err(_) => return 0,
        },
    };
    // SAFETY: the caller guarantees writable dimensions.
    unsafe {
        *width = src.width;
        *height = src.height;
    }
    if cap >= src.pixels.len() {
        // SAFETY: the caller guarantees `cap` writable bytes.
        let dst = unsafe { std::slice::from_raw_parts_mut(out, cap) };
        dst[..src.pixels.len()].copy_from_slice(&src.pixels);
    }
    src.pixels.len()
}
