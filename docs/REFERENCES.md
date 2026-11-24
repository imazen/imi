# References

Every external work this repository names: standards, papers, metrics, codecs,
tools and data sets. IMI is a research prototype that borrows ideas and
compares against other codecs, so this file exists to credit that work
properly and to make every comparison checkable.

Entries give authors, title, venue or publisher, year and a DOI or stable URL.
URLs were checked against the source on 2026-10-10; an entry that could not be
checked is marked. Where the repository uses a technique, the entry says where.

**Keeping this current:** when a doc, comment or study names a work that is not
listed here, add its entry in the same change. Cite the original paper or
standard for an idea, and the implementation for measured comparisons.

## Standards and format specifications

- **ITU-T T.81 / ISO/IEC 10918-1** — *Information technology — Digital
  compression and coding of continuous-tone still images: Requirements and
  guidelines* (JPEG), ITU, 1992. <https://www.w3.org/Graphics/JPEG/itu-t81.pdf>.
  Used for: the baseline quantizer scale (`LossyConfig::quality`), the Annex K
  base quantization matrix (`docs/LOSSY.md`), and JPEG as the decode-speed
  comparison point.
- **ISO/IEC 18181 (JPEG XL)** — *Information technology — JPEG XL image coding
  system*, ISO/IEC, 2021–2022. Overview: <https://jpeg.org/jpegxl/>. Used for:
  the XYB color space, the comparator of record in the evaluation standard,
  coding tools studied in `docs/LOW_BITRATE_RESEARCH_2026-10-03.md`, and the
  modular mode's self-correcting ("weighted") predictor and its max-error
  property, whose contexts the lossless study `benchmarks/wpctx_2026-10-11/`
  priced (part 1, modular mode; the fixed DC tree cutoffs are libjxl's,
  below).
- **ITU-T H.273** — *Coding-independent code points for video signal type
  identification* (CICP), ITU, in force 07/24.
  <https://www.itu.int/rec/T-REC-H.273>. Used for: the `Cicp` header record and
  transfer/primaries/matrix signaling throughout the format.
- **ITU-R BT.709** — *Parameter values for the HDTV standards for production
  and international programme exchange*, ITU, 2015.
  <https://www.itu.int/rec/R-REC-BT.709>. Used for: sRGB/BT.709 primaries and
  the YUV conversion in the VMAF harness.
- **ITU-R BT.2100** — *Image parameter values for high dynamic range television
  for use in production and international programme exchange* (PQ and HLG),
  ITU, 2016 (with later editions).
  <https://www.itu.int/rec/R-REC-BT.2100>. Used for: PQ and HLG transfer
  functions in `imij/src/lossy/transfer.rs` and the HDR tests.
- **SMPTE ST 2084** — *High Dynamic Range Electro-Optical Transfer Function of
  Mastering Reference Displays* (PQ), SMPTE, 2014.
  <https://ieeexplore.ieee.org/document/7291452> (SMPTE store; IEEE Xplore
  mirror). Used for: the PQ constants and formula behind BT.2100 PQ coding.
- **IEC 61966-2-1** — *Multimedia systems and equipment — Colour measurement
  and management — Part 2-1: Default RGB colour space — sRGB*, IEC, 1999
  (amended 2003). <https://webstore.iec.ch/publication/6169>. Used for: the
  normative sRGB transfer function and the sRGB8 reconstruction table.
- **W3C PNG (Third Edition)** — *Portable Network Graphics (PNG) Specification
  (Third Edition)*, W3C Recommendation, 24 June 2025.
  <https://www.w3.org/TR/png-3/>. Used for: the container's letter-case chunk
  rules (SPEC 1), the `cICP` precedent, and PNG as the lossless comparison
  point.
- **AV1 Image File Format (AVIF)** — *AV1 Image File Format (AVIF) v1.2.0*, AOM
  Final Deliverable, 16 October 2025.
  <https://aomediacodec.github.io/av1-avif/>. Used for: AVIF as the comparator
  in `imij-cmp` and the evaluation standard.
- **AV1 Bitstream & Decoding Process Specification** — Alliance for Open
  Media, v1.0.0 with errata. <https://aomediacodec.github.io/av1-spec/>. Used
  for: the AVIF encode/decode comparisons (via `zenav1-svt`, `rav1d-safe`).
