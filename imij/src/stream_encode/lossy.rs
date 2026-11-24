//! Bounded lossy output from a seekable raster (issue #14).
use super::StreamEncodeError;
use crate::config::Lowered;
use crate::lossy::bounded::{StreamIndex, encode_lossy_bounded_to};
use crate::{Error, ImageInfo, ImiEncoderConfig, Metadata};
use how_far::Pulse;
use std::io::{Read, Seek, SeekFrom, Write};

/// Where the unit table goes (SPEC 2.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LossyUnitIndex {
    /// Head record 12 reserved at this many bytes (1 through 8) per main
    /// record and filled in place after the last band, so readers load the
    /// table with the head. A record span too wide for it turns the
    /// reservation into check tail and appends a trailing `uIDX` instead.
    Reserved(u8),
    /// Head record 12 says check tail; the spans follow in a trailing `uIDX`.
    Trailing,
}

/// Source layout and index budget for [`encode_lossy_bounded`].
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct BoundedLossyConfig {
    /// Bytes from one source row to the next (at least the packed row).
    pub source_stride_bytes: u64,
    pub index: LossyUnitIndex,
    /// Index scratch budget: records the nested pass codes ahead of pass 2
    /// (exact-block stripes, and DC records on tiers whose DC stripes span
    /// several bands), then a conservative ten bytes per record for the index.
    /// Also the most bytes appended to checksum scratch (eight per piece).
    pub max_index_bytes: u64,
}

impl BoundedLossyConfig {
    pub const fn new(
        source_stride_bytes: u64,
        index: LossyUnitIndex,
        max_index_bytes: u64,
    ) -> Self {
        Self {
            source_stride_bytes,
            index,
            max_index_bytes,
        }
    }
}

/// Encodes a lossy imij from a raster too large to hold, reading it twice.
///
/// `source` holds rows of `info` (native sample layout) at
/// `cfg.source_stride_bytes` from its position at the call. Pass 1 measures
/// the DC range, tier statistics and entropy histograms; pass 2 seeks back
/// and codes. Codec working storage holds one work band (whole lossy
/// stripes, about 8 block rows, full width) of rows and coefficients, plus
/// the current DC stripe's DC image (8 bytes per block) on tiers whose DC
/// stripes are taller; record spans go to
/// `index_scratch`. `writer` receives the header and `DATA` head first and
/// each band's records as they are coded; with [`LossyUnitIndex::Reserved`]
/// it is seeked back once at the end to fill the head reservation.
///
/// `checksum_scratch` is required exactly when `lossy.checksum` is set:
/// the file checksum (SPEC 1.4) is a `cSUM` chunk of 1 MiB pieces after the
/// records (before a trailing `uIDX`), its digests spooled to that scratch.
/// The bytes up to the end of the head reservation's piece (at most one
/// piece past the reservation) stay in memory until the reservation is
/// filled.
///
/// The file decodes to the same pixels as [`crate::ImiEncoderConfig::encode`] with the
/// same `lossy` configuration. Supported: Gray, GrayAlpha, RGB and RGBA of u8,
/// u16, u32 (extended tier), f16 or f32 samples, with declared significant
/// bits (each band is range-checked as it is read), every tier, channel
/// layouts, small transforms and a decoder memory budget, as `encode_lossy`
/// takes them; AQ and explicit quantization tables are `Unsupported`. Efforts 9 and 10
/// run the AC trellis with one more source pass per rate-model pass and
/// refuse images of at most 64 Ki pixels. Exact blocks (8-bit RGB(A), as
/// `lossy.exact_blocks` or the Compact tier selects them) add two source
/// passes: one prices them over the whole image, one codes the nested exact
/// image, whose stripes spool to `index_scratch` ahead of the index entries
/// and are copied into the output in pass 2. The same blocks are exact as
/// with `encode_lossy`. Superblock merges (the Compact tier, 8-bit images
/// with a side of 128 or more) add one source pass that prices them over the
/// whole image; the coding passes choose them stripe by stripe, the same
/// merges as `encode_lossy`. Lossless coding choices differ from `encode_lossy`
/// (explicit AC tables; nested DC and exact images coded in the bounded
/// group grammar with tables measured in pass 1), so files are slightly
/// larger and not byte-identical.
///
/// `progress` advances by one per source row read (the height per source
/// pass) and is checked at every band; a stop returns `Error::Cancelled`.
/// Discard partial output after any error.
#[allow(clippy::too_many_arguments)]
pub fn encode_lossy_bounded<R, W, S>(
    source: &mut R,
    writer: W,
    index_scratch: S,
    checksum_scratch: Option<S>,
    info: ImageInfo,
    metadata: &Metadata,
    encoder: &ImiEncoderConfig,
    cfg: BoundedLossyConfig,
    progress: &dyn Pulse,
) -> Result<(W, S, Option<S>), StreamEncodeError>
where
    R: Read + Seek,
    W: Write + Seek,
    S: Read + Write + Seek,
{
    encode_lossy_bounded_version(
        source,
        writer,
        index_scratch,
        checksum_scratch,
        info,
        metadata,
        encoder,
        cfg,
        progress,
        None,
    )
}

