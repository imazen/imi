//! Codec-only A/B/C study, compiled in lab test binaries only.
//!
//! A is unchanged one-pass; B learns compatible DC tables from its first band;
//! C stores a complete existing numeric DATA payload per independently trained
//! group, with one image declaration and an explicit index. C is a private
//! experiment container, not an IMI allocation or a production file format.
use super::*;
use crate::ImiEncoderConfig;
use crate::parse::header::{Features, Header};
use core::future::Future;
use core::task::{Context, Poll, Waker};

mod grouping;
mod indexed;
mod native;
mod native_models;
mod palette;
mod run;

const MAGIC: &[u8; 8] = b"IMICTX01";
const FOOTER: &[u8; 8] = b"CTXIDX01";

fn digest(bytes: &[u8]) -> String {
    let digest = crate::HashAlgorithm::Xxh3_128.digest(bytes);
    format!(
        "{:032x}",
        u128::from_le_bytes(digest.as_bytes().try_into().unwrap())
    )
}

fn ready<F: Future>(operation: F) -> F::Output {
    match core::pin::pin!(operation)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("resident study I/O must resolve synchronously"),
    }
}

struct ResidentIo<'a> {
    source: &'a [u8],
    row_bytes: usize,
    output: Vec<u8>,
    index: Vec<u8>,
    calls: Vec<(u8, usize, usize)>,
}
impl StreamIo for ResidentIo<'_> {
    async fn source(
        &mut self,
        pass: u8,
        y: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        assert_eq!(out.len() % self.row_bytes, 0);
        let start = y.checked_mul(self.row_bytes).ok_or(Error::TooLarge)?;
        let end = start.checked_add(out.len()).ok_or(Error::TooLarge)?;
        out.copy_from_slice(
            self.source
                .get(start..end)
                .ok_or(Error::InvalidInput("study source extent"))?,
        );
        self.calls.push((pass, y, out.len() / self.row_bytes));
        Ok(())
    }
    async fn write(
        &mut self,
        stream: Stream,
        at: u64,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let dst = match stream {
            Stream::Output => &mut self.output,
            Stream::Index => &mut self.index,
            Stream::Checksum => panic!("study integrity policy is off for every arm"),
        };
        let at = usize::try_from(at).map_err(|_| Error::TooLarge)?;
        let end = at.checked_add(bytes.len()).ok_or(Error::TooLarge)?;
        assert!(at <= dst.len());
        dst.resize(end.max(dst.len()), 0);
        dst[at..end].copy_from_slice(bytes);
        Ok(())
    }
    async fn read(
        &mut self,
        stream: Stream,
        at: u64,
        out: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let data = match stream {
            Stream::Index => &self.index,
            Stream::Output => panic!("no published output replay"),
            Stream::Checksum => panic!("no checksum scratch"),
        };
        let at = usize::try_from(at).map_err(|_| Error::TooLarge)?;
        out.copy_from_slice(
            data.get(at..at + out.len())
                .ok_or(Error::Corrupt("study scratch bounds"))?,
        );
        Ok(())
    }
}

fn encode_native(info: &ImageInfo, pixels: &[u8], cfg: &LossyConfig, mode: Preparation) -> Vec<u8> {
    assert!(!cfg.checksum);
    assert_eq!(pixels.len(), info.row_bytes() * info.height as usize);
    let mut io = ResidentIo {
        source: pixels,
        row_bytes: info.row_bytes(),
        output: Vec::new(),
        index: Vec::new(),
        calls: Vec::new(),
    };
    let size = ready(encode_lossy_stream_io(
        info,
        &Metadata::default(),
        cfg,
        &mut io,
        StreamIndex::Trailing,
        0,
        None,
        16 << 20,
        mode,
    ))
    .unwrap();
    assert_eq!(size as usize, io.output.len());
    let expected: &[u8] = match mode {
        Preparation::OnePass | Preparation::StudyFirstBandDc => &[2],
        Preparation::Measured => &[1, 2],
        Preparation::Budgeted(_) => panic!("not a rate experiment"),
    };
    for &pass in expected {
        let mut next = 0;
        for &(_, y, rows) in io.calls.iter().filter(|&&(p, _, _)| p == pass) {
            assert_eq!(
                y, next,
                "source requests are consecutive and nonoverlapping"
            );
            next += rows;
        }
        assert_eq!(next, info.height as usize);
    }
    assert!(io.calls.iter().all(|(p, _, _)| expected.contains(p)));
    io.output
}

