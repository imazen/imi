//! Procedural line chart for a separate grid-phase diagnostic.
use super::*;

pub(super) fn run() {
    let name = std::env::var("OVERLAP7_OUTPUT").expect("new chart directory required");
    let output = Path::new(&name);
    std::fs::create_dir(output).unwrap();
    // 112 is divisible by both block strides. Moving a line by 0..55 samples
    // covers every mod-7/mod-8 phase pair while keeping tile origins aligned.
    let (side, phases, rows) = (112usize, 56usize, 16usize);
    let (w, h) = (side * phases, side * rows);
    let mut pixels = vec![0u8; w * h * 3];
    let mut tiles = Vec::new();
    for orientation in 0..4 {
        for context in 0..2 {
            for (wi, width) in [1i32, 3].into_iter().enumerate() {
                let row = orientation * 4 + context * 2 + wi;
                for phase in 0..phases {
                    let (a, b, c) = match orientation {
                        0 => (1, 0, 28 + phase as i32),
                        1 => (0, 1, 28 + phase as i32),
                        2 => (1, -1, phase as i32 - 28),
                        _ => (1, 1, 83 + phase as i32),
                    };
                    for y in 0..side {
                        for x in 0..side {
                            let line = (a * x as i32 + b * y as i32 - c).abs() <= width / 2;
                            let background = if context == 0 {
                                224
                            } else if (x / 8 + y / 8) % 2 == 0 {
                                200
                            } else {
                                248
                            };
                            let value = if line { 32 } else { background };
                            let i = ((row * side + y) * w + phase * side + x) * 3;
                            pixels[i..i + 3].fill(value);
                        }
                    }
                    tiles.push(serde_json::json!({
                        "orientation": orientation, "context": context,
                        "grid_width": width, "phase": phase,
                        "x": phase * side, "y": row * side,
                        "width": side, "height": side, "roi_margin": 16,
                        "line": [a,b,c]
                    }));
                }
            }
        }
    }
    let mut info = ImageInfo::new(w as u32, h as u32, Channels::Rgb, SampleType::U8);
    info.cicp = crate::Cicp::SRGB;
    let cfg = crate::v1::EncodeConfig {
        effort: 0.0,
        threads: 1,
        ..Default::default()
    };
    let file = crate::v1::encode(&pixels, w * 3, &info, &Metadata::default(), &cfg).unwrap();
    let (decoded_info, decoded) = crate::decode(&file).unwrap();
    assert_eq!(
        (decoded_info.width, decoded_info.height),
        (w as u32, h as u32)
    );
    assert_eq!(decoded_info.cicp, crate::Cicp::SRGB);
    assert_eq!(decoded, pixels);
    std::fs::write(output.join("phase.rgb"), &pixels).unwrap();
    std::fs::write(output.join("phase.imi"), &file).unwrap();
    let record = serde_json::json!({
        "width": w, "height": h, "tiles": tiles,
        "source_sha256": sha256(&pixels), "encoded_sha256": sha256(&file),
        "generator_sha256": sha256(include_bytes!("phase.rs")),
        "scope": "Procedural code-value line diagnostic, not natural-image RD evidence. Binary raster grid widths 1/3; diagonal widths are defined in x+-y coordinates, not equal Euclidean coverage. ROI excludes 16 pixels at each tile boundary."
    });
    std::fs::write(
        output.join("SOURCE.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    std::fs::write(output.join("COMPLETE"), b"896\n").unwrap();
}