pub(crate) fn encode_lossy_bounded_version<R, W, S>(
    source: &mut R,
    mut writer: W,
    index_scratch: S,
    checksum_scratch: Option<S>,
    info: ImageInfo,
    metadata: &Metadata,
    encoder: &ImiEncoderConfig,
    cfg: BoundedLossyConfig,
    progress: &dyn Pulse,
    v4: Option<(crate::v4::DirectoryPlacement, u64)>,
) -> Result<(W, S, Option<S>), StreamEncodeError>
where
    R: Read + Seek,
    W: Write + Seek,
    S: Read + Write + Seek,
{
    let lossy = &lossy_of(encoder)?;
    let row_bytes = info.row_bytes() as u64;
    if info.width == 0 || info.height == 0 || cfg.source_stride_bytes < row_bytes {
        return Err(Error::InvalidInput("bounded lossy source stride").into());
    }
    let index = if let Some((directory, capacity)) = v4 {
        StreamIndex::V4 {
            directory,
            capacity,
        }
    } else {
        match cfg.index {
            LossyUnitIndex::Reserved(w) => StreamIndex::Reserved(w),
            LossyUnitIndex::Trailing => StreamIndex::Trailing,
        }
    };
    let origin = source.stream_position()?;
    let stride = cfg.source_stride_bytes;
    let mut read_error: Option<std::io::Error> = None;
    let mut rows = |_pass: u8, y0: usize, out: &mut [u8]| -> Result<(), Error> {
        progress.check().map_err(|_| Error::Cancelled)?;
        let r = (|| -> std::io::Result<()> {
            let at = (y0 as u64)
                .checked_mul(stride)
                .and_then(|n| n.checked_add(origin))
                .ok_or_else(|| std::io::Error::other("source offset overflow"))?;
            source.seek(SeekFrom::Start(at))?;
            for (i, row) in out.chunks_mut(row_bytes as usize).enumerate() {
                if i != 0 && stride != row_bytes {
                    source.seek(SeekFrom::Current((stride - row_bytes) as i64))?;
                }
                source.read_exact(row)?;
            }
            Ok(())
        })();
        if let Err(e) = r {
            read_error = Some(e);
            return Err(Error::Read);
        }
        progress.advance((out.len() as u64) / row_bytes);
        Ok(())
    };
    let r = encode_lossy_bounded_to(
        &info,
        metadata,
        lossy,
        &mut rows,
        &mut writer,
        index,
        index_scratch,
        checksum_scratch,
        cfg.max_index_bytes,
        progress,
    );
    if let Some(e) = read_error {
        return Err(StreamEncodeError::Read(e));
    }
    let (index, sums) = r?;
    Ok((writer, index, sums))
}

/// A stream a [`BoundedLossySession`] step writes or reads. Each has its
/// own offsets, from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BoundedStream {
    /// The encoded file. Written in order, except the head unit-table
    /// reservation ([`LossyUnitIndex::Reserved`]), which is written again
    /// after the last band.
    Output,
    /// Index scratch: the records the nested pass codes ahead of pass 2,
    /// then one varint per main record; read back in pass 2 and at the end.
    /// At most `max_index_bytes`.
    IndexScratch,
    /// Checksum scratch (with `lossy.checksum`): eight bytes per 1 MiB
    /// piece, read back once at the end.
    ChecksumScratch,
}

