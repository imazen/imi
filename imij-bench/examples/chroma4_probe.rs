//! Encode-only frozen pilot, then independent score-rgb-batch specs.
#[path = "../src/chroma4.rs"]
mod chroma4;
use chroma4::{Mode, Prepared};
use serde_json::{Value, json};
use std::{fs, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args().collect();
    if a.len() != 2 {
        return Err("usage: chroma4_probe SPEC.json".into());
    }
    let spec: Value = serde_json::from_slice(&fs::read(&a[1])?)?;
    let out = Path::new(spec["output"].as_str().ok_or("output")?);
    fs::create_dir_all(out)?;
    let mut rows = Vec::new();
    for src in spec["sources"].as_array().ok_or("sources")? {
        let rgb = fs::read(src["source"].as_str().ok_or("source")?)?;
        let (w, h) = (
            src["width"].as_u64().ok_or("width")? as usize,
            src["height"].as_u64().ok_or("height")? as usize,
        );
        if rgb.len() != w * h * 3 {
            return Err("tight RGB8 size".into());
        }
        for color_name in spec["colors"].as_array().ok_or("colors")? {
            let color_name = color_name.as_str().ok_or("color")?;
            let color = match color_name {
                "xyb" => imij::LossyColor::Xyb,
                "ycocg" => imij::LossyColor::YCoCg,
                _ => return Err("color".into()),
            };
            let prepared = Prepared::new(&rgb, w * 3, w, h, color)?;
            for q in spec["qualities"].as_array().ok_or("qualities")? {
                let q = q.as_f64().ok_or("quality")? as f32;
                let id = format!(
                    "{}-{}-q{}",
                    src["image"].as_str().ok_or("image")?,
                    color_name,
                    q
                );
                let dir = out.join(&id);
                fs::create_dir_all(&dir)?;
                let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    let production = prepared.production(&rgb, w * 3, q)?;
                    fs::write(dir.join("production444.imi"), &production)?;
                    fs::write(dir.join("production444.rgb8"), imij::decode(&production)?.1)?;
                    let mut row = json!({"case_id":format!("{id}-production444"),"curve":"chroma4-production444","source":src["source"],"source_sha256":src["source_sha256"],"image":src["image"],"family":src["family"],"image_class":src["image_class"],"width":w,"height":h,"stride_bytes":w*3,"quality":q,"color":color_name,"encoded":dir.join("production444.imi"),"decoded":dir.join("production444.rgb8"),"encoded_bytes":production.len(),"inner_parts_container_params_tables_dc_ac":imij::chroma_lab::byte_parts(&production)?,"status":"complete"});
                    rows.push(row.take());
                    for mode in [Mode::Fixed444, Mode::Fixed420, Mode::Adaptive] {
                        let e = prepared.encode(&production, q, mode)?;
                        let encoded = dir.join(format!("{}.chr4", mode.name()));
                        let decoded = dir.join(format!("{}.rgb8", mode.name()));
                        let recon = chroma4::decode(&e.file)?;
                        if mode == Mode::Fixed444
                            && (e.file[36..] != production || recon != imij::decode(&production)?.1)
                        {
                            return Err("444 coefficient/file/RGB control parity failed".into());
                        }
                        fs::write(&encoded, &e.file)?;
                        fs::write(&decoded, recon)?;
                        rows.push(json!({"case_id":format!("{id}-{}",mode.name()),"curve":mode.name(),"source":src["source"],"source_sha256":src["source_sha256"],"image":src["image"],"family":src["family"],"image_class":src["image_class"],"width":w,"height":h,"stride_bytes":w*3,"quality":q,"color":color_name,"encoded":encoded,"decoded":decoded,"encoded_bytes":e.file.len(),"inner_bytes":e.inner_bytes,"inner_parts_container_params_tables_dc_ac":imij::chroma_lab::byte_parts(&e.file[36+e.map_bytes..])?,"compact_permutation_inner_bytes_diagnostic":e.compact_inner_bytes,"outer_header_bytes":36,"map_bytes":e.map_bytes,"blocks":e.blocks,"selected420":e.selected,"selection_fraction":e.selected as f64/e.blocks as f64,"estimated_chroma_bits_surrogate":e.estimated_chroma_bits,"status":"complete"}));
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    for (curve, suffix) in [
                        ("chroma4-production444", "production444"),
                        ("chroma4-wrapper444", "chroma4-wrapper444"),
                        ("chroma4-fixed420", "chroma4-fixed420"),
                        ("chroma4-adaptive", "chroma4-adaptive"),
                    ] {
                        let case = format!("{id}-{suffix}");
                        if !rows.iter().any(|r| r["case_id"] == case) {
                            rows.push(json!({"case_id":case,"curve":curve,"image":src["image"],"family":src["family"],"source":src["source"],"source_sha256":src["source_sha256"],"width":w,"height":h,"quality":q,"color":color_name,"status":"failed","error":e.to_string()}));
                        }
                    }
                }
                fs::write(
                    out.join("encode-results.json"),
                    serde_json::to_vec_pretty(&rows)?,
                )?;
                println!("{id}: {} rows", rows.len());
            }
        }
    }
    Ok(())
}
