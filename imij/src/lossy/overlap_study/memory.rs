//! One route per process, with no full reference raster or preparatory decode.
//! Measure this process externally with time -v; this module does not time work.
use super::*;

pub(super) fn run() {
    use std::io::Read;
    let required = |key| std::env::var(key).expect(key);
    let directory = required("OVERLAP7_INPUT");
    let source = required("OVERLAP7_TIME_SOURCE");
    let quality = required("OVERLAP7_TIME_Q");
    let color = required("OVERLAP7_TIME_COLOR");
    let entropy = required("OVERLAP7_TIME_ENTROPY");
    let mode_name = required("OVERLAP7_MEMORY_MODE");
    let route = required("OVERLAP7_MEMORY_ROUTE");
    let threads: usize = required("OVERLAP7_TIME_THREADS").parse().unwrap();
    assert!([1, 2, 4, 8].contains(&threads));
    let table = std::fs::read_to_string(Path::new(&directory).join("quality.tsv")).unwrap();
    let rows: Vec<Vec<&str>> = table
        .lines()
        .skip(1)
        .map(|l| l.split('\t').collect())
        .filter(|f: &Vec<&str>| {
            f[0] == source
                && f[3] == mode_name
                && f[4] == color
                && f[5] == entropy
                && f[6] == quality
        })
        .collect();
    assert_eq!(rows.len(), 1, "one retained carrier required");
    let f = &rows[0];
    assert!(
        matches!(f.len(), 22..=24),
        "digest-bearing carrier schema required"
    );
    let mode = match mode_name.as_str() {
        "base" => Mode::Base,
        "own" => Mode::Own,
        "blend" => Mode::Blend,
        _ => panic!("mode"),
    };
    let (w, h): (usize, usize) = (f[1].parse().unwrap(), f[2].parse().unwrap());
    let mut file = std::fs::read(Path::new(&directory).join(f[11])).unwrap();
    let encoded_sha256 = sha256(&file);
    assert_eq!(encoded_sha256, f[18]);
    assert_eq!(file.len(), f[9].parse::<usize>().unwrap());
    if mode != Mode::Base {
        assert_eq!(&file[..8], b"OV7LAB01");
        assert_eq!(
            u32::from_le_bytes(file[8..12].try_into().unwrap()) as usize,
            w
        );
        assert_eq!(
            u32::from_le_bytes(file[12..16].try_into().unwrap()) as usize,
            h
        );
        assert_eq!(
            u32::from_le_bytes(file[16..20].try_into().unwrap()),
            if mode == Mode::Own { 1 } else { 2 }
        );
        assert_eq!(&file[20..24], &[0; 4]);
        file.drain(..24);
    }
    let prefix = route == "firstrow" || route == "production_firstrow";
    let expected = if prefix {
        let mut row = vec![0; w * 3];
        std::fs::File::open(Path::new(&directory).join(f[12]))
            .unwrap()
            .read_exact(&mut row)
            .unwrap();
        sha256(&row)
    } else {
        f[19].to_owned()
    };
    let mut digest = Sha256::new();
    let mut delivered = 0;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    pool.install(|| match route.as_str() {
        "production" | "full" | "pruned" => {
            let mut out = vec![0; w * h * 3];
            if route == "production" {
                assert_eq!(mode, Mode::Base);
                crate::decode_into_threads(&file, &mut out, w * 3, threads).unwrap();
            } else if route == "pruned" {
                optimized::decode_pruned(&file, w, h, w * 3, &mut out, mode, threads, 32).unwrap();
            } else {
                optimized::decode_chunked(&file, w, h, w * 3, &mut out, mode, threads, 32).unwrap();
            }
            digest.update(&out);
            delivered = h;
        }
        "stream" | "firstrow" => {
            assert_eq!(threads, 1, "prototype callback route is serial");
            optimized::decode_stream(&file, w, h, mode, 32, |y, row| {
                assert_eq!(y, delivered);
                assert_eq!(row.len(), w * 3);
                digest.update(row);
                delivered += 1;
                !prefix
            })
            .unwrap();
        }
        "production_stream" | "production_firstrow" => {
            assert_eq!(mode, Mode::Base);
            assert_eq!(threads, 1);
            let mut row = vec![0; w * 3];
            let mut decoder = crate::StreamDecoder::new(&file).unwrap();
            while decoder.next_row(&mut row).unwrap() {
                digest.update(&row);
                delivered += 1;
                if prefix {
                    break;
                }
            }
        }
        _ => panic!("unknown memory route"),
    });
    assert_eq!(delivered, if prefix { 1 } else { h });
    let actual: String = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        actual, expected,
        "measured route must match retained pixels"
    );
    let record = serde_json::json!({"source":source,"width":w,"height":h,"mode":mode_name,"route":route,"threads":threads,
        "encoded_sha256":encoded_sha256,"delivered_rows":delivered,"decoded_sha256":actual,
        "scope":"Single complete route including parse, reconstruction, caller output where applicable and SHA256 validation. No full reference allocation or predecode. External process max RSS required; not elapsed-speed evidence."});
    std::fs::write(
        required("OVERLAP7_OUTPUT"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
}
