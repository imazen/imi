//! Untimed, separately built wire inspection; timing uses the default feature build.
#![forbid(unsafe_code)]
fn main() {
    let path = std::env::args().nth(1).expect("encoded file");
    let file = std::fs::read(path).unwrap();
    let (mode, table_bytes) = imij::lab::lossy_ac_table_bytes(&file).unwrap();
    assert!(mode == 0 || mode == 3);
    println!("entropy={mode}");
    println!("table_bytes={table_bytes}");
}
