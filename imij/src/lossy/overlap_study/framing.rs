//! Standard container completion for the experimental coefficient path.
use super::*;

pub(super) fn finish(frame: Vec<u8>, checksum: Option<crate::HashAlgorithm>) -> Vec<u8> {
    let file = crate::container::finish_unit_table(frame).unwrap();
    let file = crate::container::locate(file).unwrap();
    crate::checksum::finish_file(file, checksum).unwrap()
}

/// Preserve old artifacts; write verified finalized copies and a lineage map.
pub(super) fn run() {
    let directory = std::env::var("OVERLAP7_INPUT").expect("input directory required");
    let input = Path::new(&directory);
    let output_name = std::env::var("OVERLAP7_OUTPUT").expect("new output directory required");
    let output = Path::new(&output_name);
    std::fs::create_dir(output).unwrap();
    let cas_name = std::env::var("OVERLAP7_CAS").expect("canonical CAS required");
    let cas = Path::new(&cas_name);
    let text = std::fs::read_to_string(input.join("quality.tsv")).unwrap();
    let mut lines = text.lines();
    let header = lines.next().expect("table header");
    let names: Vec<_> = header.split('\t').collect();
    let index = |name| names.iter().position(|&s| s == name).expect(name);
    let optional = |name| names.iter().position(|&s| s == name);
    let mut table = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output.join("quality.tsv"))
        .unwrap();
    writeln!(table, "{header}").unwrap();
    let mut lineage = Vec::new();
    let mut source_cache = std::collections::BTreeMap::new();
    for line in lines {
        let mut row: Vec<String> = line.split('\t').map(str::to_owned).collect();
        assert_eq!(row.len(), names.len());
        let source = row[index("source")].clone();
        let w: usize = row[index("width")].parse().unwrap();
        let h: usize = row[index("height")].parse().unwrap();
        let mode = match row[index("mode")].as_str() {
            "base" => Mode::Base,
            "own" => Mode::Own,
            "blend" => Mode::Blend,
            _ => panic!("placement"),
        };
        let original_path = input.join(&row[index("encoded")]);
        let wrapped = std::fs::read(&original_path).unwrap();
        assert_eq!(wrapped.len(), row[index("bytes")].parse::<usize>().unwrap());
        let original_sha = sha256(&wrapped);
        if let Some(k) = optional("encoded_sha256") {
            assert_eq!(original_sha, row[k]);
        }
        let (prefix, frame) = if mode == Mode::Base {
            (&[][..], &wrapped[..])
        } else {
            assert_eq!(&wrapped[..8], b"OV7LAB01");
            assert_eq!(
                u32::from_le_bytes(wrapped[8..12].try_into().unwrap()) as usize,
                w
            );
            assert_eq!(
                u32::from_le_bytes(wrapped[12..16].try_into().unwrap()) as usize,
                h
            );
            assert_eq!(
                u32::from_le_bytes(wrapped[16..20].try_into().unwrap()),
                if mode == Mode::Own { 1 } else { 2 }
            );
            assert_eq!(&wrapped[20..24], &[0; 4]);
            (&wrapped[..24], &wrapped[24..])
        };
        let hp = crate::parse::header::read_header_sized(&mut ByteReader::new(frame)).unwrap();
        let checksum = hp
            .checksum
            .map(|c| c.announced.algorithm().expect("known checksum algorithm"));
        let completed = finish(frame.to_vec(), checksum);
        assert_eq!(
            finish(completed.clone(), checksum),
            completed,
            "standard completion is idempotent for study hint files"
        );
        assert_eq!(
            quantized_lab(frame).unwrap().2,
            quantized_lab(&completed).unwrap().2,
            "finalization must preserve every quantized symbol"
        );
        let decoded_path = input.join(&row[index("decoded")]);
        let expected = std::fs::read(decoded_path).unwrap();
        assert_eq!(expected.len(), w * h * 3);
        let expected_sha = sha256(&expected);
        if let Some(k) = optional("decoded_sha256") {
            assert_eq!(expected_sha, row[k]);
        }
        let mut pixels = vec![0; expected.len()];
        decode(&completed, w, h, w * 3, &mut pixels, mode, 1, false).unwrap();
        assert_eq!(
            pixels, expected,
            "finalized file must retain every decoded pixel"
        );
        let mut new_wrapped = Vec::with_capacity(prefix.len() + completed.len());
        new_wrapped.extend_from_slice(prefix);
        new_wrapped.extend_from_slice(&completed);
        let (encoded, encoded_sha) = store(cas, &new_wrapped, "ov7");
        let (decoded, decoded_sha) = store(cas, &expected, "rgb");
        row[index("encoded")] = encoded.clone();
        row[index("decoded")] = decoded;
        row[index("bytes")] = new_wrapped.len().to_string();
        if let Some(k) = optional("encoded_sha256") {
            row[k] = encoded_sha.clone();
        }
        if let Some(k) = optional("decoded_sha256") {
            row[k] = decoded_sha;
        }
        if !source_cache.contains_key(&source) {
            let path = optional("source_pixels").map_or_else(
                || input.join(format!("{source}.source.rgb")),
                |k| input.join(&row[k]),
            );
            let bytes = std::fs::read(path).unwrap();
            assert_eq!(bytes.len(), w * h * 3);
            let (path, hash) = store(cas, &bytes, "rgb");
            if optional("source_pixels").is_none() {
                std::fs::write(output.join(format!("{source}.source.rgb")), &bytes).unwrap();
            }
            source_cache.insert(source.clone(), (path, hash));
        }
        let (source_path, source_sha) = &source_cache[&source];
        if let Some(k) = optional("source_sha256") {
            assert_eq!(&row[k], source_sha);
        }
        if let Some(k) = optional("source_pixels") {
            row[k] = source_path.clone();
        }
        lineage.push(serde_json::json!({"source":source,"mode":mode.name(),"color":row[index("color")],"entropy":row[index("entropy")],"quality":row[index("quality")],
            "original_encoded":original_path,"original_sha256":original_sha,"original_bytes":wrapped.len(),
            "finalized_encoded":encoded,"finalized_sha256":encoded_sha,"finalized_bytes":new_wrapped.len(),
            "decoded_sha256":expected_sha,"source_sha256":source_sha,"symbols_identical":true,"pixels_identical":true}));
        writeln!(table, "{}", row.join("\t")).unwrap();
        table.flush().unwrap();
        if lineage.len().is_multiple_of(16) {
            println!("finalized {} verified carriers", lineage.len());
        }
    }
    let record = serde_json::json!({"original_table":input.join("quality.tsv"),"original_table_sha256":sha256(text.as_bytes()),
        "finalized_table_sha256":sha256(&std::fs::read(output.join("quality.tsv")).unwrap()),"cells":lineage.len(),"lineage":lineage,
        "scope":"Standard container finalization, per-file symbol equality and complete decoded-pixel equality. Original files and tables are preserved. Metric reuse requires matching pixel/source identities."});
    std::fs::write(
        output.join("FINALIZATION.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    std::fs::write(output.join("COMPLETE"), format!("{}\n", record["cells"])).unwrap();
}
