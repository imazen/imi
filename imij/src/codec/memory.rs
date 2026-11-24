//! Capacity admission for the adaptive still lossless encoder.
//!
//! This deliberately sums overlapping allocation families, rather than trying
//! to predict which data-dependent palette/model/bilevel candidate will win.
//! See docs/ENCODER_MEMORY.md for scope and the allocation audit. It is not an
//! RSS estimate: caller storage, shared pools, stacks and allocator metadata
//! are outside the owned-capacity contract.

use crate::{EncodeConfig, Error, ImageInfo};

pub(super) fn supported(cfg: &EncodeConfig) -> Result<(), Error> {
    #[cfg(feature = "lab")]
    if cfg.guide.is_some() || cfg.seed_bank != 0 {
        return Err(Error::Unsupported(
            "working limit with lab guide or seed bank",
        ));
    }
    crate::effort::Effort::new(cfg.effort)?;
    Ok(())
}

// Every term is evaluated in u128, before narrowing to the host allocation
// domain. Inputs are u32 geometry, <= 8 coded slots and host-sized metadata;
// products below cannot overflow u128 even on a 64-bit host.
pub(super) fn lossless_bytes(
    info: &ImageInfo,
    requested: &EncodeConfig,
    metadata_bytes: u128,
) -> Result<u64, Error> {
    supported(requested)?;
    if info.width == 0 || info.height == 0 {
        return Err(Error::InvalidInput("empty image"));
    }
    if metadata_bytes > isize::MAX as u128 {
        return Err(Error::TooLarge);
    }
    // Decoder-target geometry can differ from the requested geometry. Use the
    // very same planner as the native writer, without changing encoder policy.
    let planned;
    let cfg = if requested.decoder_memory_limit.is_some() {
        planned = crate::decoder_memory::plan_lossless(info, requested)?.0;
        &planned
    } else {
        requested
    };
    let w = u128::from(info.width);
    let h = u128::from(info.height);
    let slots = (info.channels.count() * info.sample.words()) as u128;
    let rows = u128::from(
        cfg.stripe_rows
            .unwrap_or_else(|| crate::v1::default_stripe_rows(info.width))
            .max(1)
            .min(info.height),
    );
    let lanes = u128::from(
        cfg.lanes
            .unwrap_or(crate::encode::default_lanes(info.width))
            .clamp(1, 16),
    );
    let group = crate::v1::group_cols(cfg, info.width as usize) as u128;
    let group = if group == 0 { w } else { group };
    let stripes = h.div_ceil(rows);
    let groups = w.div_ceil(group);
    let units = stripes * groups;
    let workers = (cfg.threads.max(1) as u128).min(units);
    let analysis_workers = (cfg.threads.max(1) as u128).min(stripes);
    let pixels = w * h;
    let unit_pixels = group * rows;
    // step_major always allocates 16 lanes, including when fewer are active.
    let padded_row = (w / group) * 16 * group.div_ceil(lanes) + 16 * (w % group).div_ceil(lanes);

    // Full mapped + residual words, retained padded unit words (plus palette),
    // and the full-image binary candidate. Charge all candidates together.
    let planes = pixels * (slots * 8 + 4) + padded_row * h * (slots + 1) * 4;
    // Pass-one histograms persist for every unit until model construction.
    // 18 predictor classes, <=256 tokens, u64 counts; bilevel has 1024 pairs.
    let histograms = units * ((slots + 1) * 18 * 256 * 8 + 2048 * 8);
    // Unit input/results/mutexes, per-slot nested Vecs and row/stripe metadata.
    // A 1 KiB slot allowance covers its 18 Vec headers + outer descriptors;
    // 4 KiB per unit covers the remaining pass1/pass2 and scheduler records.
    let descriptors = units * 4096 + (units + stripes) * slots * 1024 + h * groups * 16;

    // Full-width analysis: d plus eight neighbor rows per slot, ro/bo;
    // four decorrelation candidates * seven predictors and their Vec headers.
    // Near-lossless also keeps reconstructed and residual stripe words.
    let analysis = analysis_workers
        * ((9 * slots + 2) * w * 4 + rows * (4 * 7 * 8 + 4 * 24) + 2 * slots * w * rows * 4);
    // Group analysis can coexist with cropped planes, residuals, near-lossless
    // reconstruction and palette trial indices/residuals. Its row scratch is
    // charged separately from the original full-width analysis.
    let unit_analysis = workers
        * ((4 * slots + 2) * unit_pixels * 4
            + (9 * slots + 2) * group * 4
            + rows * (4 * 7 * 8 + 4 * 24));
    // One slot at a time: Sym (12 B), up to three renormalization words
    // (8 B each), and the fallback sorted merge (16 B each). Include all
    // three even though the ordered path does not allocate the merge.
    let entropy = workers * unit_pixels * (12 + 3 * 8 + 3 * 16);
    // Precision-map selection samples at most 128x8 pixels, one window at a
    // time: cropped + residual planes and the same row/predictor scratch.
    let sample_w = w.min(128);
    let sample_h = h.min(8);
    let precision = 2 * slots * sample_w * sample_h * 4
        + (9 * slots + 2) * sample_w * 4
        + sample_h * (4 * 7 * 8 + 4 * 24);
    // Models/merged class histograms/table candidates and finalizer decode
    // tables: <=128 KiB per slot or palette model. Mapping-domain scratch,
    // palette hash tables and bilevel tables share
    // the additional 1 MiB allowance; replicated palette scratch is per worker.
    let tables = (slots + 1) * 128 * 1024 + 1024 * 1024 + workers * 32 * 1024;

    // At most three u16 words per coded sample, plus 16 u32 initial states
    // per slot/unit. Unit reserve widths are u8 even in lab configurations;
    // 1024 B/unit covers framing/indexes and up to 255 B reserved offsets.
    // Palette entries (256 * up to 2 B/slot), row predictors, model wire,
    // metadata and a bilevel trial are charged in addition to normal streams.
    let file = pixels * (6 * slots + 2)
        + units * (64 * (slots + 1) + 512 * slots + 1024)
        + h * groups
        + (slots + 1) * 4 * 256 * 8
        + metadata_bytes
        + 4096;
    // Template palette trials (palette configurations): the image's and every
    // unit's u8 index planes, retained stream candidates (at most seven
    // binary or index symbols per pixel, two bytes each, plus the stripe's
    // entries, row flags and states) and their copy into the unit bytes, per
    // unit context histograms, and per worker a padded index plane, packed
    // symbols and renormalization words (eight bytes each). The tables fit
    // the fixed scratch allowance.
    let template = if cfg.palette {
        pixels * (2 + 2 * 14)
            + units * (20 * 1024 + 2048 + 64)
            + h * groups
            + workers * (unit_pixels * (7 * 8 * 2 + 2) + 8 * group + 64)
    } else {
        0
    };
    // Include the smallest legal checksum piece, also selectable by lab.
    let file = file
        + file.div_ceil(1 << crate::checksum::MIN_PIECE_LOG2)
            * crate::Digest::STORAGE_WIDTH as u128
        + 4096;
    // Retained stripe bytes, DATA assembly, output and a finalizer copy may
    // coexist. Fourfold capacity allowance covers Vec geometric growth and
    // old+new allocations during a reallocation, including tiny Vec minima.
    // Metadata is also retained by the job and copied for native emission.
    let bytes = (planes
        + histograms
        + descriptors
        + analysis
        + unit_analysis
        + entropy
        + precision
        + tables
        + template
        + file * 4
        + metadata_bytes * 2)
        * 4;
    if bytes > isize::MAX as u128 {
        return Err(Error::TooLarge);
    }
    u64::try_from(bytes).map_err(|_| Error::TooLarge)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, SampleType};

    #[cfg(feature = "rayon")]
    #[test]
    fn working_plan_uses_the_callers_actual_pool() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zencodec::{ResourceLimits, ThreadingPolicy};
        use zenpixels::{PixelDescriptor, PixelSlice};
        let info = ImageInfo::new(33, 17, Channels::Rgb, SampleType::U8);
        let public = crate::ImiEncoderConfig::default().tune_lossless(|t| t.stripe_rows = Some(1));
        let cfg = public.lossless_config();
        let data: alloc::vec::Vec<u8> = (0..33 * 17 * 3).map(|i| (i * 31) as u8).collect();
        let pixels = PixelSlice::new(&data, 33, 17, 99, PixelDescriptor::RGB8_SRGB).unwrap();
        let mut reference = None;
        for threads in [1, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let mut actual = cfg.clone();
            actual.threads = threads;
            let bytes = lossless_bytes(&info, &actual, 0).unwrap();
            let output = pool.install(|| {
                let run = |cap| {
                    public
                        .clone()
                        .job()
                        .with_limits(
                            ResourceLimits::none()
                                .with_threading(ThreadingPolicy::Parallel)
                                .with_max_memory(cap),
                        )
                        .encoder()
                        .unwrap()
                        .encode(pixels.clone())
                };
                assert_eq!(run(bytes - 1).unwrap_err(), Error::TooLarge);
                run(bytes).unwrap()
            });
            assert_eq!(crate::decode(output.data()).unwrap().1, data);
            if let Some(previous) = &reference {
                assert_eq!(output.data(), previous);
            }
            reference = Some(output.data().to_vec());
        }
    }

    #[test]
    #[allow(deprecated)] // Preserve this legacy metadata setter's memory contract.
    fn working_plan_exact_limit_and_owned_metadata() {
        use zencodec::encode::{EncodeJob, Encoder, EncoderConfig};
        use zencodec::{ResourceLimits, ThreadingPolicy};
        use zenpixels::{PixelDescriptor, PixelSlice};
        let info = ImageInfo::new(17, 5, Channels::Rgb, SampleType::U8);
        let public = crate::ImiEncoderConfig::default();
        let cfg = public.lossless_config();
        let bytes = lossless_bytes(&info, &cfg, 0).unwrap();
        let data = [47; 17 * 5 * 3];
        let pixels = PixelSlice::new(&data, 17, 5, 51, PixelDescriptor::RGB8_SRGB).unwrap();
        let limits = ResourceLimits::none().with_threading(ThreadingPolicy::Sequential);
        let run = |cap, metadata| {
            public
                .clone()
                .job()
                .with_limits(limits.with_max_memory(cap))
                .with_metadata_policy(metadata, zencodec::MetadataPolicy::PreserveExact)
                .encoder()
                .unwrap()
                .encode(pixels.clone())
        };
        assert_eq!(
            run(bytes - 1, zencodec::Metadata::none()).unwrap_err(),
            Error::TooLarge
        );
        let admitted = run(bytes, zencodec::Metadata::none()).unwrap();
        assert_eq!(crate::decode(admitted.data()).unwrap().1, data);
        let meta = zencodec::Metadata::none().with_xmp(alloc::vec![3; 65536]);
        assert_eq!(run(bytes, meta.clone()).unwrap_err(), Error::TooLarge);
        let with_metadata = lossless_bytes(&info, &cfg, 65536).unwrap();
        let output = run(with_metadata, meta).unwrap();
        assert_eq!(
            crate::probe(output.data()).unwrap().1.xmp.as_deref(),
            Some(&[3; 65536][..])
        );
    }

    #[test]
    fn working_plan_geometry_and_representability() {
        let info = ImageInfo::new(129, 65, Channels::Rgba, SampleType::F32);
        let cfg = EncodeConfig::default();
        let ordinary = lossless_bytes(&info, &cfg, 0).unwrap();
        let mut more = cfg.clone();
        more.lanes = Some(1);
        assert!(lossless_bytes(&info, &more, 0).unwrap() > ordinary);
        more.stripe_rows = Some(1);
        more.column_groups = Some(1);
        // Retaining a histogram for each one-pixel unit exceeds the 32-bit
        // capacity domain even though this input raster is small.
        #[cfg(target_pointer_width = "32")]
        assert_eq!(lossless_bytes(&info, &more, 0), Err(Error::TooLarge));
        #[cfg(target_pointer_width = "64")]
        assert!(lossless_bytes(&info, &more, 0).unwrap() > ordinary);
        more = cfg.clone();
        more.threads = 8;
        assert!(lossless_bytes(&info, &more, 0).unwrap() > ordinary);
        assert!(lossless_bytes(&info, &cfg, 65_536).unwrap() > ordinary);
        let giant = ImageInfo::new(u32::MAX, u32::MAX, Channels::Rgba, SampleType::F32);
        assert_eq!(lossless_bytes(&giant, &more, 0), Err(Error::TooLarge));
        assert_eq!(
            lossless_bytes(&info, &cfg, usize::MAX as u128),
            Err(Error::TooLarge)
        );
        assert_eq!(lossless_bytes(&info, &cfg, u128::MAX), Err(Error::TooLarge));
    }
}
