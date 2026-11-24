//! Decodes files and writes the raw pixels to `OUT/<file name>.raw`, for
//! byte-exact regression checks across decoder changes (compare with
//! `sha256sum` or `cmp`).
//!
//! Usage: dechash [--threads T] OUT FILE...

fn main() {
    let mut threads = 1usize;
    let mut rest = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--threads" {
            threads = args.next().unwrap().parse().unwrap();
        } else {
            rest.push(a);
        }
    }
    // COEFGAP_EOB_PROXY=1 (lab): the coefgap study's decode-cost proxy on.
    #[cfg(feature = "lab")]
    if let Ok(v) = std::env::var("COEFGAP_EOB_PROXY") {
        imij::lab::set_eob_bucket_proxy(true);
        // COEFGAP_EOB_PROXY=probe: every bucket offset 1 (decodes go wrong);
        // =pass / =loads: only the bucket pass / only the kernel loads.
        imij::lab::set_eob_bucket_probe((v == "probe") as u32);
        imij::lab::set_eob_bucket_proxy_mode(match v.as_str() {
            "pass" => 2,
            "loads" => 3,
            _ => 1,
        });
    }
    let out = std::path::PathBuf::from(&rest[0]);
    std::fs::create_dir_all(&out).unwrap();
    for path in &rest[1..] {
        let data = std::fs::read(path).unwrap();
        let (info, _) = imij::decode(&data).unwrap();
        let mut buf = vec![0u8; info.row_bytes() * info.height as usize];
        imij::decode_into_threads(&data, &mut buf, info.row_bytes(), threads).unwrap();
        let name = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        std::fs::write(out.join(name + ".raw"), &buf).unwrap();
    }
}
