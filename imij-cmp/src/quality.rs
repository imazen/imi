//! Warm references with the matrix's shared SDR perceptual conventions.
//! Source hashes for both matrix and shared helpers are recorded in provenance.
use crate::rd::{Result, save};
use std::{fs, path::Path};
#[path = "../../benchmarks/perceptual.rs"]
pub(crate) mod perceptual;

pub(crate) struct WarmScorer {
    width: u32,
    height: u32,
    original: Vec<u8>,
    source_hash: String,
    cv: cvvdp::Cvvdp,
    ssim: fast_ssim2::Ssimulacra2Reference,
    ba: butteraugli::ButteraugliReference,
    ba_maps: Option<butteraugli::ButteraugliReference>,
}
impl WarmScorer {
    pub(crate) fn new(source_path: &Path, width: u32, height: u32) -> Result<Self> {
        let original = fs::read(source_path)?;
        crate::rd::check_rgb(&original, width, height)?;
        let mut cv = perceptual::cvvdp_fhd(width, height)?;
        cv.warm_reference(&original)?;
        let source = fast_ssim2::PixelSlice::new(
            &original,
            width,
            height,
            width as usize * 3,
            fast_ssim2::PixelDescriptor::RGB8_SRGB,
        )?;
        let ssim = fast_ssim2::Ssimulacra2Reference::new(&source)?;
        let ba = butteraugli::ButteraugliReference::new(
            &original,
            width as usize,
            height as usize,
            butteraugli::ButteraugliParams::default(),
        )?;
        let source_hash = crate::rd::hash(&original);
        Ok(Self {
            width,
            height,
            original,
            source_hash,
            cv,
            ssim,
            ba,
            ba_maps: None,
        })
    }
    pub(crate) fn source_sha256(&self) -> &str {
        &self.source_hash
    }
    pub(crate) fn score(
        &mut self,
        source_path: &Path,
        decoded_path: &Path,
        output: &Path,
        maps: bool,
    ) -> Result<()> {
        let (width, height) = (self.width, self.height);
        if output.exists() {
            return Err("score output already exists".into());
        }
        let original = &self.original;
        let decoded = fs::read(decoded_path)?;
        crate::rd::check_rgb(original, width, height)?;
        crate::rd::check_rgb(&decoded, width, height)?;
        fs::create_dir_all(output)?;
        let (w, h) = (width as usize, height as usize);
        let mut cv_map = Vec::new();
        let cv_jod = if maps {
            self.cv.score_with_warm_ref_diffmap(&decoded, &mut cv_map)?
        } else {
            self.cv.score_with_warm_ref(&decoded)?
        };
        let cv_map_path = output.join(format!("{}.cvvdp-fhd.f32le", "pair"));
        if maps {
            fs::write(
                &cv_map_path,
                cv_map
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>(),
            )?;
        }
        let vmaf = perceptual::vmaf_still(original, &decoded, w, h)?;
        let descriptor = fast_ssim2::PixelDescriptor::RGB8_SRGB;
        let distorted = fast_ssim2::PixelSlice::new(&decoded, width, height, w * 3, descriptor)?;
        let ssim2 = self.ssim.compare(&distorted)?;
        let ba = if maps {
            if self.ba_maps.is_none() {
                self.ba_maps = Some(butteraugli::ButteraugliReference::new(
                    original,
                    w,
                    h,
                    butteraugli::ButteraugliParams::default().with_compute_diffmap(true),
                )?);
            }
            self.ba_maps.as_ref().unwrap().compare(&decoded)?
        } else {
            self.ba.compare(&decoded)?
        };
        // zensim 0.2.7, PreviewV0_2 profile: the convention `cmp::metrics::zensim` uses.
        let (zensim, zensim_error) = match zensim::Zensim::new(zensim::ZensimProfile::PreviewV0_2)
            .compute(
                &zensim::RgbSlice::new(original.as_chunks::<3>().0, w, h),
                &zensim::RgbSlice::new(decoded.as_chunks::<3>().0, w, h),
            ) {
            Ok(r) => (Some(r.score()), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let map_path = output.join(format!("{}.butteraugli.f32le", "pair"));
        if maps {
            let map = ba
                .diffmap
                .as_ref()
                .ok_or("requested Butteraugli map missing")?;
            let bytes: Vec<u8> = map.buf().iter().flat_map(|v| v.to_le_bytes()).collect();
            fs::write(&map_path, bytes)?;
        }
        let stats = output.join("scores.json");
        // These reductions require a retained diagnostic map. Older library
        // versions return a fallback without it; do not label that fallback
        // as an evaluated p1/p2/p6 score in routine scalar-only output.
        let pnorms = if maps {
            [1.0, 2.0, 6.0].map(|p| serde_json::json!(ba.pnorm(p)))
        } else {
            [
                serde_json::Value::Null,
                serde_json::Value::Null,
                serde_json::Value::Null,
            ]
        };
        save(
            &stats,
            &serde_json::json!({
                "case_id": "pair", "encoded": serde_json::Value::Null, "pixels": source_path,
                "decoded": decoded_path, "source_sha256": self.source_hash, "decoded_sha256": crate::rd::hash(&decoded), "width": w, "height": h,
                "ssimulacra2": ssim2, "butteraugli_max": ba.score,
                "butteraugli_p1": pnorms[0], "butteraugli_p2": pnorms[1],
                "butteraugli_p3": ba.pnorm_3, "butteraugli_p6": pnorms[2],
                "butteraugli_diffmap": maps.then_some(map_path),
                "cvvdp_fhd_jod": cv_jod, "cvvdp_fhd_diffmap": maps.then_some(cv_map_path),
                "vmaf_v1_fhd": vmaf,
                "zensim_v0_2": zensim, "zensim_v0_2_error": zensim_error,
                "metric_settings": perceptual::settings(vmaf.is_some()),
                "diagnostic_maps_requested": maps,
                "diagnostic_pnorms_available": maps,
            }),
        )?;
        Ok(())
    }
}
pub(crate) fn score(
    source: &Path,
    decoded: &Path,
    width: u32,
    height: u32,
    output: &Path,
    maps: bool,
) -> Result<()> {
    WarmScorer::new(source, width, height)?.score(source, decoded, output, maps)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn score_only_matches_diagnostics_without_creating_maps() -> Result<()> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or("home directory required for disk-backed test outputs")?;
        let root = std::path::PathBuf::from(home).join("tmp").join(format!(
            "imij-cmp-score-only-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir_all(&root)?;
        let (w, h) = (256u32, 256u32);
        let original: Vec<u8> = (0..w as usize * h as usize * 3)
            .map(|i| ((i * 13 + i / 17) % 256) as u8)
            .collect();
        let decoded: Vec<u8> = original
            .iter()
            .enumerate()
            .map(|(i, &v)| v.saturating_sub((i % 7) as u8))
            .collect();
        let source = root.join("source.rgb");
        let dist = root.join("dist.rgb");
        fs::write(&source, &original)?;
        fs::write(&dist, &decoded)?;
        let mut scorer = WarmScorer::new(&source, w, h)?;
        scorer.score(&source, &dist, &root.join("plain"), false)?;
        assert!(
            scorer.ba_maps.is_none(),
            "no diagnostic reference constructed by default"
        );
        scorer.score(&source, &dist, &root.join("maps"), true)?;
        scorer.score(&source, &dist, &root.join("plain-again"), false)?;
        let read = |name: &str| -> Result<serde_json::Value> {
            Ok(serde_json::from_slice(&fs::read(
                root.join(name).join("scores.json"),
            )?)?)
        };
        let plain = read("plain")?;
        let maps = read("maps")?;
        let again = read("plain-again")?;
        for field in [
            "ssimulacra2",
            "cvvdp_fhd_jod",
            "vmaf_v1_fhd",
            "butteraugli_max",
            "butteraugli_p3",
            "zensim_v0_2",
        ] {
            assert_eq!(plain[field], maps[field], "{field}");
            assert_eq!(plain[field], again[field], "{field}");
        }
        assert!(plain["vmaf_v1_fhd"].is_number());
        assert_eq!(fs::read_dir(root.join("plain"))?.count(), 1);
        assert_eq!(fs::read_dir(root.join("plain-again"))?.count(), 1);
        assert_eq!(fs::read_dir(root.join("maps"))?.count(), 3);
        for field in [
            "butteraugli_diffmap",
            "cvvdp_fhd_diffmap",
            "butteraugli_p1",
            "butteraugli_p2",
            "butteraugli_p6",
        ] {
            assert!(plain[field].is_null(), "{field}");
            assert!(!maps[field].is_null(), "{field}");
        }
        Ok(())
    }
    #[test]
    fn warm_metrics_match_matrix_one_shot() -> Result<()> {
        let (w, h) = (64, 64);
        let original: Vec<u8> = (0..w * h * 3)
            .map(|i| ((i * 13 + i / 17) % 256) as u8)
            .collect();
        let decoded: Vec<u8> = original
            .iter()
            .enumerate()
            .map(|(i, &p)| p.saturating_sub((i % 7) as u8))
            .collect();
        let cv_new = || {
            cvvdp::Cvvdp::with_geometry(
                w as u32,
                h as u32,
                cvvdp::CvvdpParams {
                    display: cvvdp::DisplayModel::STANDARD_FHD,
                    ..Default::default()
                },
                cvvdp::DisplayGeometry::STANDARD_FHD,
            )
        };
        let mut cold = cv_new()?;
        let mut map = Vec::new();
        let expected = cold.score_with_diffmap(&original, &decoded, &mut map)?;
        let mut warm = cv_new()?;
        warm.warm_reference(&original)?;
        let mut warmmap = Vec::new();
        let actual = warm.score_with_warm_ref_diffmap(&decoded, &mut warmmap)?;
        assert_eq!(expected, actual);
        assert_eq!(map, warmmap);
        let to_rgb = |p: &[u8]| {
            p.as_chunks::<3>()
                .0
                .iter()
                .map(|c| butteraugli::RGB8::new(c[0], c[1], c[2]))
                .collect::<Vec<_>>()
        };
        let a = butteraugli::Img::new(to_rgb(&original), w, h);
        let b = butteraugli::Img::new(to_rgb(&decoded), w, h);
        let params = butteraugli::ButteraugliParams::default().with_compute_diffmap(true);
        let expected = butteraugli::butteraugli(a.as_ref(), b.as_ref(), &params)?;
        let reference = butteraugli::ButteraugliReference::new(&original, w, h, params)?;
        let actual = reference.compare(&decoded)?;
        assert_eq!(expected.score, actual.score);
        assert_eq!(expected.pnorm_3, actual.pnorm_3);
        assert_eq!(
            expected.diffmap.unwrap().buf(),
            actual.diffmap.unwrap().buf()
        );
        assert!(perceptual::vmaf_still(&original, &decoded, 33, 64)?.is_none());
        assert!(perceptual::vmaf_still(&original, &decoded, 63, 64)?.is_none());
        // Passes the size gate, but VMAF's speed-chroma operating grid is empty.
        let tall: Vec<u8> = (0..144 * 256 * 3).map(|i| (i % 251) as u8).collect();
        assert!(perceptual::vmaf_still(&tall, &tall, 144, 256)?.is_none());
        Ok(())
    }
}
