//! Evaluation PNG extraction using the existing zenpng corpus loader.
//! Usage: formats-png INPUT.png OUTPUT.raw
//! Writes native-endian tightly packed samples; reports W H CHANNELS SAMPLE.
#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "../corpus.rs"]
mod corpus;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: formats-png INPUT.png OUTPUT.raw".into());
    }
    let img = corpus::load_png("evaluation", std::path::Path::new(&args[0]))?;
    std::fs::write(&args[1], &img.pixels)?;
    println!(
        "{} {} {} {}",
        img.info.width,
        img.info.height,
        img.info.channels.count(),
        match img.info.sample {
            imij::SampleType::U8 => "u8",
            imij::SampleType::U16 => "u16",
            _ => return Err("unexpected PNG sample type".into()),
        }
    );
    Ok(())
}