- **DICOM PS3** — *Digital Imaging and Communications in Medicine*, NEMA,
  current edition. <https://www.dicomstandard.org/current>. Used for: the
  DICOM mapping in `docs/SCIENCE_METADATA.md`.
- **FITS Standard 4.0** — *Definition of the Flexible Image Transport System
  (FITS)*, IAU FITS Working Group, 2018.
  <https://fits.gsfc.nasa.gov/fits_standard.html>. Used for: FITS mappings in
  `docs/SCIENCE_METADATA.md` and the science baselines.
- **OME-NGFF (Zarr)** — *OME-NGFF: next-generation file format specification*,
  OME community. <https://ngff.openmicroscopy.org/>. Used for: the OME-Zarr
  mapping in `docs/SCIENCE_METADATA.md`.
- **CF Conventions** — *CF Conventions and Metadata* (CF-1.11), Unidata / CF
  community. <https://cfconventions.org/>. Used for: the CF mapping in
  `docs/SCIENCE_METADATA.md` and the ERA5 baselines.
- **OGC GeoTIFF Standard 1.1** — Open Geospatial Consortium, 2019.
  <https://www.ogc.org/standards/geotiff>. Used for: the GeoTIFF mapping in
  `docs/SCIENCE_METADATA.md` and the DEM baselines.
- **HDF5 File Format Specification** — The HDF Group, version 3.0.
  <https://www.hdfgroup.org/solutions/hdf5/>. Used for: HDF5 comparisons in
  `docs/TIFF_HDF5_REVIEW_2026-10-08.md`.
- **ICC.1:2022** — *Image technology colour management — Architecture, profile
  format, and data structure*, International Color Consortium, 2022.
  <https://www.color.org/specification/ICC.1-2022-05.pdf>. Used for: ICC
  profile passthrough and metadata compression.
- **CIPA DC-008 (Exif 2.32)** — *Exchangeable image file format for digital
  still cameras: Exif Version 2.32*, CIPA, 2019.
  <https://www.cipa.jp/std/documents/e/DC-X008-Translation-2019-E.pdf>. Used
  for: EXIF passthrough in `imij/src/metadata.rs`.
- **ISO 16684-1 (XMP)** — *Graphic technology — Extensible metadata platform
  (XMP) specification — Part 1: Data model, serialization and core
  properties*, ISO, 2019. <https://www.iso.org/standard/75163.html> (ISO
  blocks automated checks; standard number from the ISO catalogue). Used for:
  XMP passthrough and metadata compression.
- **RFC 8878** — Y. Collet, M. Kucherawy, *Zstandard Compression and the
  'application/zstd' Media Type*, IETF, 2021.
  <https://www.rfc-editor.org/rfc/rfc8878>. Used for: `.zst` result files and
  entropy comparisons in the benchmark tooling.
- **LZ4 block format** — Y. Collet, *LZ4 Block Format* (specification document,
  lz4 repository). <https://github.com/lz4/lz4/blob/dev/doc/lz4_Block_format.md>.
  Used for: the grammar of `imij/src/metadata_pack.rs` (an independent safe
  Rust implementation).
- **XXH3 (xxHash)** — Y. Collet, *xxHash — Extremely fast non-cryptographic
  hash algorithm*, specification in the repository.
  <https://github.com/Cyan4973/xxHash>. Used for: the optional `cSUM` file
  checksum (through zenhash) and its independent test oracle.
- **ITU-T T.88 / ISO/IEC 14492 (JBIG2)** — *Information technology — Lossy/lossless
  coding of bi-level images*, ITU/ISO, 2000 (with later editions).
  <https://www.itu.int/rec/T-REC-T.88>. Used for: the bilevel page comparison
  in `benchmarks/pages_2026-10-08/BILEVEL.md`.
- **ITU-T T.82 / ISO/IEC 11544 (JBIG)** — *Information technology — Coded
  representation of picture and audio information — Progressive bi-level image
  compression*, ITU/ISO, 1993. <https://www.itu.int/rec/T-REC-T.82>. Used for:
  the ten-pixel three-line template of bilevel stripes (SPEC 2.10,
  `imij/src/bilevel.rs`), coded here with static per-image probabilities and
  the lossless coder's interleaved rANS instead of JBIG's adaptive arithmetic
  coder.
