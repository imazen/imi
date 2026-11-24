//! Native DC component measurements, separate from full-image codec timings.
use super::*;

const MODES: [(&str, Mode); 5] = [
    ("native", Mode::Native),
    ("root", Mode::Root),
    ("inline", Mode::Inline),
    ("catalog", Mode::Catalog),
    ("references", Mode::References),
];

fn dc_decode(reader: &ReadPalette, bytes: &[u8], threads: usize) -> Vec<Vec<u8>> {
    let entries = reader
        .units
        .iter()
        .enumerate()
        .filter_map(|(i, u)| (u.0 == 0).then_some(i))
        .collect();
    crate::par::par_map(entries, threads, |_, i| reader.unit(bytes, i))
}

fn coverage(spans: &[(usize, usize)], total: usize) -> Option<Vec<usize>> {
    let mut next = 0;
    let mut owners = Vec::new();
    for (owner, &(first, count)) in spans.iter().enumerate() {
        if first != next || count == 0 {
            return None;
        }
        next = next.checked_add(count)?;
        if next > total {
            return None;
        }
        owners.resize(next, owner);
    }
    (next == total).then_some(owners)
}

/// A coverage/decoding witness, not a proposed production container.
/// Each span carries an ordinary numeric head and one ordinary numeric unit.
fn nonuniform(file: &[u8]) -> Value {
    let p = super::super::super::super::super::parse_lossy(file).unwrap();
    assert_eq!(p.ngroups(), 1);
    let (info, pixels, _) = super::super::super::super::super::dc_image_lab(file).unwrap();
    let count = p.stripes.len();
    let mut first = 0;
    let mut spans = Vec::new();
    let mut encoded = Vec::new();
    let mut recovered = Vec::new();
    let mut row_counts = Vec::new();
    while first < count {
        let n = [1, 3, 2][spans.len() % 3].min(count - first);
        let a = p.g.rows(first).0;
        let b = p.g.rows(first + n - 1).1;
        let mut local = info.clone();
        local.height = (b - a) as u32;
        let cfg = crate::EncodeConfig {
            effort: 0.0,
            stripe_rows: Some((b - a) as u32),
            column_groups: Some(0),
            float_shift: false,
            ..Default::default()
        };
        let (head, units, _) = crate::v1::encode_parts(
            &pixels[a * info.row_bytes()..b * info.row_bytes()],
            info.row_bytes(),
            &local,
            &cfg,
        )
        .unwrap();
        assert_eq!(units.len(), 1);
        let d = Definition {
            training: Training::Global,
            bank: 0,
            head,
            records: units,
        };
        let output = d.unit(&local, 0).unwrap();
        assert_eq!(output, pixels[a * info.row_bytes()..b * info.row_bytes()]);
        recovered.extend(output);
        put_varint(&mut encoded, n as u64);
        bytes(&mut encoded, &d.head);
        bytes(&mut encoded, &d.records[0]);
        row_counts.push(b - a);
        spans.push((first, n));
        first += n;
    }
    assert_eq!(recovered, pixels);
    let owners = coverage(&spans, count).unwrap();
    for (unit, &owner) in owners.iter().enumerate() {
        let (first, n) = spans[owner];
        assert!(unit >= first && unit < first + n);
    }
    json!({"spans":spans,"dc_rows":row_counts,"ac_owner":owners,
        "span_count_head_length_payload_length_and_payload_bytes":encoded.len(),
        "scope":"exact native DC samples and consecutive AC dependency closure; ordinary numeric heads/units; no native nonuniform reader integration or throughput claim"})
}

#[test]
fn coverage_rejects_gaps_overlaps_and_unbounded_spans() {
    assert_eq!(
        coverage(&[(0, 1), (1, 3), (4, 2)], 6),
        Some(vec![0, 1, 1, 1, 2, 2])
    );
    for spans in [
        vec![(0, 0)],
        vec![(1, 2)],
        vec![(0, 2), (1, 1)],
        vec![(0, 1), (2, 1)],
        vec![(0, 2)],
        vec![(0, usize::MAX), (usize::MAX, 1)],
    ] {
        assert_eq!(coverage(&spans, 3), None);
    }
}

