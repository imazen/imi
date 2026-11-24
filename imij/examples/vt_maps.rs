//! `vt_maps FILE...` prints (mode, bits) of each channel of lossless IMI
//! files — the value-table sizes behind decode-speed studies.
fn main() {
    for f in std::env::args().skip(1) {
        let data = std::fs::read(&f).unwrap();
        let maps = imij::lab::channel_maps(&data).unwrap();
        let s: Vec<String> = maps.iter().map(|(m, b)| format!("{m}:{b}")).collect();
        println!("{f}\t{}", s.join(" "));
    }
}
