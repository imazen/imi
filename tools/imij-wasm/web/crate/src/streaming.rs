//! Optional measurement of the existing caller-fed codec interface on bounded cells.
use imij::codec::{DecodeConfig, FeedStatus};
use wasm_bindgen::prelude::*;
use zencodec::decode::DecoderConfig;

/// Decode the same complete bounded cell by feeding small chunks through the
/// production incremental interface. Final checks must pass before pixels escape.
/// This probe compares decoder work, not HTTP latency or a whole-image ROI walk.
#[wasm_bindgen(js_name = decodeStreaming)]
pub fn decode_streaming(data: &[u8], chunk_bytes: u32) -> Result<js_sys::Object, JsError> {
    if chunk_bytes == 0 || chunk_bytes > 65_536 {
        return Err(JsError::new("stream chunk must be 1..65536 bytes"));
    }
    let start = super::now_ms();
    let mut decoder = DecodeConfig::default()
        .job()
        .feed_decoder(chunk_bytes as usize)
        .map_err(super::js_err)?;
    let mut at = 0;
    let mut pixels = Vec::new();
    let mut rows = 0;
    loop {
        match decoder.advance().map_err(super::js_err)? {
            FeedStatus::NeedInput => {
                if at == data.len() {
                    decoder.finish_input().map_err(super::js_err)?;
                } else {
                    let end = data.len().min(at + chunk_bytes as usize);
                    let n = decoder.push(&data[at..end]).map_err(super::js_err)?;
                    if n == 0 {
                        return Err(JsError::new("stream input made no progress"));
                    }
                    at += n;
                }
            }
            FeedStatus::Row { y } => {
                if y != rows {
                    return Err(JsError::new("stream row order"));
                }
                let row = decoder
                    .row()
                    .map_err(super::js_err)?
                    .ok_or_else(|| JsError::new("stream row missing"))?;
                let bytes = row.row(0);
                super::buffers::reserve_result(&mut pixels, bytes.len()).map_err(super::js_err)?;
                pixels.extend_from_slice(bytes);
                rows += 1;
            }
            FeedStatus::Complete => break,
            _ => return Err(JsError::new("unknown stream status")),
        }
    }
    let info = decoder
        .native_header()
        .ok_or_else(|| JsError::new("stream header missing"))?;
    let (_, total) = super::buffers::layout(&info, 0).map_err(super::js_err)?;
    if rows != info.height || pixels.len() != total || at != data.len() {
        return Err(JsError::new("stream output extent"));
    }
    let out = js_sys::Object::new();
    super::set_info(&out, &info);
    super::set(&out, "pixels", js_sys::Uint8Array::from(&pixels[..]));
    super::set(&out, "ms", super::now_ms() - start);
    Ok(out)
}
