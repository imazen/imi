//! Command-line interface: `decode` one file, or `check` a sealed fixture set.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("decode") => {
            let (Some(input), Some(output)) = (args.get(2), args.get(3)) else {
                eprintln!("usage: imij-oracle decode <in.imi> <out.raw>");
                return ExitCode::from(2);
            };
            let data = match std::fs::read(input) {
                Ok(data) => data,
                Err(err) => {
                    eprintln!("read {input}: {err}");
                    return ExitCode::FAILURE;
                }
            };
            match imij_oracle::decode(&data) {
                Ok(decoded) => {
                    let mut raw = Vec::new();
                    for frame in &decoded.frames {
                        raw.extend_from_slice(frame);
                    }
                    if let Err(err) = std::fs::write(output, &raw) {
                        eprintln!("write {output}: {err}");
                        return ExitCode::FAILURE;
                    }
                    println!(
                        "decoded {}x{} sample={} channels={} frames={}",
                        decoded.width,
                        decoded.height,
                        decoded.sample,
                        decoded.channels,
                        decoded.frames.len()
                    );
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("{err}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check") => {
            let Some(path) = args.get(2) else {
                eprintln!("usage: imij-oracle check <set-dir-or-sha256.json>");
                return ExitCode::from(2);
            };
            match imij_oracle::check::check_set(std::path::Path::new(path)) {
                Ok(report) => {
                    print!("{}", report.text);
                    if report.failures == 0 {
                        ExitCode::SUCCESS
                    } else {
                        eprintln!("{} fixture failures", report.failures);
                        ExitCode::FAILURE
                    }
                }
                Err(err) => {
                    eprintln!("{err}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!(
                "usage: imij-oracle <decode <in.imi> <out.raw> | check <set-dir-or-sha256.json>>"
            );
            ExitCode::from(2)
        }
    }
}