#[derive(Debug)]
struct CodingStats {
    ac_bytes: usize,
    dc_bytes: usize,
    numeric_other_bytes: usize,
    outer_bytes: usize,
    coefficients: String,
    dc_u16_groups: usize,
    dc_i32_groups: usize,
}

fn coefficient_digest(hasher: &mut crate::Hasher, values: &[i32]) {
    for value in values {
        hasher.update(&value.to_le_bytes());
    }
}
fn native_stats(encoded: &[u8]) -> CodingStats {
    let (_, _, values, ac, dc) = super::super::quantized_lab(encoded).unwrap();
    let (_, _, body, _) = crate::v1::lossy_body(encoded).unwrap().unwrap();
    let (info, _, _) = super::super::dc_image_lab(encoded).unwrap();
    let mut h = crate::Hasher::new(crate::HashAlgorithm::Xxh3_128);
    coefficient_digest(&mut h, &values);
    CodingStats {
        ac_bytes: ac,
        dc_bytes: dc,
        numeric_other_bytes: body.len() - ac - dc,
        outer_bytes: encoded.len() - body.len(),
        coefficients: format!(
            "{:032x}",
            u128::from_le_bytes(h.finish().as_bytes().try_into().unwrap())
        ),
        dc_u16_groups: usize::from(info.sample == SampleType::U16),
        dc_i32_groups: usize::from(info.sample == SampleType::I32),
    }
}

fn encode_contexts(
    info: &ImageInfo,
    pixels: &[u8],
    cfg: &LossyConfig,
    group_rows: usize,
    common_native: &[u8],
) -> (Vec<u8>, CodingStats) {
    assert!(group_rows > 0 && group_rows.is_multiple_of(8));
    let mut reader = ByteReader::new(common_native);
    crate::parse::header::read_header_sized(&mut reader).unwrap();
    let common = &common_native[..reader.pos];
    let mut out = MAGIC.to_vec();
    put_varint(&mut out, common.len() as u64);
    out.extend_from_slice(common);
    put_varint(&mut out, group_rows as u64);
    let mut spans = Vec::new();
    let mut hash = crate::Hasher::new(crate::HashAlgorithm::Xxh3_128);
    let mut stats = CodingStats {
        ac_bytes: 0,
        dc_bytes: 0,
        numeric_other_bytes: 0,
        outer_bytes: 0,
        coefficients: String::new(),
        dc_u16_groups: 0,
        dc_i32_groups: 0,
    };
    for (ordinal, band) in pixels.chunks(group_rows * info.row_bytes()).enumerate() {
        let mut local = info.clone();
        local.height = (band.len() / info.row_bytes()) as u32;
        // The upstream source enters once as this bounded band. Only this band
        // is visible to the existing statistics/coding passes; no future rows.
        let encoded =
            super::super::encode_lossy(band, local.row_bytes(), &local, &Metadata::default(), cfg)
                .unwrap();
        let (_, _, coefficients, ac, dc) = super::super::quantized_lab(&encoded).unwrap();
        coefficient_digest(&mut hash, &coefficients);
        let (_, _, body, features) = crate::v1::lossy_body(&encoded).unwrap().unwrap();
        let (dc_info, _, _) = super::super::dc_image_lab(&encoded).unwrap();
        stats.dc_u16_groups += usize::from(dc_info.sample == SampleType::U16);
        stats.dc_i32_groups += usize::from(dc_info.sample == SampleType::I32);
        stats.ac_bytes += ac;
        stats.dc_bytes += dc;
        stats.numeric_other_bytes += body.len() - ac - dc;
        let start = out.len();
        put_varint(&mut out, ordinal as u64);
        put_varint(&mut out, body.len() as u64);
        put_varint(&mut out, features.bits());
        out.extend_from_slice(body);
        spans.push((start as u64, (out.len() - start) as u64));
    }
    let index = out.len() as u64;
    for (start, len) in spans {
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
    }
    out.extend_from_slice(&index.to_le_bytes());
    out.extend_from_slice(FOOTER);
    stats.outer_bytes = out.len() - stats.ac_bytes - stats.dc_bytes - stats.numeric_other_bytes;
    stats.coefficients = format!(
        "{:032x}",
        u128::from_le_bytes(hash.finish().as_bytes().try_into().unwrap())
    );
    (out, stats)
}