/// What a [`BoundedLossySession`] needs next. The borrowed buffer belongs to
/// the session; the next [`BoundedLossySession::step`] takes the answer.
#[derive(Debug)]
#[non_exhaustive]
pub enum BoundedLossyStep<'a> {
    /// Fill `rows` with source rows from row `y0`: whole rows of
    /// `info.row_bytes()`, tightly packed. Each pass asks for every band
    /// once, top to bottom; `pass` tells passes apart (0 exact-block prices,
    /// 4 superblock prices, 1 statistics, 10 + k trellis rate model k,
    /// 3 nested images, 2 coding).
    Rows {
        pass: u8,
        y0: u32,
        rows: &'a mut [u8],
    },
    /// Write `bytes` at `offset` of `stream`.
    Write {
        stream: BoundedStream,
        offset: u64,
        bytes: &'a [u8],
    },
    /// Read `buf.len()` bytes at `offset` of `stream` into `buf` (bytes this
    /// session wrote there earlier).
    Read {
        stream: BoundedStream,
        offset: u64,
        buf: &'a mut [u8],
    },
    /// The file is complete: `len` bytes of [`BoundedStream::Output`].
    Done { len: u64 },
}

/// The source reads a [`BoundedLossySession`] plans, from the image geometry
/// and configuration alone: known before its first step.
///
/// Every pass reads every source row once, top to bottom, in
/// [`BoundedLossyStep::Rows`] requests of at most `band_rows` rows. The
/// coding pass (`pass` 2) is always the last, and the session writes no
/// [`BoundedStream::Output`] byte before every earlier pass has finished.
/// Lab one-pass and budgeted one-pass sessions have `min == max == 1`;
/// their sole source traversal retains the native coding label 2. Output can
/// become available during that traversal, before later source bands arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BoundedLossyPasses {
    /// Passes every image of this geometry and configuration takes.
    pub min: u32,
    /// Passes when every content-dependent pass runs; `min` when none can.
    /// The nested pass of exact blocks runs only when their prices select
    /// some, unless DC stripes span several work bands (then it always
    /// runs). With a decode speed whose tier compares entropy codings, the
    /// trellis rate-model passes can depend on pass 1's statistics.
    pub max: u32,
    /// The most rows one `Rows` request asks for: one work band. The last
    /// band of the image can be shorter.
    pub band_rows: u32,
}

/// A validated bounded lossy encode retained across application admission.
///
/// Owns the exact image declaration, lowered configuration and supplied metadata.
/// Preparation does not construct a session, poll native work, read pixels or
/// allocate sample conversion tables, raster/coefficients, mailbox or future.
/// Configuration lowering may copy configuration-owned data (not source pixels);
/// metadata is moved in. No whole-operation workspace bound follows from this.
/// Sample and metadata validity and content-dependent scratch requirements can
/// still cause execution to fail. Preparation does not validate all metadata.
///
/// Inspect [`Self::passes`], then consume this same owner with [`Self::start`].
/// Startup accepts no replacement declaration/configuration/metadata. Changes to
/// the original caller configuration cannot change this prepared encode.
pub struct BoundedLossyPlan {
    inner: crate::lossy::bounded::SessionPlan,
}

impl BoundedLossyPlan {
    /// Validate native settings, geometry, reservation width and the minimum
    /// index budget before allocating native execution state. Moves metadata
    /// without cloning its payloads; does not inspect a source or output backend.
    pub fn new(
        info: ImageInfo,
        metadata: Metadata,
        cfg: &ImiEncoderConfig,
        index: LossyUnitIndex,
        max_index_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        let index = match index {
            LossyUnitIndex::Reserved(w) => StreamIndex::Reserved(w),
            LossyUnitIndex::Trailing => StreamIndex::Trailing,
        };
        Ok(Self {
            inner: crate::lossy::bounded::SessionPlan::new(
                info,
                metadata,
                lossy_of(cfg)?,
                index,
                max_index_bytes,
            )?,
        })
    }

    pub(crate) fn v4(
        info: ImageInfo,
        metadata: Metadata,
        cfg: &ImiEncoderConfig,
        max_index_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        Ok(Self {
            inner: crate::lossy::bounded::SessionPlan::new(
                info,
                metadata,
                lossy_of(cfg)?,
                StreamIndex::V4 {
                    directory: crate::v4::DirectoryPlacement::Tail,
                    capacity: 0,
                },
                max_index_bytes,
            )?,
        })
    }

    /// Private bridge for the common adapter's opt-in lab streaming profile.
    /// Keep the exact validated preparation until startup.
    #[cfg(feature = "lab")]
    pub(crate) fn one_pass(
        info: ImageInfo,
        metadata: Metadata,
        cfg: &ImiEncoderConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        Ok(Self {
            inner: crate::lossy::bounded::SessionPlan::one_pass(
                info,
                metadata,
                lossy_of(cfg)?,
                max_index_bytes,
                max_working_bytes,
            )?,
        })
    }

    /// Source pass bounds and band size from this retained native plan.
    pub fn passes(&self) -> BoundedLossyPasses {
        let (min, max, band_rows) = self.inner.passes();
        BoundedLossyPasses {
            min,
            max,
            band_rows,
        }
    }

