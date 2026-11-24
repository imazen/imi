// `header` target: header, info and profile probing only (very high
// exec/s; no pixel allocation).
//
// [`imij::header`], [`imij::probe`] and [`imij::previews`] parse the
// container and the payload heads at most; every entry point must return
// `Ok`/`Err` without panicking on arbitrary bytes.

include!("common_core.rs");

use imij::{header, previews, probe, supported_profiles};

pub fn run(data: &[u8]) {
    if let Ok(h) = header(data) {
        let _ = h.info.width;
        let _ = h.info.height;
        let _ = h.info.channels;
        let _ = h.info.sample;
        let _ = h.info.cicp;
        let _ = h.info.alpha_mode;
        let _ = h.info.channel_bits(0);
        let _ = h.info.channel_bits(1);
        let _ = h.info.channel_bits(2);
        let _ = h.info.channel_bits(3);
        let _ = h.info.bytes_per_pixel();
        let _ = h.info.row_bytes();
        let _ = h.info.checked_buffer_len();
        let _ = h.coding;
        let _ = h.lossy_color;
        let _ = h.decode_speed;
        let _ = h.orientation;
        for p in &h.profiles {
            let _ = (p.profile, p.major, p.minor);
        }
    }
    if let Ok((info, meta)) = probe(data) {
        let _ = (info.width, info.height, info.channels, info.sample);
        let _ = meta.icc.as_ref().map(|v| v.len());
        let _ = meta.exif.as_ref().map(|v| v.len());
        let _ = meta.xmp.as_ref().map(|v| v.len());
        let _ = meta.mastering_display;
        let _ = meta.content_light_level;
        let _ = meta.orientation;
        if let Some(g) = &meta.gain_map {
            let _ = g.image.len();
            let _ = g.alt_cicp;
            let _ = g.alt_icc.as_ref().map(|v| v.len());
        }
        for c in &meta.chunks {
            let _ = c.tag;
            let _ = c.data.len();
        }
    }
    if let Ok(found) = previews(data) {
        for p in found {
            let _ = p.len();
        }
    }
    let _ = supported_profiles().len();
}