- **ITU-T T.6 (CCITT Group 4)** — *Facsimile coding schemes and coding control
  functions for Group 4 facsimile apparatus*, ITU, 1988.
  <https://www.itu.int/rec/T-REC-T.6>. Used for: the G4 baseline in the
  bilevel page comparison.
- **ITU-T T.87 / ISO/IEC 14495-1 (JPEG-LS)** — *Information technology —
  Lossless and near-lossless compression of continuous-tone still images:
  Baseline*, ITU/ISO, 1998. <https://www.itu.int/rec/T-REC-T.87>. Used for:
  the NEAR near-lossless mode that Scientific 0.1's error-bounded coding
  follows (SPEC 2.6).

## Papers and technical articles

- **Arai, Y., Agui, T., Nakajima, M.** — *A Fast DCT-SQ Scheme for Images*,
  IEICE Transactions on Fundamentals E71(11), 1988, pp. 1095–1097. Author
  listing: <https://globals.ieice.org/en_transactions/Author/a_name=Takeshi%20AGUI>
  (no DOI; IEICE catalogue). Used for: the scaled float inverse DCT
  (`imij/src/lossy/dct.rs`, "as in libjpeg's `jidctflt`").
- **Catmull, E., Rom, R.** — *A Class of Local Interpolating Splines*, in
  Computer Aided Geometric Design (R. E. Barnhill, R. F. Riesenfeld, eds.),
  Academic Press, 1974, pp. 317–326. DOI
  [10.1016/B978-0-12-079050-0.50020-5](https://doi.org/10.1016/B978-0-12-079050-0.50020-5).
  Used for: the Catmull-Rom interpolation in the deringing encoder
  (`imij/src/lossy/dering.rs`).
- **Duda, J.** — *Asymmetric numeral systems: entropy coding combining speed of
  Huffman coding with compression rate of arithmetic coding*, 2013,
  arXiv:1311.2540. <https://arxiv.org/abs/1311.2540>. Used for: the
  interleaved rANS entropy layer (`imij/src/rans.rs`, SPEC 3).
- **Duda, J.** — *Encoding of probability distributions for Asymmetric Numeral
  Systems*, 2021, arXiv:2106.06438.
  <https://arxiv.org/abs/2106.06438>. Studied for: seed-table and frequency
  representation questions (`docs/LOW_BITRATE_RESEARCH_2026-10-03.md`).
- **Sneyers, J., Alakuijala, J., Versari, L., Szabadka, Z., Boukortt, S.,
  Cohen-Tidhar, A., Firsching, M., Kliuchnikov, E., Lev-Ami, T., Portis, E.,
  Richter, T., Watanabe, O.** — *The JPEG XL Image Coding System: History,
  Features, Coding Tools, Design Rationale, and Future*, 2025,
  arXiv:2506.05987. <https://arxiv.org/abs/2506.05987>. Used for: design
  rationale comparisons (XYB, adaptive quantization, filters) throughout
  `docs/LOSSY.md` and the low-bitrate research note.
- **Rhatushnyak, A., Wassenberg, J., Sneyers, J., et al.** — *Committee Draft
  of JPEG XL Image Coding System*, 2019, arXiv:1908.03565.
  <https://arxiv.org/abs/1908.03565>. Used for: historical LF-prediction
  comparison in `docs/LOW_BITRATE_RESEARCH_2026-10-03.md`.
- **Alakuijala, J., Obryk, R., Stoliarchuk, O., Szabadka, Z., Vandevenne, L.,
  Wassenberg, J.** — *Guetzli: Perceptually Guided JPEG Encoder*, 2017,
  arXiv:1703.04421. <https://arxiv.org/abs/1703.04421>. Studied for: RDOQ and
  trellis comparisons (`docs/EVAL_RDOQ_2026-10-04.md`).
- **Midtskogen, S., Valin, J.-M.** — *The AV1 Constrained Directional
  Enhancement Filter (CDEF)*, ICASSP 2018 (preprint 2016), arXiv:1602.05975.
  <https://arxiv.org/abs/1602.05975>. Studied for: the postfilter experiment
  (`docs/EVAL_POSTFILTER_2026-10-04.md`).
- **Wang, Z., Bovik, A. C., Sheikh, H. R., Simoncelli, E. P.** — *Image quality
  assessment: from error visibility to structural similarity*, IEEE
  Transactions on Image Processing 13(4), 2004, pp. 600–612. DOI
  [10.1109/TIP.2003.819861](https://doi.org/10.1109/TIP.2003.819861). Used
  for: the structural-similarity basis of SSIMULACRA2.
- **Sneyers, J.** — *SSIMULACRA 2 — Structural SIMilarity Unveiling Local And
  Compression Related Artifacts*, Cloudinary, 2022 (updated 2023).
  <https://github.com/cloudinary/ssimulacra2>. Used for: the primary
  size-at-equal-quality metric in the evaluation standard.
- **Sneyers, J.** — *Detecting the psychovisual impact of compression related
  artifacts using SSIMULACRA*, Cloudinary blog, 2017.
  <https://cloudinary.com/blog/detecting_the_psychovisual_impact_of_compression_related_artifacts_using_ssimulacra>.
  Background for the SSIMULACRA family.
- **Mantiuk, R. K., Hanji, P., Ashraf, M., Asano, Y., Chapiro, A.** —
  *ColorVideoVDP: A visual difference predictor for image, video and display
  distortions*, SIGGRAPH 2024 Technical Papers, ACM TOG 43(4), Article 129.
  DOI [10.1145/3658144](https://doi.org/10.1145/3658144), arXiv:2401.11485
  <https://arxiv.org/abs/2401.11485>. Used for: the CVVDP metric (JOD) in
  `docs/METRIC_DECISIONS.md`.
- **Watson, A. B.** — *DCT quantization matrices visually optimized for
  individual images*, Proc. SPIE 1913 (Human Vision, Visual Processing, and
  Digital Display IV), 1993. DOI
  [10.1117/12.152694](https://doi.org/10.1117/12.152694). Studied for:
  quantization-matrix design (`docs/LOW_BITRATE_RESEARCH_2026-10-03.md`).
- **Grajek, T., Stankowski, J.** — *Rate-Distortion Optimized Quantization in
  Motion JPEG*, 2022. DOI
  [10.24132/csrn.3201.29](https://doi.org/10.24132/csrn.3201.29). Studied
  for: RDOQ (`docs/EVAL_RDOQ_2026-10-04.md`).
- **Wang, M., Wang, S., Li, J., Zhang, L., Wang, Y., Ma, S., Kwong, S.** —
  *Low Complexity Trellis-Coded Quantization in Versatile Video Coding*, IEEE
  Transactions on Image Processing, 2021. DOI
  [10.1109/TIP.2021.3051460](https://doi.org/10.1109/TIP.2021.3051460).
  Studied for: trellis quantization.
- **Lin, F., Arunruangsirilert, K., Sun, H., Katto, J.** — *Recoil: Parallel
  rANS Decoding with Decoder-Adaptive Scalability*, International Conference
  on Parallel Processing, 2023. DOI
  [10.1145/3605573.3605588](https://doi.org/10.1145/3605573.3605588).
  Studied for: parallel entropy decoding options
  (`docs/LOW_BITRATE_RESEARCH_2026-10-03.md`).

## Perceptual metrics

- **SSIMULACRA2** — J. Sneyers, Cloudinary, 2022/2023.
  <https://github.com/cloudinary/ssimulacra2>. The primary size metric of the
  evaluation standard; implemented for the harness by `fast-ssim2`.
- **Butteraugli** — J. Alakuijala et al. (JPEG XL project); implementation and
  documentation in the libjxl repository.
  <https://github.com/libjxl/libjxl/tree/main/tools/butteraugli>. Max and
  3-norm guards in the evaluation standard.
- **zensim** — imazen, 2026. <https://github.com/imazen/zensim>. A
  project-internal perceptual metric used alongside the above; reported with
  its preview version in `docs/METRIC_DECISIONS.md`.
- **CVVDP (ColorVideoVDP)** — see the Mantiuk et al. entry above; implementation
  at <https://github.com/gfxdisp/ColorVideoVDP>. JOD units, STANDARD_FHD
  display model.
- **VMAF** — Netflix, 2016–. <https://github.com/Netflix/vmaf>. Reported beside
  CVVDP for every metric-based decision, with the model variant and still-frame
  limitations recorded in `docs/METRIC_DECISIONS.md`.
- **fast-ssim2** — imazen. <https://github.com/imazen/fast-ssim2>. The
  SSIMULACRA2 implementation used by the harness; its per-scale weights are
  also what `imij-cmp/src/ssim2map.rs` attributes the score with.
- **butteraugli (Rust)** — imazen. <https://github.com/imazen/butteraugli>.
  Butteraugli used by the harness.

## Codecs, entropy coders and tooling

- **libjxl** — *JPEG XL reference implementation*, JPEG XL Project Authors,
  BSD-3-Clause. <https://github.com/libjxl/libjxl>. Compared against
  (`zenjxl-decoder` is a Rust decoder) and the source of the XYB opsin
  constants and filter ideas credited in `THIRD_PARTY_NOTICES.md`.
- **jpegli** — Google, BSD-3-Clause. <https://github.com/google/jpegli>.
  Studied for: quantization matrices (including matrix 5's YCbCr base
  tables, per-frequency exponents and nonlinear distance law; SPEC 2.8.1),
  adaptive quantization (AQ) and the compatibility quality scale;
  `docs/LOSSY.md` E6d. Its development moved from libjxl to this separate
  repository; source-path credits distinguish the two codebases.
- **Kornel Lesiński** — [*Deringing in DCT via overshoot and clipping*](https://kornel.ski/deringing/).
  Origin of the overshoot and spline-extrapolation technique used in
  `imij/src/lossy/dering.rs`; his implementation was contributed to mozjpeg.
- **mozjpeg** — Mozilla, BSD-3-Clause / IJG. <https://github.com/mozilla/mozjpeg>.
  Implementation studied for overshoot deringing (`jcdctmgr.c`) and trellis
  quantization (`imij/src/lossy/trellis.rs`).
- **libjpeg / IJG** — Independent JPEG Group. <https://www.ijg.org/>. Studied
  for: the scaled float IDCT (AAN, `jidctflt`) in `imij/src/lossy/dct.rs`.
- **zenjpeg** — imazen. <https://github.com/imazen/zenjpeg>. The JPEG baseline
  used for size/speed comparisons and the trellis/AQ porting reference.
- **libavif** — AOMedia, BSD-2-Clause. <https://github.com/AOMediaCodec/libavif>.
  The AVIF comparison point (through `zenavif` and `rav1d-safe`).
- **SVT-AV1** — AOMedia, BSD-3-Clause.
  <https://gitlab.com/AOMediaCodec/SVT-AV1>. The AVIF encoder used in
  comparisons (through `zenav1-svt`).
- **rav1e** — Xiph.Org, BSD-2-Clause. <https://github.com/xiph/rav1e>. The AV1
  encoder this project's `zenrav1e` fork builds on.
- **libwebp** — WebM Project, BSD-3-Clause.
  <https://github.com/webmproject/libwebp>. Historical comparison point
  (EXPERIMENTS.md).
- **zstd** — Meta, BSD-3-Clause / GPLv2. <https://github.com/facebook/zstd>.
  Result-file compression and the HUF/FSE entropy study in
  `docs/LOW_BITRATE_RESEARCH_2026-10-03.md`.
- **lz4** — Y. Collet, BSD-2-Clause. <https://github.com/lz4/lz4>. The block
  grammar `imij/src/metadata_pack.rs` implements; `lz4_flex` is the Rust
  comparison.
- **OpenJPH** — A. Bilgin et al., BSD-2-Clause.
  <https://github.com/aous72/OpenJPH> (project page:
  <https://openjph.org/>). The HTJ2K baseline in the science comparisons.
- **OpenEXR** — Academy Software Foundation, BSD-3-Clause.
  <https://github.com/AcademySoftwareFoundation/openexr>. Comparison for HDR
  float data (`docs/EVAL_QOIR_OPENEXR_2026-10-04.md`).
- **LERC** — Esri, Apache-2.0. <https://github.com/Esri/lerc>. The
  error-bounded raster comparison for float science data.
- **jbig2enc** — A. Langley, Apache-2.0. <https://github.com/agl/jbig2enc>.
  Bilevel comparison tool (test oracle only, not a dependency).
- **jbig2dec** — Artifex Software. <https://github.com/ArtifexSoftware/jbig2dec>.
  Bilevel comparison tool (test oracle only).
- **libtiff** — libtiff project. <https://gitlab.com/libtiff/libtiff>. The G4
  TIFF decode baseline for bilevel pages.
- **oxipng** — oxipng contributors, MIT. <https://github.com/oxipng/oxipng>.
  The optimized-PNG comparison in `benchmarks/lowbit_2026-10-11/` (palette,
  gray and bit-depth reductions, Zopfli).
- **giflib** — giflib project (through Pillow's GIF writer). <https://giflib.sourceforge.net/>.
  The GIF comparison for palettes in `benchmarks/lowbit_2026-10-11/`.
- **Pillow** — Python Imaging Library. <https://python-pillow.org/>. Image I/O
  in the experiment scripts.
- **imagecodecs** — C. Gohlke, BSD-3-Clause.
  <https://github.com/cgohlke/imagecodecs>. Codec bindings used by the science
  and page baselines.
- **tifffile** — C. Gohlke, BSD-3-Clause. <https://github.com/cgohlke/tifffile>.
  Reads the ESA WorldCover GeoTIFF tiles in `experiments/lowbit_2026-10-11/fetch_corpus.py`.
- **NiBabel** — NiBabel developers, MIT. <https://nipy.org/nibabel/>. Reads the
  Medical Segmentation Decathlon NIfTI label volumes in the same script.
- **QOI** — D. Szablewski, *Quite OK Image Format*.
  <https://qoiformat.org/>. The lossless speed/size comparison that motivated
  the format.
- **Zarr** — Zarr community. <https://zarr-specs.readthedocs.io/>. The chunked
  array format behind OME-NGFF.
- **NetCDF** — Unidata. <https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html>.
  The CF/ERA5 container.
- **wasmtime** — Bytecode Alliance, Apache-2.0 WITH LLVM-exception.
  <https://github.com/bytecodealliance/wasmtime>. Runs the wasm32-wasip1 tests
  and benchmarks.
- **qemu** — QEMU project, GPLv2. <https://www.qemu.org/>. Runs the
  aarch64/NEON test suite on x86 hosts.
- **Intel SDE** — Intel, *Software Development Emulator*.
  <https://www.intel.com/content/www/us/en/developer/articles/tool/software-development-emulator.html>.
  Runs the AVX-512 tiers on hosts without them (`just test-sde`).
- **Valgrind / Callgrind** — Valgrind developers, GPLv2.
  <https://valgrind.org/>. Instruction-count attribution (via gungraun).
- **gungraun** — gungraun project, MIT/Apache-2.0.
  <https://github.com/gungraun/gungraun>. Callgrind bench harness
  (`imij/benches/cg_decode.rs`).
- **heaptrack** — KDE, LGPL-2.1. <https://github.com/KDE/heaptrack>. Peak-heap
  measurements for decode memory claims.
- **Playwright** — Microsoft, Apache-2.0. <https://playwright.dev/>. Browser
  parity and worker-pool tests in `tools/imij-wasm/web`.
- **IBM Plex** — IBM, SIL Open Font License 1.1.
  <https://fonts.google.com/specimen/IBM+Plex+Mono>. The codestream atlas
  viewer's typeface (`tools/codestream-atlas/index.html`).

## Data sets and corpora

- **imazen-26** — imazen, public domain / CC0 / Unsplash-licensed, per-folder
  manifests. <https://github.com/imazen/imazen-26>. The development corpus
  behind most measurements; its `PROVENANCE.md` and `CORPUS-MANIFEST.tsv` are
  the source of truth. The seed banks (`imij/src/seeds/bank1.bin`) are trained
  on its render ladders (`benchmarks/entropy_seeds_2026-10-07/README.md`).
- **codec-corpus** — imazen. <https://github.com/imazen/codec-corpus>. Public
  mirror/curation of the third-party corpora used in comparisons (CID22,
  KADID-10k references, GB82, CLIC 2025, QOI benchmark).
- **CID22 (Cloudinary Image Dataset '22)** — Cloudinary, CC BY-SA 4.0.
  <https://cloudinary.com/labs/cid22>. Held out as validation-only; no source
  from it trains or tunes IMI (`docs/DATA_AND_TOOLS.md`).
- **Medical Segmentation Decathlon (MSD)** — M. Antonelli et al.,
  <http://medicaldecathlon.com/>, CC BY-SA 4.0. The CT/MRI stacks in the pages
  and near-lossless studies (`benchmarks/pages_2026-10-08/`).
- **Copernicus DEM GLO-30** — ESA, 2021. DOI
  [10.5270/ESA-c5d3d65](https://doi.org/10.5270/ESA-c5d3d65). DEM float/u16
  science baselines.
- **ERA5** — H. Hersbach et al., *The ERA5 global reanalysis*, Quarterly
  Journal of the Royal Meteorological Society 146(730), 2020, pp. 1999–2049.
  DOI [10.1002/qj.3803](https://doi.org/10.1002/qj.3803) (publisher blocks
  automated checks). NetCDF weather fields in the science baselines.
- **Sentinel-2 L2A** — ESA Copernicus.
  <https://www.esa.int/Applications/Observing_the_Earth/Copernicus/Sentinel-2>.
  u16 remote-sensing tiles in the near-lossless and science baselines.
- **USGS 3DEP** — U.S. Geological Survey, *3D Elevation Program*.
  <https://www.usgs.gov/3d-elevation-program> (site blocks automated checks).
  Elevation GeoTIFFs in the float-recipe studies.
- **Unsplash License** — Unsplash. <https://unsplash.com/license>. The 51
  Unsplash photographs in imazen-26; attribution is kept in file names and
  manifests.
- **ESA WorldCover 10 m 2021 v200** — D. Zanaga et al., ESA WorldCover
  project, CC BY 4.0. DOI [10.5281/zenodo.7254221](https://doi.org/10.5281/zenodo.7254221);
  tiles from <https://esa-worldcover.s3.eu-central-1.amazonaws.com/>. Land-cover
  class rasters (11 classes) in the low-bit study (`benchmarks/lowbit_2026-10-11/`);
  © ESA WorldCover project 2021 / contains modified Copernicus Sentinel data
  (2021) processed by the ESA WorldCover consortium.
- **Medical Segmentation Decathlon, Task05 Prostate labels** — M. Antonelli et
  al., *The Medical Segmentation Decathlon*, Nature Communications 13, 4128,
  2022, DOI [10.1038/s41467-022-30695-9](https://doi.org/10.1038/s41467-022-30695-9);
  A. L. Simpson et al., arXiv:1902.09063, 2019. CC BY-SA 4.0. Label slices
  (two and three classes) in the low-bit study.
- **Wikimedia Commons** — Wikimedia Foundation and contributors,
  <https://commons.wikimedia.org/>. Screenshots, pixel art, icons and other
  images with at most 32 colors under each file's own free license (recorded
  per file, with its author, in the low-bit study's data pointer).
- **US patent page scans** — Google Patents PDF scans
  (`patentimages.storage.googleapis.com`); US patents are U.S. government works
  in the public domain. Used for the bilevel page comparison
  (`experiments/pages_2026-10-08/README.md`).

## Project-internal crates and docs

IMI's own crates are documented in [ARCHITECTURE.md](ARCHITECTURE.md) and their
repository READMEs: `imij`, `imij-bench`, `imij-cmp`, `tools/imij-seeds`,
`tools/imij-wasm`, plus the sibling crates it builds on (`archmage`,
`magetypes`, `zencodec`, `zenhash`, `zenjpeg`, `zenjxl-decoder`, `zenavif`,
`zenpng`, `zenresize`, `zensim`, `fast-ssim2`, `butteraugli`, `cvvdp`, `vmaf`,
`jxl-encoder`, `zenrav1e`). They are not third-party works and need no
attribution here; third-party components that this repository derives code or
constants from are listed in [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).
