#![forbid(unsafe_code)]
mod cost;
mod importer;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = if args.first().is_some_and(|s| s == "--cost") {
        cost::run(&args[1..])
    } else {
        importer::import(&args)
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
