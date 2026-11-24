// `frames` target: animations and pages, every frame.
//
// [`FrameDecoder::with_limits`] walks the `aNIM`/`fRAM` structure and
// [`FrameDecoder::decode_into`] writes each frame into the canvas (an
// animation frame may keep the previous canvas pixels) or the page buffer.
// The first animation frame decoded with 4 threads must equal the 1-thread
// pixels. Frames processed are capped only to bound one input's work; the
// parser already bounds the frame list by the input size.

include!("common_core.rs");

use imij::{FrameDecoder, FrameKind};

/// Most frames one input may decode (all frames of real seeds fit).
const MAX_FRAMES: usize = 1024;

pub fn run(data: &[u8]) {
    let limits = decode_limits();
    let mut dec = match FrameDecoder::with_limits(data, &limits) {
        Ok(d) => d,
        Err(_) => return,
    };
    let kind = dec.kind();
    let _ = dec.loop_count();
    let count = dec.frame_count();
    let canvas_info = dec.info().clone();
    let Some(canvas_len) = canvas_info.checked_buffer_len() else {
        return;
    };
    if canvas_len > MAX_BYTES as usize {
        return;
    }
    let mut history: Vec<(Vec<u8>, usize)> = Vec::new();
    let mut retained = 0usize;
    while let Ok(Some(fh)) = dec.next_frame() {
        let info = if kind == FrameKind::Animation {
            canvas_info.clone()
        } else if let Some(info) = dec.frame_info() {
            info
        } else {
            break;
        };
        let Some(len) = info.checked_buffer_len() else {
            return;
        };
        if len > MAX_BYTES as usize || retained.saturating_add(len) > MAX_WORKING as usize {
            return;
        }
        let stride = info.row_bytes();
        let mut pixels = vec![0u8; len];
        if let Some(index) = dec.reference_index() {
            let Some((reference, reference_stride)) = history.get(index) else {
                break;
            };
            if *reference_stride != stride || reference.len() != len {
                break;
            }
            pixels.copy_from_slice(reference);
        }
        // Both routes start from the same unmodified reference, including empty
        // legacy frames and references to nonprevious unified items.
        let mut alt = pixels.clone();
        let one = dec.decode_into(&mut pixels, stride, 1);
        let four = dec.decode_into(&mut alt, stride, 4);
        assert_eq!(
            one.is_ok(),
            four.is_ok(),
            "frame threads disagree on success"
        );
        if one.is_err() {
            break;
        }
        assert_eq!(alt, pixels, "frame threads differ");
        retained += len;
        history.push((pixels, stride));
        if history.len() >= MAX_FRAMES || history.len() >= count {
            break;
        }
        let _ = fh;
    }
    let _ = dec.frame_metadata();
}