pub(super) fn measure(path: &Path) {
    let m: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let root = Path::new(m["output"].as_str().unwrap());
    std::fs::create_dir(root).unwrap();
    let file = Arc::new(std::fs::read(m["native_file"].as_str().unwrap()).unwrap());
    assert_eq!(digest(&file), m["native_xxh3_128"].as_str().unwrap());
    let effort = m["effort"].as_f64().unwrap() as f32;
    let bank = m["bank"].as_u64().unwrap();
    let (_, _, study) = prepare(&file, effort, bank);
    let common = common(&file);
    let p = super::super::super::super::super::parse_lossy(&file).unwrap();
    let mut candidates = Vec::new();
    let mut stats = Vec::new();
    for (name, mode) in MODES {
        let candidate = select(&common, &p.stripes, &study, mode);
        let encoded = Arc::new(candidate.write());
        let reader = Arc::new(check_native(&file, &encoded));
        let expected = dc_decode(&reader, &encoded, 1);
        for t in [2, 4, 8] {
            assert_eq!(dc_decode(&reader, &encoded, t), expected);
        }
        let dc_unit_bytes: usize = candidate
            .units
            .iter()
            .filter(|u| u.kind == 0)
            .map(|u| u.bytes.len())
            .sum();
        stats.push(json!({"arm":name,"bytes":encoded.len(),"version":candidate.version,
            "root_and_catalog_definitions":candidate.dc.len(),"expanded_shared_definitions":reader.dc.len(),
            "inline_definitions":reader.inline_dc.iter().filter(|d|d.is_some()).count(),
            "dc_unit_bytes_including_selectors":dc_unit_bytes,"xxh3_128":digest(&encoded)}));
        std::fs::write(root.join(format!("{name}.private")), &**encoded).unwrap();
        candidates.push((encoded, reader));
    }
    let nonuniform = (p.ngroups() == 1).then(|| nonuniform(&file));
    std::fs::write(root.join("correctness.json"),serde_json::to_vec_pretty(&json!({
        "source_revision":m["source_revision"],"binary_xxh3_128":digest(&std::fs::read(std::env::current_exe().unwrap()).unwrap()),
        "native_file":m["native_file"],"native_bytes":file.len(),"native_xxh3_128":digest(&file),"arms":stats,
        "nonuniform":nonuniform,"effort":effort,"bank":bank,
        "scope":"private-carrier size deltas and native DC-only decoding, with parse/output allocation; identical native AC payloads. Offline trial generation includes DC extraction, native analysis, all model training and entropy coding; it is not full-image encode speed.",
        "selection":"complete-carrier-priced root enumeration and backward deletion, heuristic not global optimum",
        "common_head":"retains original native tables conservatively in every arm; duplicated ordinary DC root, not projected production size",
        "maps":false,"nas_writes":0})).unwrap()).unwrap();
    let result = zenbench::run_gated(
        zenbench::GateConfig::strict()
            .max_heavy_processes(4)
            .min_available_ram_mb(4096)
            .max_cpu_temp_c(Some(85.0))
            .max_wait(Duration::from_secs(30)),
        |suite| {
            suite.plan().workload_id = Some(format!("native-dc-model-{}", digest(&file)));
            suite.group("offline-trial-generation-1t", |g| {
                g.config()
                    .min_rounds(24)
                    .max_rounds(24)
                    .auto_rounds(false)
                    .warmup_time(Duration::from_millis(200))
                    .max_wall_time(Duration::from_secs(600));
                for name in ["native-trial-pool", "aa-trial-pool"] {
                    let f = file.clone();
                    g.bench(name, move |b| b.iter(|| prepare(&f, effort, bank)));
                }
            });
            for threads in [1, 2, 4, 8] {
                suite.group(format!("cold-dc-decode-{threads}t"), |g| {
                    g.config()
                        .min_rounds(24)
                        .max_rounds(24)
                        .auto_rounds(false)
                        .warmup_time(Duration::from_millis(200))
                        .max_wall_time(Duration::from_secs(600));
                    for (name, i) in [
                        ("native", 0),
                        ("root", 1),
                        ("inline", 2),
                        ("catalog", 3),
                        ("references", 4),
                        ("aa", 0),
                    ] {
                        let encoded = candidates[i].0.clone();
                        g.bench(name, move |b| {
                            b.iter(|| dc_decode(&ReadPalette::new(&encoded), &encoded, threads))
                        });
                    }
                });
            }
        },
    );
    result.save(root.join("timing.json")).unwrap();
    let v: Value =
        serde_json::from_slice(&std::fs::read(root.join("timing.json")).unwrap()).unwrap();
    assert_eq!(v["incomplete"], false);
    for g in v["comparisons"].as_array().unwrap() {
        assert_eq!(g["completed_rounds"], 24);
        assert!(
            g["samples"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["gate_clean"] == true)
        );
    }
    assert_eq!(v["unreliable"], false);
    std::fs::write(
        root.join("COMPLETE"),
        b"native DC correctness and timing gates pass\n",
    )
    .unwrap();
}
