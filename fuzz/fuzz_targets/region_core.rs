// `region` target: region decoding through [`RegionReader`] over an
// in-memory [`ReadAt`](imij::planes::ReadAt) and the `region::decode_region`
// free functions, with the rectangle taken from trailing input bytes.
//
// Whenever the region and a full decode of the same file both succeed, the
// region's pixels must equal the crop of the full decode (zero tolerance);
// the 1-thread and 4-thread region routes must also agree. Files the region
// reader decodes but the full decoder refuses are legal (the reader stops at
// the end of `DATA`), so only the both-succeed case is asserted.

include!("common_core.rs");

use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;
use imij::{decode_with_limits, region};

/// A rectangle inside `width`x`height` from 8 trailing input bytes, capped
/// so the caller's output stays inside the fuzz limits.
fn rect_from(data: &[u8], width: u32, height: u32, bpp: usize) -> (Rect, usize) {
    let at = data.len().saturating_sub(8);
    let tail = |i: usize| -> u32 {
        let b = data.get(at + i).copied().unwrap_or(0) as u32;
        let c = data.get(at + i + 1).copied().unwrap_or(0) as u32;
        b | (c << 8)
    };
    let x = tail(0) % width;
    let y = tail(2) % height;
    let w = 1 + tail(4) % (width - x);
    let h = 1 + tail(6) % (height - y);
    // Cap the output at the limits the reader enforces, before allocating.
    let mut w = w.min(1024);
    let mut h = h.min(1024);
    while (w as u64) * (h as u64) * bpp as u64 > MAX_BYTES && h > 1 {
        h = h.div_ceil(2);
    }
    while (w as u64) * (h as u64) > MAX_PIXELS && w > 1 {
        w = w.div_ceil(2);
    }
    let rb = w as usize * bpp;
    (Rect::new(x, y, w, h), rb)
}

pub fn run(data: &[u8]) {
    let limits = decode_limits();
    let mut reader = match RegionReader::open_with_limits(SliceReader::new(data), &limits) {
        Ok(r) => r,
        Err(_) => return,
    };
    let info = reader.info().clone();
    if info.width == 0 || info.height == 0 {
        return;
    }
    let _ = reader.header();
    let _ = reader.has_unit_table();
    let _ = reader.used_locator();
    let bpp = info.bytes_per_pixel();
    let (rect, rb) = rect_from(data, info.width, info.height, bpp);
    let h = rect.height() as usize;
    let mut out = vec![0u8; rb * h];
    if reader.decode_region(rect, &mut out, rb).is_ok() {
        // 4-thread region route must agree with the 1-thread route.
        let mut out4 = vec![0u8; rb * h];
        if reader.decode_region_threads(rect, &mut out4, rb, 4).is_ok() {
            assert_eq!(out4, out, "region threads differ");
        }
        let _ = reader.read_stats();
        reader.reset_read_stats();

        // Free-function route (opens its own reader under `Limits::DEFAULT`).
        let mut out_free = vec![0u8; rb * h];
        if region::decode_region(data, rect, &mut out_free, rb).is_ok() {
            assert_eq!(out_free, out, "free region route differs");
        }

        // Memory-amplification route: output plus fetched record bytes near
        // the input and rect sizes. Refusal is legal; success must match.
        let amp_limits = amplification_limits(data.len(), rb * h);
        if let Ok(mut r2) = RegionReader::open_with_limits(SliceReader::new(data), &amp_limits) {
            let mut out_amp = vec![0u8; rb * h];
            if r2.decode_region(rect, &mut out_amp, rb).is_ok() {
                assert_eq!(out_amp, out, "input-sized region budget changed the pixels");
            }
        }

        // Crop of the full decode, when it succeeds under the same limits.
        if let Ok((finfo, full)) = decode_with_limits(data, &limits)
            && finfo.width == info.width
            && finfo.height == info.height
            && finfo.channels == info.channels
            && finfo.sample == info.sample
        {
            let frb = finfo.row_bytes();
            let x0 = rect.x() as usize * bpp;
            let y0 = rect.y() as usize;
            for row in 0..h {
                let region_row = &out[row * rb..row * rb + rb];
                let full_row = &full[(y0 + row) * frb + x0..][..rb];
                assert_eq!(region_row, full_row, "region differs from full decode, row {row}");
            }
        }
    }
}