    /// The exact image declaration retained for startup.
    pub fn info(&self) -> &ImageInfo {
        self.inner.info()
    }

    /// The exact retained metadata; this does not certify its privacy contents.
    pub fn metadata(&self) -> &Metadata {
        self.inner.metadata()
    }

    /// Materialize sample tables and session state after application admission.
    /// The returned session has not read a pixel or exposed output.
    pub fn start(self) -> Result<BoundedLossySession, StreamEncodeError> {
        Ok(BoundedLossySession {
            inner: self.inner.start()?,
        })
    }
}

/// [`encode_lossy_bounded`] without I/O: a state machine that asks its caller
/// for every source band, output write and scratch access, so it runs over
/// any transport (async runtimes, browser workers, remote storage) without
/// threads or blocking. The bytes are those [`encode_lossy_bounded`] writes
/// for the same arguments.
///
/// Call [`Self::step`] until it returns [`BoundedLossyStep::Done`]; handle
/// each step before the next call. Progress is the `y0` of `Rows` steps;
/// to cancel, stop stepping and drop the session. Working memory is that of
/// [`encode_lossy_bounded`] plus one request buffer (a band's rows or one
/// record). The session is `Send`: handle the current request and end its
/// borrow before moving the owner to another thread. This adds no worker bridge.
pub struct BoundedLossySession {
    inner: crate::lossy::bounded::Session,
}

impl BoundedLossySession {
    /// Plans the encode; refuses what [`encode_lossy_bounded`] refuses.
    pub fn new(
        info: ImageInfo,
        metadata: &Metadata,
        cfg: &ImiEncoderConfig,
        index: LossyUnitIndex,
        max_index_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        let index = match index {
            LossyUnitIndex::Reserved(w) => StreamIndex::Reserved(w),
            LossyUnitIndex::Trailing => StreamIndex::Trailing,
        };
        Ok(Self {
            inner: crate::lossy::bounded::Session::new(
                info,
                metadata.clone(),
                lossy_of(cfg)?,
                index,
                max_index_bytes,
            )?,
        })
    }

    /// The source passes and request size this session plans; see
    /// [`BoundedLossyPasses`]. Reads no source row.
    pub fn passes(&self) -> BoundedLossyPasses {
        let (min, max, band_rows) = self.inner.passes();
        BoundedLossyPasses {
            min,
            max,
            band_rows,
        }
    }

    /// Experimental single-pass native output (`lab` only).
    ///
    /// Requests every source band once, in increasing row order (pass2), and
    /// writes append-only native output with a trailing unit index. It uses
    /// entropy models fixed after training on the first band (with positive
    /// support for future symbols), and signed32 DC. Bytes/rate differ from the
    /// two-pass encoder. Reconstructed pixels follow the same quantization.
    ///
    /// Requires effort0, AQ off, default8x8 layouts, no exact blocks, merges,
    /// trellis, inter reference, explicit color transform or unit ordinals.
    /// Unsupported requested
    /// options fail before the first request. Band height is the configured
    /// lossy stripe height; a complete band must be supplied to produce its
    /// rows. Later bands need not be available. Caller-owned index and optional
    /// checksum scratch remain required and bounded by `max_index_bytes`.
    ///
    /// `max_working_bytes` admits a conservative codec allocation estimate,
    /// including metadata copies, before source/output I/O. It excludes the
    /// caller's source, scratch and transport/decoder queues and is not a
    /// process RSS guarantee. Dropping or failing the session cancels it;
    /// discard partial output and any provisional decoded rows after errors.
    /// Real-video latency, rate cost and resource peaks require qualification.
    #[cfg(feature = "lab")]
    pub fn one_pass(
        info: ImageInfo,
        metadata: &Metadata,
        cfg: &ImiEncoderConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        let lossy = lossy_of(cfg)?;
        // Admission precedes cloning potentially large caller metadata.
        crate::lossy::bounded::one_pass::admit(
            &info,
            metadata,
            &lossy,
            max_index_bytes,
            max_working_bytes,
        )?;
        Ok(Self {
            inner: crate::lossy::bounded::Session::one_pass(
                info,
                metadata.clone(),
                lossy,
                max_index_bytes,
                max_working_bytes,
            )?,
        })
    }

