//! Correctness-only parity against the frozen isolated joint-map experiment.
#![forbid(unsafe_code)]
use serde_json::{Value, json};
use std::{error::Error, fs};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3, "hdr_joint_parity V4_RESULTS OUTPUT_JSON");
    let rows: Value = serde_json::from_slice(&fs::read(&args[1])?)?;
    let mut verified = Vec::new();
    for row in rows.as_array().ok_or("results is not an array")? {
        if row["mode"] != "sampled_select" {
            continue;
        }
        let image = row["image"].as_str().ok_or("missing image")?;
        let reference = fs::read(row["encoded"].as_str().ok_or("missing file")?)?;
        let (info, pixels) = imij::decode(&reference)?;
        let effort = if image == "synthetic-effort0" {
            0.0
        } else {
            10.0
        };
        let mut cfg = imij::ImiEncoderConfig::default().with_effort(effort);
        if image == "synthetic-no-decor" {
            cfg = cfg.tune_lossless(|t| {
                t.decor_mask = 1;
            });
        }
        let actual = cfg.encode_samples(
            imij::ImageRef::new(&pixels, info.row_bytes(), &info),
            &imij::Metadata::default(),
        )?;
        assert_eq!(actual, reference, "{image} bits{}", row["bits"]);
        verified.push(json!({"image":image,"bits":row["bits"],
            "encoded_sha256":row["encoded_sha256"],"bytes":actual.len(),"byte_exact":true}));
    }
    assert_eq!(verified.len(), 150);
    fs::write(
        &args[2],
        serde_json::to_vec_pretty(&json!({"cases":verified,
        "all150_byte_exact":true,"source":args[1],"timing":false}))?,
    )?;
    Ok(())
}
