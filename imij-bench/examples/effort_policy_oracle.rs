//! Production low-effort byte oracle against frozen verified policies. No timing.
#![forbid(unsafe_code)]
use imij::{Channels, ImageInfo, ImiEncoderConfig, Metadata, SampleType};
use serde::Deserialize;
use serde_json::json;
use std::{error::Error, fs, path::Path};
type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    effort: f32,
    name: String,
    file: String,
    expected_bytes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    image: String,
    path: String,
    width: u32,
    height: u32,
    references: Vec<Reference>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    version: u32,
    sources: Vec<Source>,
}

fn need(value: bool, message: &str) -> Result<()> {
    if value { Ok(()) } else { Err(message.into()) }
}

fn encode(pixels: &[u8], info: &ImageInfo, effort: f32, threads: usize) -> Result<Vec<u8>> {
    let mut config = imij::ImiEncoderConfig::default().with_effort(effort);
    config = config.with_threads(threads);
    Ok(config.encode_samples(
        imij::ImageRef::new(pixels, info.row_bytes(), info),
        &Metadata::default(),
    )?)
}

fn run(spec_file: &Path, output: &Path) -> Result<()> {
    need(fs::metadata(spec_file)?.len() < 128_000, "spec too large")?;
    let spec: Spec = serde_json::from_slice(&fs::read(spec_file)?)?;
    need(
        spec.version == 1 && spec.sources.len() == 36,
        "exact36 source spec required",
    )?;
    // Validate all geometry/reference identities before any encoder work.
    for (index, source) in spec.sources.iter().enumerate() {
        need(
            !source.image.is_empty()
                && source.image.len() <= 96
                && source
                    .image
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !spec.sources[..index]
                    .iter()
                    .any(|s| s.image == source.image),
            "source name duplicate/invalid",
        )?;
        let pixels = u64::from(source.width) * u64::from(source.height);
        need(pixels > 0 && pixels <= 4_194_304, "source geometry bound")?;
        need(
            fs::metadata(&source.path)?.len() == pixels * 3,
            "tight RGB8 source size",
        )?;
        need(
            source.references.len() == 4,
            "four reference policies required",
        )?;
        for (reference, (effort, name)) in source.references.iter().zip([
            (0., "fixed-w-d1"),
            (2.5, "fixed-w-med-d1"),
            (5., "current-e5"),
            (7.5, "current-e7p5"),
        ]) {
            need(
                reference.effort == effort && reference.name == name,
                "policy correspondence changed",
            )?;
            need(
                reference.expected_bytes > 0
                    && reference.expected_bytes <= 128 << 20
                    && fs::metadata(&reference.file)?.len() == reference.expected_bytes as u64,
                "reference size",
            )?;
        }
    }
    fs::create_dir(output)?;
    let mut cases = 0;
    for source in spec.sources {
        let info = ImageInfo::new(source.width, source.height, Channels::Rgb, SampleType::U8);
        let pixels = fs::read(&source.path)?;
        need(
            pixels.len() == info.row_bytes() * source.height as usize,
            "source changed",
        )?;
        let mut rows = Vec::new();
        for reference in source.references {
            let expected = fs::read(&reference.file)?;
            need(
                expected.len() == reference.expected_bytes,
                "reference changed",
            )?;
            let encoded = encode(&pixels, &info, reference.effort, 1)?;
            need(
                encoded == expected,
                "production file differs from frozen candidate/control",
            )?;
            let (decoded_info, decoded) = imij::decode(&encoded)?;
            need(
                decoded_info == info && decoded == pixels,
                "production header/source inverse differs",
            )?;
            need(
                encode(&pixels, &info, reference.effort, 1)? == encoded,
                "repeat bytes differ",
            )?;
            need(
                encode(&pixels, &info, reference.effort, 4)? == encoded,
                "one/four-thread bytes differ",
            )?;
            rows.push(json!({"image":source.image,"effort":reference.effort,
                "reference_name":reference.name,"reference":reference.file,
                "encoded_bytes":encoded.len(),"exact_frozen_file":true,
                "exact_source_inverse":true,"exact_header":true,"repeat_bytes":true,
                "one_four_thread_bytes":true}));
            cases += 1;
        }
        fs::write(
            output.join(format!("{}.json", source.image)),
            serde_json::to_vec_pretty(&rows)?,
        )?;
    }
    fs::write(
        output.join("complete.json"),
        serde_json::to_vec_pretty(&json!({
            "accepted":true,"sources":36,"cases":cases,"threads":[1,4],
            "encoder_configs":"actual production default masks with explicit efforts0/2.5/5/7.5",
            "reference_map":"0=fixed-w-d1;2.5=fixed-w-med-d1;5=current-e5;7.5=current-e7p5",
            "exact_frozen_files":true,"exact_source_inverse":true,"native_timing":false,
            "new_rgb_or_encoded_copies":false,"sha_contract":"external controller pins source/reference/spec/binary; exact byte equality supplies encoded hash identity"
        }))?,
    )?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [spec, output] => run(Path::new(spec), Path::new(output)),
        _ => Err("usage: effort_policy_oracle SPEC NEW_OUTPUT_DIRECTORY".into()),
    }
}