    /// Experimental AQ-controlled byte budgets (`lab`, proposed for review).
    ///
    /// Extends the one-pass profile with AC scales64/80/96/128/160/192/224/255
    /// over64. Each source band is read once and staged before publication;
    /// the first tested scale fitting the byte limits is selected. DC steps,
    /// exact alpha and entropy models remain fixed by the frame head.
    ///
    /// `max_output_bytes` counts the entire native file, including headers,
    /// tables, checksums and trailing index. `max_band_record_bytes` counts
    /// all framed DC/alpha/AC records of each source band. A conservative tail
    /// reservation can refuse a budget even if another encoder could fit.
    /// Exhaustion returns `TooLarge`; discard partial/provisional output.
    /// This does not pad a constant-rate stream or guarantee real-time work.
    ///
    /// Supports U8/U16 Gray/GrayAlpha/RGB/RGBA under the one-pass restrictions.
    /// Additional trial working storage is admitted before metadata cloning.
    #[cfg(feature = "lab")]
    pub fn one_pass_budgeted(
        info: ImageInfo,
        metadata: &Metadata,
        cfg: &ImiEncoderConfig,
        max_index_bytes: u64,
        max_working_bytes: u64,
        max_output_bytes: u64,
        max_band_record_bytes: u64,
    ) -> Result<Self, StreamEncodeError> {
        let lossy = lossy_of(cfg)?;
        let limits = crate::lossy::bounded::one_pass::rate::Limits {
            frame_bytes: max_output_bytes,
            band_record_bytes: max_band_record_bytes,
        };
        crate::lossy::bounded::one_pass::rate::admit(
            &info,
            metadata,
            &lossy,
            max_index_bytes,
            max_working_bytes,
            limits,
        )?;
        Ok(Self {
            inner: crate::lossy::bounded::Session::budgeted(
                info,
                metadata.clone(),
                lossy,
                max_index_bytes,
                max_working_bytes,
                limits,
            )?,
        })
    }

    pub(crate) fn pending_rows(&mut self) -> Option<BoundedLossyStep<'_>> {
        let (pass, y0, rows) = self.inner.pending_rows()?;
        Some(BoundedLossyStep::Rows {
            pass,
            y0: y0 as u32,
            rows,
        })
    }

    /// Completes the previous step and returns the next. After an error the
    /// session is finished; discard partial output.
    pub fn step(&mut self) -> Result<BoundedLossyStep<'_>, StreamEncodeError> {
        use crate::lossy::bounded::{Step, Stream};
        let stream = |s: Stream| match s {
            Stream::Output => BoundedStream::Output,
            Stream::Index => BoundedStream::IndexScratch,
            Stream::Checksum => BoundedStream::ChecksumScratch,
        };
        Ok(match self.inner.step()? {
            Step::Rows { pass, y0, rows } => BoundedLossyStep::Rows {
                pass,
                y0: y0 as u32,
                rows,
            },
            Step::Write {
                stream: s,
                at,
                bytes,
            } => BoundedLossyStep::Write {
                stream: stream(s),
                offset: at,
                bytes,
            },
            Step::Read { stream: s, at, buf } => BoundedLossyStep::Read {
                stream: stream(s),
                offset: at,
                buf,
            },
            Step::Done { len } => BoundedLossyStep::Done { len },
        })
    }

    /// The caller could not handle the last step: the encode stops, and the
    /// next [`Self::step`] returns the first original `error`. Pending native
    /// state is released immediately and later calls cannot resume it.
    pub fn fail(&mut self, error: StreamEncodeError) {
        self.inner.fail(error);
    }
}

/// The lossy coder's settings, or the refusal for a lossless configuration.
fn lossy_of(cfg: &ImiEncoderConfig) -> Result<crate::lossy::LossyConfig, Error> {
    match cfg.lower() {
        Lowered::Lossy(c) => Ok(c),
        _ => Err(Error::Unsupported("bounded lossy encoding is lossy")),
    }
}

/// Virtual positioning for a forward-only destination. The selected trailing
/// writer never seeks backwards; rejecting such a request preserves that contract.
pub(crate) struct ForwardOutput<W> {
    pub(crate) inner: W,
    position: u64,
}
impl<W> ForwardOutput<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self { inner, position: 0 }
    }
}
impl<W: Write> Write for ForwardOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.position = self
            .position
            .checked_add(count as u64)
            .ok_or_else(|| std::io::Error::other("forward output offset overflow"))?;
        Ok(count)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
impl<W: Write> Seek for ForwardOutput<W> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        match to {
            SeekFrom::Current(0) => Ok(self.position),
            SeekFrom::Start(at) if at == self.position => Ok(at),
            _ => Err(std::io::Error::other("forward V4 output requested a seek")),
        }
    }
}