#[derive(Clone)]
struct GroupContext<'a> {
    body: &'a [u8],
    features: Features,
}
struct ContextFile<'a> {
    header: Header,
    group_rows: usize,
    groups: Vec<GroupContext<'a>>,
    header_bytes: usize,
    index_start: usize,
}
impl<'a> ContextFile<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let mut source = crate::planes::SliceReader::new(bytes);
        let directory = indexed::read_directory(&mut source)?;
        let groups = directory
            .groups
            .into_iter()
            .map(|g| GroupContext {
                body: &bytes[g.body],
                features: g.features,
            })
            .collect();
        Ok(Self {
            header: directory.header,
            group_rows: directory.group_rows,
            groups,
            header_bytes: directory.header_bytes,
            index_start: directory.index_start,
        })
    }

    fn decode(&self, threads: usize) -> Result<Vec<u8>, Error> {
        let info = &self.header.info;
        let row = info.row_bytes();
        let mut pixels = vec![0; row * info.height as usize];
        let jobs = self
            .groups
            .iter()
            .zip(pixels.chunks_mut(self.group_rows * row))
            .collect();
        let results = crate::par::par_map(jobs, threads, |_, (group, out)| {
            let mut local = info.clone();
            local.height = (out.len() / row) as u32;
            decode_group(local, &self.header, group.features, group.body, out, row)
        });
        for result in results {
            result?;
        }
        Ok(pixels)
    }
    fn center_pixel(&self) -> Result<(Vec<u8>, usize), Error> {
        let info = &self.header.info;
        let y = info.height as usize / 2;
        let ordinal = y / self.group_rows;
        let group = &self.groups[ordinal];
        let height = self
            .group_rows
            .min(info.height as usize - ordinal * self.group_rows);
        let mut local = info.clone();
        local.height = height as u32;
        let mut pixels = vec![0; info.row_bytes() * height];
        decode_group(
            local,
            &self.header,
            group.features,
            group.body,
            &mut pixels,
            info.row_bytes(),
        )?;
        let at = y % self.group_rows * info.row_bytes()
            + info.width as usize / 2 * info.bytes_per_pixel();
        let record_heads = self.index_start
            - self.header_bytes
            - self.groups.iter().map(|g| g.body.len()).sum::<usize>();
        let required =
            self.header_bytes + record_heads + group.body.len() + self.groups.len() * 16 + 16;
        Ok((pixels[at..at + info.bytes_per_pixel()].to_vec(), required))
    }
}

fn decode_group(
    info: ImageInfo,
    header: &Header,
    features: Features,
    body: &[u8],
    out: &mut [u8],
    stride: usize,
) -> Result<(), Error> {
    let parsed = super::super::parse_lossy_as(
        info,
        header.version,
        header.lossy_color.unwrap(),
        features,
        body,
    )?;
    if parsed
        .reserve
        .as_ref()
        .is_some_and(|reserve| reserve.check_tail())
    {
        return Err(Error::Corrupt(
            "study context still requires a child trailer",
        ));
    }
    super::super::decode_lossy_parsed(parsed, out, stride, 1, &crate::Limits::DEFAULT, true, false)
        .map(|_| ())
}

fn decode_native(data: &[u8], info: &ImageInfo, threads: usize) -> Vec<u8> {
    let mut pixels = vec![0; info.row_bytes() * info.height as usize];
    assert_eq!(
        crate::decode_into_threads(data, &mut pixels, info.row_bytes(), threads).unwrap(),
        *info
    );
    pixels
}

fn configuration(
    quality: f32,
    color: LossyColor,
    entropy: LossyEntropy,
    group_rows: usize,
) -> LossyConfig {
    ImiEncoderConfig::lossy(quality)
        .with_effort(0.0)
        .with_threads(1)
        .with_column_groups(Some(0))
        .with_checksum(false)
        .tune_lossy(|t| {
            t.color = color;
            t.entropy = entropy;
            t.stripe_block_rows = (group_rows / 8) as u32;
            t.aq_gamma = 0.0;
            t.small_transforms = false;
            t.adaptive_partitions = false;
            t.exact_blocks = Some(0.0);
        })
        .lossy_config()
}

#[test]
fn coding_context_comparison() {
    // This is a real small regression without the explicit measurement request;
    // it never skips because a scientific corpus or output directory is absent.
    run::small_regression();
    if let Some(manifest) = std::env::var_os("IMI_CODING_CONTEXT_MANIFEST") {
        run::measure(std::path::Path::new(&manifest));
    }
}
