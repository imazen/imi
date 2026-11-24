//! Writes the `zen-codestream-trace/0` JSON of a .imi file (lab build):
//! `cargo run --release --features lab --example codestream_trace -- FILE.imi [--rgba OUT.raw] > trace.json`
//! `--rgba` also writes the decoded image as raw RGBA8 for viewers that draw
//! the record regions over real pixels: 8-bit samples as they are, 16-bit
//! samples by their high byte (a preview). Other sample types write no preview.
//! Exits non-zero when the trace's structural self-checks report problems.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args
        .first()
        .expect("usage: codestream_trace FILE.imi [--rgba OUT.raw]");
    let data = std::fs::read(path).expect("read input");
    let json = imij::lab::codestream_trace(&data);
    println!("{json}");
    if let Some(i) = args.iter().position(|a| a == "--rgba") {
        let (info, px) = imij::decode(&data).expect("decode");
        let px: Vec<u8> = match info.sample {
            imij::SampleType::U8 => px,
            imij::SampleType::U16 => px
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| (u16::from_ne_bytes(*s) >> 8) as u8)
                .collect(),
            other => {
                eprintln!("no --rgba preview for {other:?} samples");
                Vec::new()
            }
        };
        let n = info.channels.count();
        let rgba: Vec<u8> = px
            .chunks_exact(n)
            .flat_map(|p| match n {
                1 => [p[0], p[0], p[0], 255],
                2 => [p[0], p[0], p[0], p[1]],
                3 => [p[0], p[1], p[2], 255],
                _ => [p[0], p[1], p[2], p[3]],
            })
            .collect();
        std::fs::write(&args[i + 1], rgba).expect("write rgba");
    }
    if !json.contains("\"problems\":[]") {
        eprintln!("trace self-check problems; see checks.problems");
        std::process::exit(1);
    }
}
