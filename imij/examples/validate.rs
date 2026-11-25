//! Fully decode and validate IMI files; `validate --help` describes profiles and limits.
#[path = "support/validate_cli.rs"]
mod validate_cli;
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match validate_cli::run(&args) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::from(2)
        }
    }
}
