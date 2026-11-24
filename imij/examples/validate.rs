//! Fully decode and validate v3 files; `validate --help` describes resource limits.
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
