//! Persisted RGB8 rate-distortion scoring of the same cases used for timing.

use super::*;

#[path = "../../../benchmarks/perceptual.rs"]
mod perceptual;

pub(super) fn score(index: &Path, output: &Path) -> Result<()> {
    if output.exists() {
        return Err("score output already exists".into());
    }
    fs::create_dir_all(output)?;
    let cases: Vec<Case> = json(index)?;
    let mut cache = HashMap::<(PathBuf, PathBuf, bool), PathBuf>::new();
    let mut records = Vec::new();
    for case in cases {
        case.profile.validate()?;
        let key = (
            case.pixels.clone(),
            case.encoded.clone(),
            case.profile.idct_fma,
        );
        let stats = if let Some(path) = cache.get(&key) {
            path.clone()
        } else {
            let info = case.info()?;
            if info.sample != SampleType::U8
                || info.channels != Channels::Rgb
                || info.cicp
                    != ImageInfo::new(info.width, info.height, Channels::Rgb, SampleType::U8).cicp
            {
                return Err("score currently requires ordinary sRGB RGB8 cases".into());
            }
            let original = fs::read(&case.pixels)?;
            #[cfg(feature = "lab")]
            imij::lab::set_idct_fma(case.profile.idct_fma);
            let (_, decoded) = imij::decode(&fs::read(&case.encoded)?)?;
            let raw = output.join(format!("{}.raw", case.id));
            fs::write(&raw, &decoded)?;
            let (w, h) = (info.width as usize, info.height as usize);
            let mut cv = perceptual::cvvdp_fhd(info.width, info.height)?;
            let mut cv_map = Vec::new();
            let cv_jod = cv.score_with_diffmap(&original, &decoded, &mut cv_map)?;
            let cv_map_path = output.join(format!("{}.cvvdp-fhd.f32le", case.id));
            fs::write(
                &cv_map_path,
                cv_map
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>(),
            )?;
            let vmaf = perceptual::vmaf_still(&original, &decoded, w, h)?;
            let descriptor = fast_ssim2::PixelDescriptor::RGB8_SRGB;
            let source = fast_ssim2::PixelSlice::new(
                &original,
                info.width,
                info.height,
                info.row_bytes(),
                descriptor,
            )?;
            let distorted = fast_ssim2::PixelSlice::new(
                &decoded,
                info.width,
                info.height,
                info.row_bytes(),
                descriptor,
            )?;
            let ssim2 = fast_ssim2::Ssimulacra2Reference::new(&source)?.compare(&distorted)?;
            let to_rgb = |p: &[u8]| {
                p.as_chunks::<3>()
                    .0
                    .iter()
                    .map(|v| butteraugli::RGB8::new(v[0], v[1], v[2]))
                    .collect::<Vec<_>>()
            };
            let a = butteraugli::Img::new(to_rgb(&original), w, h);
            let b = butteraugli::Img::new(to_rgb(&decoded), w, h);
            let ba = butteraugli::butteraugli(
                a.as_ref(),
                b.as_ref(),
                &butteraugli::ButteraugliParams::default().with_compute_diffmap(true),
            )?;
            let map_path = output.join(format!("{}.butteraugli.f32le", case.id));
            let map = ba.diffmap.as_ref().unwrap();
            let bytes: Vec<u8> = map.buf().iter().flat_map(|v| v.to_le_bytes()).collect();
            fs::write(&map_path, bytes)?;
            let stats = output.join(format!("{}.json", case.id));
            save(
                &stats,
                &serde_json::json!({
                    "case_id": case.id, "encoded": case.encoded, "pixels": case.pixels,
                    "decoded": raw, "width": w, "height": h,
                    "ssimulacra2": ssim2, "butteraugli_max": ba.score,
                    "butteraugli_p1": ba.pnorm(1.0), "butteraugli_p2": ba.pnorm(2.0),
                    "butteraugli_p3": ba.pnorm_3, "butteraugli_p6": ba.pnorm(6.0),
                    "butteraugli_diffmap": map_path,
                    "cvvdp_fhd_jod": cv_jod, "cvvdp_fhd_diffmap": cv_map_path,
                    "vmaf_v1_fhd": vmaf,
                    "metric_settings": perceptual::settings(vmaf.is_some()),
                }),
            )?;
            cache.insert(key, stats.clone());
            eprintln!("scored {} {}", case.comparison, case.profile.name);
            stats
        };
        records.push(serde_json::json!({"case": case, "stats": stats}));
        save(&output.join("index.json"), &records)?;
    }
    Ok(())
}
