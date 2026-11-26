//! Positioned stream I/O driver, replay checksums, and V3/V4 bounded streaming writers.

use super::*;

/// Where the streamed writer puts the unit table (SPEC 2.7).
#[cfg(feature = "std")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamIndex {
    /// Head record 12 reserved at this many bytes per span and filled in place
    /// after each band (the index stays at the head); a span too wide for it
    /// turns the reservation into check tail plus a trailing `uIDX`.
    Reserved(u8),
    /// Head record 12 says check tail; the spans follow in a trailing `uIDX`.
    Trailing,
    V4 {
        directory: crate::v4::DirectoryPlacement,
        capacity: u64,
    },
}

/// A stream a bounded encode writes or reads by offset.
#[cfg(feature = "std")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stream {
    /// The encoded file; offset 0 is its first byte. Written in order, except
    /// the unit-table reservation, which is filled after the last band.
    Output,
    /// Index scratch: the spooled nested records, then one varint per main
    /// record. Appended, and read back in pass 2 and at the end.
    Index,
    /// Checksum scratch: eight bytes per checksum piece. Appended, and read
    /// back once at the end.
    Checksum,
}

/// The I/O of one bounded encode: source rows, and positioned writes and
/// reads of the three [`Stream`]s. Async, so the same driver runs blocking
/// (every call completes at once) or as a sans-IO session (a call waits for
/// the caller).
#[cfg(feature = "std")]
pub(crate) trait StreamIo {
    fn stop(&self) -> &dyn how_far::Stop {
        &how_far::NoPulse
    }
    /// Source rows from `y0` into `out` (its whole length, rows of
    /// `row_bytes`), for `pass` (0 exact prices, 4 superblock prices,
    /// 1 statistics, 10 + k trellis rate model k, 3 nested images,
    /// 2 coding). `out` may be replaced by another buffer of its length.
    async fn source(
        &mut self,
        pass: u8,
        y0: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), crate::stream_encode::StreamEncodeError>;
    async fn write(
        &mut self,
        stream: Stream,
        at: u64,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError>;
    async fn read(
        &mut self,
        stream: Stream,
        at: u64,
        buf: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError>;
}

/// An append-only range of a scratch stream from `start`, with a running
/// hash so a replay can tell whether the scratch changed underneath
/// (`stream_encode::spool::Spool` by offset).
#[cfg(feature = "std")]
pub(super) struct Range {
    pub(super) stream: Stream,
    pub(super) start: u64,
    pub(super) len: u64,
    pub(super) limit: u64,
    pub(super) hash: zenhash::Xxh3,
    pub(super) changed: &'static str,
}

#[cfg(feature = "std")]
impl Range {
    pub(super) fn new(
        stream: Stream,
        start: u64,
        limit: u64,
        changed: &'static str,
    ) -> Result<Self, Error> {
        start.checked_add(limit).ok_or(Error::TooLarge)?;
        Ok(Self {
            stream,
            start,
            len: 0,
            limit,
            hash: zenhash::Xxh3::new(),
            changed,
        })
    }

    pub(super) async fn append<'a, X: StreamIo>(
        &'a mut self,
        x: &'a mut X,
        bytes: &'a [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let len = self
            .len
            .checked_add(bytes.len() as u64)
            .filter(|&n| n <= self.limit)
            .ok_or(Error::TooLarge)?;
        x.write(self.stream, self.start + self.len, bytes).await?;
        self.hash.update(bytes);
        self.len = len;
        Ok(())
    }

    /// Reads bytes the stream held before this range began.
    pub(super) async fn read_before<X: StreamIo>(
        &self,
        x: &mut X,
        at: u64,
        buf: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        if at
            .checked_add(buf.len() as u64)
            .is_none_or(|end| end > self.start)
        {
            return Err(Error::InvalidInput(self.changed).into());
        }
        x.read(self.stream, at, buf).await
    }

    /// A checked replay, consumed to `None` to verify the entire range. Callers
    /// process each chunk before asking for the next, and propagate any error.
    pub(super) fn replay(&self) -> CheckedReplay<'_> {
        CheckedReplay {
            range: self,
            buffer: [0u8; crate::stream_encode::spool::REPLAY_BYTES],
            done: 0,
            hash: zenhash::Xxh3::new(),
            failed: false,
        }
    }
}

/// Keeps the original replay buffer and hash, without an async callback whose
/// higher-ranked borrowed future prevents proving the native session is Send.
/// Each caller scopes this cursor to the same interval as the former replay.
#[cfg(feature = "std")]
pub(super) struct CheckedReplay<'a> {
    pub(super) range: &'a Range,
    pub(super) buffer: [u8; crate::stream_encode::spool::REPLAY_BYTES],
    pub(super) done: u64,
    pub(super) hash: zenhash::Xxh3,
    pub(super) failed: bool,
}

#[cfg(feature = "std")]
impl CheckedReplay<'_> {
    /// Replay is provisional until a call returns verified `None`.
    /// Verification follows consumption of the last chunk, preserving the old
    /// partial-write semantics. A read error or interrupted read cannot retry.
    pub(super) async fn next<X: StreamIo>(
        &mut self,
        x: &mut X,
    ) -> Result<Option<&[u8]>, crate::stream_encode::StreamEncodeError> {
        if self.failed {
            return Err(Error::Corrupt("bounded replay failed").into());
        }
        self.failed = true;
        if self.done == self.range.len {
            if self.hash.digest() != self.range.hash.digest() {
                return Err(Error::InvalidInput(self.range.changed).into());
            }
            self.failed = false;
            return Ok(None);
        }
        let count = (self.range.len - self.done).min(self.buffer.len() as u64) as usize;
        x.read(
            self.range.stream,
            self.range.start + self.done,
            &mut self.buffer[..count],
        )
        .await?;
        self.hash.update(&self.buffer[..count]);
        self.done += count as u64;
        self.failed = false;
        Ok(Some(&self.buffer[..count]))
    }
}

/// Piece size of streamed file checksums: 1 MiB, the whole-image writers'
/// piece for files over 16 MiB (`checksum::piece_log2_for`); the length is
/// not known when the header is written.
#[cfg(feature = "std")]
pub(crate) const STREAM_PIECE_LOG2: u8 = 20;

/// A `cSUM` table (SPEC 1.4) of the bytes fed in order, XXH3-128 in `2^log2`
/// pieces, the digests spooled to the checksum stream. The bytes before
/// `hold` (rounded up to a piece) are kept and hashed last, so a writer may
/// patch them ([`Self::patch`]) after writing.
#[cfg(feature = "std")]
pub(super) struct StreamSum {
    pub(super) log2: u8,
    pub(super) algo: crate::HashAlgorithm,
    pub(super) hold: u64,
    pub(super) held: Vec<u8>,
    pub(super) hash: crate::Hasher,
    pub(super) piece: u64,
    pub(super) covered: u64,
    pub(super) digests: Range,
}

#[cfg(feature = "std")]
impl StreamSum {
    pub(super) fn new(
        start: u64,
        log2: u8,
        algo: crate::HashAlgorithm,
        hold: u64,
        limit: u64,
    ) -> Result<Self, Error> {
        Ok(Self {
            log2,
            algo,
            hold: hold.div_ceil(1 << log2) << log2,
            held: Vec::new(),
            hash: crate::Hasher::new(algo),
            piece: 0,
            covered: 0,
            digests: Range::new(
                Stream::Checksum,
                start,
                limit,
                "bounded lossy checksum scratch changed",
            )?,
        })
    }

    pub(super) async fn feed<X: StreamIo>(
        &mut self,
        x: &mut X,
        mut bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        self.covered = self
            .covered
            .checked_add(bytes.len() as u64)
            .ok_or(Error::TooLarge)?;
        let room = self.hold - self.held.len() as u64;
        if room != 0 {
            let n = room.min(bytes.len() as u64) as usize;
            self.held.try_reserve(n).map_err(|_| Error::TooLarge)?;
            self.held.extend_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
        }
        let size = 1u64 << self.log2;
        while !bytes.is_empty() {
            let n = (size - self.piece).min(bytes.len() as u64) as usize;
            self.hash.update(&bytes[..n]);
            self.piece += n as u64;
            bytes = &bytes[n..];
            if self.piece == size {
                self.end_piece(x).await?;
            }
        }
        Ok(())
    }

    pub(super) async fn end_piece<X: StreamIo>(
        &mut self,
        x: &mut X,
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        if self.piece != 0 {
            let d = self.hash.finish();
            self.digests.append(x, d.as_bytes()).await?;
            self.hash.reset();
            self.piece = 0;
        }
        Ok(())
    }

    /// Replaces held bytes at `at` (from the first byte fed).
    pub(super) fn patch(&mut self, at: u64, bytes: &[u8]) {
        self.held[at as usize..][..bytes.len()].copy_from_slice(bytes);
    }

    /// Writes the whole `cSUM` chunk at `out[at..]`; returns its length.
    pub(super) async fn write_chunk<X: StreamIo>(
        mut self,
        x: &mut X,
        at: u64,
    ) -> Result<u64, crate::stream_encode::StreamEncodeError> {
        self.end_piece(x).await?;
        let mut first = Vec::new();
        for piece in self.held.chunks(1 << self.log2) {
            first.extend_from_slice(crate::checksum::digest_of(self.algo, [piece]).as_bytes());
        }
        let payload = (first.len() as u64)
            .checked_add(self.digests.len)
            .and_then(|n| n.checked_add(4 + crate::varint_len(self.covered) as u64))
            .ok_or(Error::TooLarge)?;
        let mut head = crate::checksum::CSUM.to_vec();
        put_varint(&mut head, payload);
        head.extend_from_slice(&[self.algo.wire(), self.algo.width() as u8, self.log2]);
        put_varint(&mut head, self.covered);
        head.extend_from_slice(&first);
        x.write(Stream::Output, at, &head).await?;
        let mut pos = at + head.len() as u64;
        {
            let mut replay = self.digests.replay();
            while let Some(bytes) = replay.next(x).await? {
                x.write(Stream::Output, pos, bytes).await?;
                pos += bytes.len() as u64;
            }
        }
        Ok(pos - at)
    }
}

/// [`PassIo`] for the streamed driver: the passes' rows from the source, the
/// nested records spooled to the index stream, pass 2's records written to
/// the output (and fed to the checksum) with their spans spooled after the
/// nested records.
#[cfg(feature = "std")]
pub(super) struct Drive<'a, X> {
    pub(super) x: &'a mut X,
    pub(super) p: &'a Plan,
    /// The first stream error; the passes see `Error::Cancelled`.
    pub(super) failure: Option<crate::stream_encode::StreamEncodeError>,
    pub(super) spool: Range,
    pub(super) exact_at: Vec<(u64, usize, crate::Digest)>,
    pub(super) dc_at: Vec<(u64, usize, crate::Digest)>,
    /// Pass 2: the index entries, the checksum, the output position, and
    /// the `DATA` length so far.
    pub(super) index: Option<Range>,
    pub(super) sum: Option<StreamSum>,
    pub(super) end: u64,
    pub(super) data_len: u64,
    pub(super) v4: Option<super::v4::encode::Encoder>,
    pub(super) v4_directory: crate::v4::DirectoryPlacement,
}

#[cfg(feature = "std")]
impl<X: StreamIo> Drive<'_, X> {
    pub(super) fn fail(&mut self, e: crate::stream_encode::StreamEncodeError) -> Error {
        // A codec error from the source (a cancellation, an out-of-range
        // sample) is the passes' own error; the rest is reported after.
        match e {
            crate::stream_encode::StreamEncodeError::Codec(e) => e,
            e => {
                self.failure = Some(e);
                Error::Cancelled
            }
        }
    }

    pub(super) async fn drain_v4(&mut self) -> Result<(), crate::stream_encode::StreamEncodeError> {
        let (data, entries) = self
            .v4
            .as_mut()
            .ok_or(Error::Corrupt("missing bounded V4 writer"))?
            .take_output();
        if self.v4_directory != crate::v4::DirectoryPlacement::None {
            self.index
                .as_mut()
                .ok_or(Error::Corrupt("bounded V4 index order"))?
                .append(self.x, &entries)
                .await?;
        }
        self.put(&data).await?;
        self.data_len = self
            .data_len
            .checked_add(data.len() as u64)
            .ok_or(Error::TooLarge)?;
        self.v4.as_mut().unwrap().restore_output(data, entries);
        Ok(())
    }

    pub(super) async fn start_v4(
        &mut self,
        mut writer: super::v4::encode::Encoder,
        capacity: u64,
    ) -> Result<Option<u64>, crate::stream_encode::StreamEncodeError> {
        use crate::v4::DirectoryPlacement;
        let immutable = writer.prefix_len();
        let (data, entries) = writer.take_output();
        if data.get(immutable) != Some(&1) {
            return Err(Error::Corrupt("bounded V4 prefix").into());
        }
        let start = self.end;
        self.put(&data[..immutable]).await?;
        let mut prefix = Vec::new();
        put_varint(&mut prefix, self.v4_directory.wire_value());
        if self.v4_directory == DirectoryPlacement::Front {
            put_varint(&mut prefix, capacity);
        }
        self.put(&prefix).await?;
        let reservation = if self.v4_directory == DirectoryPlacement::Front {
            let at = self.end;
            let zeros = [0; 8192];
            let mut left = capacity;
            while left != 0 {
                let n = left.min(zeros.len() as u64) as usize;
                self.put(&zeros[..n]).await?;
                left -= n as u64;
            }
            Some(at)
        } else {
            None
        };
        self.put(&data[immutable + 1..]).await?;
        if self.v4_directory != DirectoryPlacement::None {
            self.index
                .as_mut()
                .ok_or(Error::Corrupt("bounded V4 index order"))?
                .append(self.x, &entries)
                .await?;
        }
        self.data_len = self.end.checked_sub(start).ok_or(Error::TooLarge)?;
        writer.restore_output(data, entries);
        self.v4 = Some(writer);
        Ok(reservation)
    }

    /// Writes `bytes` at the output position, fed to the checksum first.
    pub(super) async fn put(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        if let Some(s) = &mut self.sum {
            s.feed(self.x, bytes).await?;
        }
        let next = self
            .end
            .checked_add(bytes.len() as u64)
            .ok_or(Error::TooLarge)?;
        self.x.write(Stream::Output, self.end, bytes).await?;
        self.end = next;
        Ok(())
    }
}

#[cfg(feature = "std")]
impl<X: StreamIo> PassIo for Drive<'_, X> {
    fn stop(&self) -> &dyn how_far::Stop {
        self.x.stop()
    }
    async fn rows(
        &mut self,
        pass: u8,
        by0: usize,
        by1: usize,
        band: &mut Vec<u8>,
    ) -> Result<usize, Error> {
        let p = self.p;
        let (y0, y1) = p.band_rows(by0, by1);
        let n = (y1 - y0).checked_mul(p.rb).ok_or(Error::TooLarge)?;
        band.clear();
        band.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        band.resize(n, 0);
        if let Err(e) = self.x.source(pass, y0, band).await {
            return Err(self.fail(e));
        }
        if p.input.is_some() {
            let mut info = p.info.clone();
            info.height = (y1 - y0) as u32;
            precision::validate_pixels(&info, band, p.rb)?;
        }
        Ok(y0)
    }

    async fn store<'a>(&'a mut self, kind: Spooled, rec: &'a [u8]) -> Result<(), Error> {
        let entry = (
            self.spool.len,
            rec.len(),
            crate::checksum::digest_of(crate::HashAlgorithm::Xxh3_64, [rec]),
        );
        match kind {
            Spooled::Exact => self.exact_at.push(entry),
            Spooled::Dc => self.dc_at.push(entry),
        }
        if let Err(e) = self.spool.append(self.x, rec).await {
            return Err(self.fail(e));
        }
        Ok(())
    }

    async fn load(&mut self, kind: Spooled, t: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        let (at, len, digest) = match kind {
            Spooled::Exact => self.exact_at[t],
            Spooled::Dc => self.dc_at[t],
        };
        buf.clear();
        buf.resize(len, 0);
        let index = self
            .index
            .as_ref()
            .ok_or(Error::Corrupt("bounded load order"))?;
        if let Err(e) = index.read_before(self.x, self.spool.start + at, buf).await {
            return Err(self.fail(e));
        }
        if crate::checksum::digest_of(crate::HashAlgorithm::Xxh3_64, [&buf[..]]) != digest {
            return Err(Error::InvalidInput("bounded lossy exact scratch changed"));
        }
        Ok(())
    }

    async fn emit(&mut self, id: u64, payload: &[u8]) -> Result<(), Error> {
        if let Some(writer) = &mut self.v4 {
            writer.push(id, payload, None)?;
            return self.drain_v4().await.map_err(|e| self.fail(e));
        }

        use crate::parse::records::{framed_len, put_record};
        let mut framed = Vec::new();
        put_record(&mut framed, id, payload);
        let mut entry = Vec::new();
        put_varint(&mut entry, framed_len(id, payload.len()));
        let r = async {
            self.put(&framed).await?;
            let index = self
                .index
                .as_mut()
                .ok_or(Error::Corrupt("bounded emit order"))?;
            index.append(self.x, &entry).await
        }
        .await;
        match r {
            Ok(()) => {
                self.data_len += framed.len() as u64;
                Ok(())
            }
            Err(e) => Err(self.fail(e)),
        }
    }
    async fn emit_ac(&mut self, payload: &[u8], stripe: &StripeCoefs) -> Result<(), Error> {
        if let Some(writer) = &mut self.v4 {
            writer.push(crate::parse::records::R_AC, payload, Some(stripe))?;
            return self.drain_v4().await.map_err(|e| self.fail(e));
        }
        self.emit(crate::parse::records::R_AC, payload).await
    }
}

/// [`encode_lossy_bounded`] written as it is coded, over [`StreamIo`]: the
/// header and `DATA` head first (an open `DATA` chunk), then each band's
/// records. Spans go to the index stream (one varint per record, at most
/// `max_index_bytes` together with the spooled nested records, from
/// `index_start`); with [`StreamIndex::Reserved`] they are filled into the
/// head reservation after the last band. No `DATA` locator is written. A
/// configured file checksum is a streamed `cSUM` chunk in
/// [`STREAM_PIECE_LOG2`] pieces after the records (before a trailing
/// `uIDX`, SPEC 1.4): its digests spool to the checksum stream from
/// `checksum_start` (8 bytes per piece, at most `max_index_bytes`), and the
/// bytes up to the end of the reservation's piece stay in memory until the
/// reservation is filled. Returns the file length.
#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn encode_lossy_bounded_io<X: StreamIo>(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index: StreamIndex,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    encode_lossy_stream_io(
        info,
        meta,
        cfg,
        x,
        index,
        index_start,
        checksum_start,
        max_index_bytes,
        Preparation::Measured,
    )
    .await
}

#[cfg(feature = "std")]
#[derive(Clone, Copy)]
pub(super) enum Preparation {
    Measured,
    #[cfg(all(test, feature = "lab"))]
    StudyFirstBandDc,
    #[cfg(feature = "lab")]
    OnePass,
    #[cfg(feature = "lab")]
    Budgeted(one_pass::rate::Limits),
}

#[cfg(feature = "std")]
pub(super) fn validate_stream_plan(
    p: &Plan,
    cfg: &LossyConfig,
    index: StreamIndex,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
) -> Result<Option<usize>, crate::stream_encode::StreamEncodeError> {
    if cfg.checksum != checksum_start.is_some() {
        return Err(Error::InvalidInput(
            "checksum scratch is required exactly when the file checksum is on",
        )
        .into());
    }
    let nrec = p.record_count(false);
    let width = match index {
        StreamIndex::Reserved(w) if (1..=8).contains(&w) => Some(w as usize),
        StreamIndex::Reserved(_) => {
            return Err(Error::InvalidInput("unit table reservation width").into());
        }
        StreamIndex::Trailing | StreamIndex::V4 { .. } => None,
    };
    if (nrec as u64)
        .checked_mul(10)
        .is_none_or(|n| n > max_index_bytes)
    {
        return Err(Error::TooLarge.into());
    }
    Ok(width)
}

#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
pub(super) async fn encode_lossy_stream_io<X: StreamIo>(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index: StreamIndex,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
    preparation: Preparation,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    let p = plan(info, cfg)?;
    let p = match preparation {
        Preparation::Measured => p,
        #[cfg(all(test, feature = "lab"))]
        Preparation::StudyFirstBandDc => one_pass::configure(p, cfg)?,
        #[cfg(feature = "lab")]
        Preparation::OnePass | Preparation::Budgeted(_) => one_pass::configure(p, cfg)?,
    };
    encode_planned_stream_io(
        p,
        info,
        meta,
        cfg,
        x,
        index,
        index_start,
        checksum_start,
        max_index_bytes,
        preparation,
    )
    .await
}

#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
pub(super) async fn encode_planned_stream_io<X: StreamIo>(
    p: Plan,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index: StreamIndex,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
    preparation: Preparation,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    if let StreamIndex::V4 {
        directory,
        capacity,
    } = index
    {
        return encode_v4_stream_io(
            p,
            info,
            meta,
            cfg,
            x,
            index_start,
            checksum_start,
            max_index_bytes,
            directory,
            capacity,
        )
        .await;
    }
    use crate::parse::records::{
        H_UNIT_RESERVE, INDEX_FOOTER, RESERVE_FILLED, RESERVE_TAIL, UIDX, chunk_len_of, put_record,
    };
    let width = validate_stream_plan(&p, cfg, index, checksum_start, max_index_bytes)?;
    // Nested exact stripes and split DC records spool to the index stream
    // first (pass 1 codes them, pass 2 places them by index); the index
    // entries follow them. Both share `max_index_bytes`.
    let mut d = Drive {
        x,
        p: &p,
        failure: None,
        spool: Range::new(
            Stream::Index,
            index_start,
            max_index_bytes,
            "bounded lossy exact scratch changed",
        )?,
        exact_at: Vec::new(),
        dc_at: Vec::new(),
        index: None,
        sum: None,
        end: 0,
        data_len: 0,
        v4: None,
        v4_directory: crate::v4::DirectoryPlacement::Tail,
    };
    let r = match preparation {
        Preparation::Measured => pass1(&p, cfg, &mut d).await,
        #[cfg(all(test, feature = "lab"))]
        Preparation::StudyFirstBandDc => {
            let mut prepared = one_pass::prepare(&p, cfg, &mut d, false).await?;
            one_pass::train_first_band_dc(&p, &mut prepared)?;
            Ok(prepared)
        }
        #[cfg(feature = "lab")]
        Preparation::OnePass => one_pass::prepare(&p, cfg, &mut d, false).await,
        #[cfg(feature = "lab")]
        Preparation::Budgeted(_) => one_pass::prepare(&p, cfg, &mut d, true).await,
    };
    if let Some(e) = d.failure.take() {
        return Err(e);
    }
    let mut prep = r?;
    let nrec = p.record_count(prep.exact.is_some());
    let left = max_index_bytes
        .checked_sub(d.spool.len)
        .ok_or(Error::TooLarge)?;
    if (nrec as u64).checked_mul(10).is_none_or(|n| n > left) {
        return Err(Error::TooLarge.into());
    }
    d.index = Some(Range::new(
        Stream::Index,
        d.spool.start + d.spool.len,
        left,
        "bounded lossy index scratch changed",
    )?);

    // Head records in increasing id order: exact blocks, DC32, then the unit
    // table (record 12), whose payload offset the reservation fill needs.
    let mut records = Vec::new();
    if prep.exact.is_some() {
        put_record(&mut records, H_EXACT, &[]);
    }
    #[cfg(feature = "lab")]
    if matches!(preparation, Preparation::Budgeted(_)) {
        put_record(&mut records, H_AQ, &[]);
    }
    if !prep.fits16 {
        put_record(&mut records, H_DC32, &[]);
    }
    let reserve_len = match width {
        Some(w) => nrec
            .checked_mul(w)
            .and_then(|n| n.checked_add(2))
            .ok_or(Error::TooLarge)?,
        None => 1,
    };
    let mut reserve = Vec::new();
    reserve
        .try_reserve_exact(reserve_len)
        .map_err(|_| Error::TooLarge)?;
    reserve.resize(reserve_len, 0);
    reserve[0] = RESERVE_TAIL;
    put_record(&mut records, H_UNIT_RESERVE, &reserve);
    drop(reserve);
    let reserve_in_records = records.len() - reserve_len;
    match if cfg.coding_defaults {
        None
    } else {
        prep.merges.as_ref().map(|m| m.1)
    } {
        Some(m) if m.joint() => put_record(&mut records, small::H_PARTITIONS, &[]),
        Some(m) if m.merges_only() => put_record(&mut records, superblock::H_SUPERBLOCKS, &[]),
        _ => {}
    }
    if p.layouts != [LossyChannelLayout::default(); 3] {
        put_record(&mut records, H_LAYOUTS, &p.layouts.map(|l| l.byte()));
    }
    if cfg.local_ans_contexts {
        put_record(&mut records, local_ans::HEAD, &[]);
    }
    if cfg.coding_defaults {
        put_record(&mut records, coding_defaults::HEAD, &[1, 0, 0]);
    }
    if cfg.model_references {
        put_record(&mut records, model_refs::HEAD, &[1, 0]);
    }
    let (head, records_at) = data_head(&p, &prep, &records);
    drop(records);
    let mut header = file_header(info, meta, cfg, &p, &prep)?;
    if cfg.checksum {
        header = crate::container::with_extension(
            header,
            crate::checksum::EXT_CHECKSUM,
            Some(&[STREAM_PIECE_LOG2, cfg.hash.wire()]),
        )?;
    }
    let data_rel = header.len() as u64 + 5;
    let reserve_at = data_rel + (records_at + reserve_in_records) as u64;
    if let Some(start) = checksum_start {
        d.sum = Some(StreamSum::new(
            start,
            STREAM_PIECE_LOG2,
            cfg.hash,
            reserve_at + reserve_len as u64,
            max_index_bytes,
        )?);
    }
    d.put(&header).await?;
    d.put(b"DATA\0").await?;
    d.put(&head).await?;
    d.data_len = head.len() as u64;
    drop(head);

    // Pass 2, written through; each record's span spooled.
    let r = match preparation {
        #[cfg(feature = "lab")]
        Preparation::Budgeted(limits) => {
            one_pass::rate::pass2(&p, cfg, &mut prep, &mut d, limits).await
        }
        _ => pass2(&p, cfg, &mut prep, &mut d).await,
    };
    if let Some(e) = d.failure.take() {
        return Err(e);
    }
    r?;
    let Drive {
        x,
        index,
        mut sum,
        end,
        data_len,
        ..
    } = d;
    let index = index.ok_or(Error::Corrupt("bounded index"))?;

    // The delayed update: fill the head reservation from the spooled spans,
    // in 64 KiB writes, unless a span is too wide for it.
    if let Some(w) = width {
        let bits = 8 * w as u32;
        let mut overflow = false;
        let mut entries: Vec<u8> = Vec::new();
        let mut pending: Vec<u8> = Vec::new();
        let mut written = 0u64;
        {
            let mut replay = index.replay();
            while let Some(chunk) = replay.next(x).await? {
                pending.extend_from_slice(chunk);
                let mut r = ByteReader::new(&pending);
                let mut used = 0;
                // A varint cut by the chunk boundary stays pending.
                while let Ok(e) = r.varint() {
                    used = r.pos;
                    overflow |= bits < 64 && e >> bits != 0;
                    if !overflow {
                        entries.extend_from_slice(&e.to_le_bytes()[..w]);
                    }
                }
                pending.drain(..used);
                if !overflow && entries.len() >= 1 << 16 {
                    if let Some(s) = &mut sum {
                        s.patch(reserve_at + 2 + written, &entries);
                    }
                    x.write(Stream::Output, reserve_at + 2 + written, &entries)
                        .await?;
                    written += entries.len() as u64;
                    entries.clear();
                }
            }
        }
        if !pending.is_empty() {
            return Err(Error::Corrupt("bounded lossy index scratch").into());
        }
        if !overflow {
            if let Some(s) = &mut sum {
                s.patch(reserve_at + 2 + written, &entries);
                s.patch(reserve_at, &[RESERVE_FILLED, w as u8]);
            }
            x.write(Stream::Output, reserve_at + 2 + written, &entries)
                .await?;
            written += entries.len() as u64;
            if written != (nrec * w) as u64 {
                return Err(Error::Corrupt("bounded lossy unit table count").into());
            }
            x.write(Stream::Output, reserve_at, &[RESERVE_FILLED, w as u8])
                .await?;
            let n = match sum {
                Some(s) => s.write_chunk(x, end).await?,
                None => 0,
            };
            return Ok(end + n);
        }
        // Too wide: any entries already written are inside a reservation that
        // still says check tail (its first byte); the trailing index decides.
    }
    // Trailing `uIDX` (SPEC 2.7): the spooled entries, then the footer. A
    // checksum covers it (one replay) and goes just before it.
    let n = index.len + INDEX_FOOTER as u64;
    let mut prefix = UIDX.to_vec();
    put_varint(&mut prefix, n + 1);
    let mut footer = data_len.to_le_bytes().to_vec();
    footer.extend_from_slice(&chunk_len_of(n).to_le_bytes());
    footer.extend_from_slice(&UIDX);
    let mut at = end;
    if let Some(mut s) = sum {
        s.feed(x, &prefix).await?;
        {
            let mut replay = index.replay();
            while let Some(chunk) = replay.next(x).await? {
                s.feed(x, chunk).await?;
            }
        }
        s.feed(x, &footer).await?;
        at += s.write_chunk(x, at).await?;
    }
    x.write(Stream::Output, at, &prefix).await?;
    at += prefix.len() as u64;
    {
        let mut replay = index.replay();
        while let Some(chunk) = replay.next(x).await? {
            x.write(Stream::Output, at, chunk).await?;
            at += chunk.len() as u64;
        }
    }
    x.write(Stream::Output, at, &footer).await?;
    at += footer.len() as u64;
    Ok(at)
}

/// [`StreamIo`] over blocking streams: the source rows callback, a
/// `Write + Seek` output and `Read + Write + Seek` scratch, each offset
/// relative to where the stream stood when the encode began. A stream is
/// seeked only when an access does not continue the previous one, so a
/// buffered output is not flushed for every record.
#[cfg(feature = "std")]
pub(super) struct Blocking<'a, W, S, C> {
    pub(super) stop: &'a dyn how_far::Stop,
    pub(super) rows: &'a mut dyn FnMut(u8, usize, &mut [u8]) -> Result<(), Error>,
    pub(super) out: &'a mut W,
    pub(super) out_base: u64,
    pub(super) out_at: u64,
    pub(super) scratch: S,
    pub(super) scratch_at: Option<u64>,
    pub(super) sums: Option<C>,
    pub(super) sums_at: Option<u64>,
}

#[cfg(feature = "std")]
pub(super) fn seek_to<T: std::io::Seek>(
    s: &mut T,
    at: &mut Option<u64>,
    to: u64,
) -> Result<(), crate::stream_encode::StreamEncodeError> {
    if *at != Some(to) {
        s.seek(std::io::SeekFrom::Start(to))?;
    }
    Ok(())
}

#[cfg(feature = "std")]
impl<W, S, C> StreamIo for Blocking<'_, W, S, C>
where
    W: std::io::Write + std::io::Seek,
    S: std::io::Read + std::io::Write + std::io::Seek,
    C: std::io::Read + std::io::Write + std::io::Seek,
{
    fn stop(&self) -> &dyn how_far::Stop {
        self.stop
    }
    async fn source(
        &mut self,
        pass: u8,
        y0: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        self.stop.check().map_err(|_| Error::Cancelled)?;
        (self.rows)(pass, y0, out)?;
        self.stop.check().map_err(|_| Error::Cancelled)?;
        Ok(())
    }

    async fn write(
        &mut self,
        stream: Stream,
        at: u64,
        bytes: &[u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        self.stop.check().map_err(|_| Error::Cancelled)?;
        match stream {
            Stream::Output => {
                let to = self.out_base + at;
                if self.out_at != to {
                    self.out.seek(std::io::SeekFrom::Start(to))?;
                }
                self.out.write_all(bytes)?;
                self.out_at = to + bytes.len() as u64;
            }
            Stream::Index => {
                seek_to(&mut self.scratch, &mut self.scratch_at, at)?;
                self.scratch.write_all(bytes)?;
                self.scratch_at = Some(at + bytes.len() as u64);
            }
            Stream::Checksum => {
                let c = self
                    .sums
                    .as_mut()
                    .ok_or(Error::Corrupt("bounded checksum stream"))?;
                seek_to(c, &mut self.sums_at, at)?;
                c.write_all(bytes)?;
                self.sums_at = Some(at + bytes.len() as u64);
            }
        }
        self.stop.check().map_err(|_| Error::Cancelled)?;
        Ok(())
    }

    async fn read(
        &mut self,
        stream: Stream,
        at: u64,
        buf: &mut [u8],
    ) -> Result<(), crate::stream_encode::StreamEncodeError> {
        self.stop.check().map_err(|_| Error::Cancelled)?;
        use crate::stream_encode::StreamEncodeError;
        let (s, pos): (&mut dyn ReadSeekWrite, &mut Option<u64>) = match stream {
            Stream::Output => return Err(Error::Corrupt("bounded output read").into()),
            Stream::Index => (&mut self.scratch, &mut self.scratch_at),
            Stream::Checksum => (
                self.sums
                    .as_mut()
                    .ok_or(Error::Corrupt("bounded checksum stream"))?,
                &mut self.sums_at,
            ),
        };
        s.flush()?;
        if *pos != Some(at) {
            s.seek(std::io::SeekFrom::Start(at))?;
        }
        s.read_exact(buf).map_err(StreamEncodeError::Read)?;
        *pos = Some(at + buf.len() as u64);
        self.stop.check().map_err(|_| Error::Cancelled)?;
        Ok(())
    }
}

#[cfg(feature = "std")]
pub(super) trait ReadSeekWrite: std::io::Read + std::io::Write + std::io::Seek {}
#[cfg(feature = "std")]
impl<T: std::io::Read + std::io::Write + std::io::Seek> ReadSeekWrite for T {}

/// The existing measured band pipeline with V4 serialization and caller scratch.
#[cfg(feature = "std")]
pub(super) async fn encode_v4_stream_io<X: StreamIo>(
    p: Plan,
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    x: &mut X,
    index_start: u64,
    checksum_start: Option<u64>,
    max_index_bytes: u64,
    directory: crate::v4::DirectoryPlacement,
    capacity: u64,
) -> Result<u64, crate::stream_encode::StreamEncodeError> {
    use crate::v4::DirectoryPlacement;
    if directory == DirectoryPlacement::Front {
        if capacity == 0 {
            return Err(Error::InvalidInput("zero V4 front reservation").into());
        }
        if capacity > max_index_bytes {
            return Err(Error::TooLarge.into());
        }
    } else if capacity != 0 {
        return Err(Error::InvalidInput("V4 reservation without front placement").into());
    }
    validate_stream_plan(
        &p,
        cfg,
        StreamIndex::Trailing,
        checksum_start,
        max_index_bytes,
    )?;
    let mut d = Drive {
        x,
        p: &p,
        failure: None,
        spool: Range::new(
            Stream::Index,
            index_start,
            max_index_bytes,
            "bounded lossy exact scratch changed",
        )?,
        exact_at: Vec::new(),
        dc_at: Vec::new(),
        index: None,
        sum: None,
        end: 0,
        data_len: 0,
        v4: None,
        v4_directory: crate::v4::DirectoryPlacement::Tail,
    };
    let measured = pass1(&p, cfg, &mut d).await;
    if let Some(error) = d.failure.take() {
        return Err(error);
    }
    let mut prep = measured?;
    let left = max_index_bytes
        .checked_sub(d.spool.len)
        .ok_or(Error::TooLarge)?;
    d.index = Some(Range::new(
        Stream::Index,
        d.spool.start + d.spool.len,
        left,
        "bounded V4 directory scratch changed",
    )?);
    d.v4_directory = directory;
    let exact = if let Some(exact) = &prep.exact {
        let mut r = ByteReader::new(&exact.head);
        let blocks_per_row = r.var_usize()?;
        Some(super::v4::encode::ExactInput {
            blocks_per_row,
            counts: &exact.counts,
            model: &exact.head[exact.model.clone()],
            lengths: &exact.head[exact.model.end..],
            stripes: exact.stripes,
        })
    } else {
        None
    };
    let maps = prep.merges.as_ref().map(|m| m.1).unwrap_or_default();
    let writer = super::v4::encode::Encoder::new(super::v4::encode::Input {
        info,
        color: p.color,
        rows: p.sb as usize,
        columns: p.gcols,
        params: envelope::Params {
            scale: p.scale,
            qp: p.qp,
            entropy: prep.entropy,
            compact: false,
            seed: crate::seeds::Selection::NONE,
            records: false,
        },
        layouts: p.layouts,
        dc_rows: p.dc_rows,
        ac_head: &prep.tb,
        dc_head: &prep.dc_head,
        alpha_head: prep.alpha.as_ref().map(|(_, head)| head.as_slice()),
        exact,
        control: if prep.exact.is_some() { 2 } else { 0 },
        dc32: !prep.fits16,
        scoped: cfg.coding_defaults,
        local_ans: cfg.local_ans_contexts,
        references: cfg.model_references,
        partitioned: maps.merge || maps.split,
        retained: &[],
        max_bytes: usize::MAX,
    })?;
    let legacy_header = file_header(info, meta, cfg, &p, &prep)?;
    let hp = crate::parse::header::read_header_sized(&mut ByteReader::new(&legacy_header))?;
    let features = crate::Features::from_bits(hp.declared.bits() & !(0x3f << 26))
        .ok_or(Error::Corrupt("bounded V4 features"))?;
    let mut header = Vec::new();
    crate::container::write_group_head_features(
        &mut header,
        info,
        meta,
        crate::Coding::Lossy,
        Some(p.color),
        cfg.decode_speed(),
        features,
        4,
    )?;
    if cfg.checksum {
        header = crate::container::with_extension(
            header,
            crate::checksum::EXT_CHECKSUM,
            Some(&[STREAM_PIECE_LOG2, cfg.hash.wire()]),
        )?;
        let hold = if directory == DirectoryPlacement::Front {
            (header.len() as u64)
                .checked_add(5)
                .and_then(|n| n.checked_add(writer.prefix_len() as u64))
                .and_then(|n| n.checked_add(1 + crate::varint_len(capacity) as u64))
                .and_then(|n| n.checked_add(capacity))
                .ok_or(Error::TooLarge)?
        } else {
            0
        };
        d.sum = Some(StreamSum::new(
            checksum_start.ok_or(Error::InvalidInput("missing checksum scratch"))?,
            STREAM_PIECE_LOG2,
            cfg.hash,
            hold,
            max_index_bytes,
        )?);
    }
    d.put(&header).await?;
    d.put(b"DATA\0").await?;
    let reservation = d.start_v4(writer, capacity).await?;
    let encoded = pass2(&p, cfg, &mut prep, &mut d).await;
    if let Some(error) = d.failure.take() {
        return Err(error);
    }
    encoded?;
    let (count, objects_len) = d.v4.as_mut().unwrap().finish()?;
    d.drain_v4().await?;
    let Drive {
        x,
        index,
        mut sum,
        mut end,
        ..
    } = d;
    let index = index.ok_or(Error::Corrupt("missing bounded V4 directory"))?;
    if directory != DirectoryPlacement::Tail {
        if directory == DirectoryPlacement::Front {
            let body_len = (crate::varint_len(count) as u64)
                .checked_add(index.len)
                .and_then(|n| n.checked_add(8))
                .ok_or(Error::TooLarge)?;
            if body_len
                .checked_add(crate::varint_len(body_len) as u64)
                .is_none_or(|n| n > capacity)
            {
                return Err(Error::TooLarge.into());
            }
            let mut at = reservation.ok_or(Error::Corrupt("missing V4 front reservation"))?;
            let mut prefix = Vec::new();
            put_varint(&mut prefix, body_len);
            put_varint(&mut prefix, count);
            if let Some(checksum) = &mut sum {
                checksum.patch(at, &prefix);
            }
            x.write(Stream::Output, at, &prefix).await?;
            at = at.checked_add(prefix.len() as u64).ok_or(Error::TooLarge)?;
            let mut replay = index.replay();
            while let Some(bytes) = replay.next(x).await? {
                if let Some(checksum) = &mut sum {
                    checksum.patch(at, bytes);
                }
                x.write(Stream::Output, at, bytes).await?;
                at = at.checked_add(bytes.len() as u64).ok_or(Error::TooLarge)?;
            }
            let suffix = objects_len.to_le_bytes();
            if let Some(checksum) = &mut sum {
                checksum.patch(at, &suffix);
            }
            x.write(Stream::Output, at, &suffix).await?;
        }
        if let Some(checksum) = sum {
            end = end
                .checked_add(checksum.write_chunk(x, end).await?)
                .ok_or(Error::TooLarge)?;
        }
        return Ok(end);
    }
    let payload_len = (crate::varint_len(count) as u64)
        .checked_add(index.len)
        .and_then(|n| n.checked_add(20))
        .ok_or(Error::TooLarge)?;
    let length = payload_len.checked_add(1).ok_or(Error::TooLarge)?;
    let chunk_len = (4 + crate::varint_len(length) as u64)
        .checked_add(payload_len)
        .ok_or(Error::TooLarge)?;
    let mut head = b"uIDX".to_vec();
    put_varint(&mut head, length);
    put_varint(&mut head, count);
    let mut suffix = objects_len.to_le_bytes().to_vec();
    suffix.extend_from_slice(&chunk_len.to_le_bytes());
    suffix.extend_from_slice(b"uIDX");
    if let Some(checksum) = &mut sum {
        checksum.feed(x, &head).await?;
        let mut replay = index.replay();
        while let Some(bytes) = replay.next(x).await? {
            checksum.feed(x, bytes).await?;
        }
        checksum.feed(x, &suffix).await?;
    }
    if let Some(checksum) = sum {
        end += checksum.write_chunk(x, end).await?;
    }
    x.write(Stream::Output, end, &head).await?;
    end += head.len() as u64;
    let mut replay = index.replay();
    while let Some(bytes) = replay.next(x).await? {
        x.write(Stream::Output, end, bytes).await?;
        end = end.checked_add(bytes.len() as u64).ok_or(Error::TooLarge)?;
    }
    x.write(Stream::Output, end, &suffix).await?;
    end.checked_add(suffix.len() as u64)
        .ok_or(Error::TooLarge.into())
}

/// [`encode_lossy_bounded_io`] over blocking streams: rows from `rows`,
/// the file to `out` from its current position, the index to `scratch` and
/// checksum digests to `checksum_scratch`, each appended after what they
/// hold. Returns both scratch objects.
#[cfg(feature = "std")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn encode_lossy_bounded_to<W, S, C>(
    info: &ImageInfo,
    meta: &Metadata,
    cfg: &LossyConfig,
    rows: &mut dyn FnMut(u8, usize, &mut [u8]) -> Result<(), Error>,
    out: &mut W,
    index: StreamIndex,
    mut scratch: S,
    mut checksum_scratch: Option<C>,
    max_index_bytes: u64,
    stop: &dyn how_far::Stop,
) -> Result<(S, Option<C>), crate::stream_encode::StreamEncodeError>
where
    W: std::io::Write + std::io::Seek,
    S: std::io::Read + std::io::Write + std::io::Seek,
    C: std::io::Read + std::io::Write + std::io::Seek,
{
    use std::io::SeekFrom;
    stop.check().map_err(|_| Error::Cancelled)?;
    let index_start = scratch.seek(SeekFrom::End(0))?;
    let checksum_start = match &mut checksum_scratch {
        Some(c) => Some(c.seek(SeekFrom::End(0))?),
        None => None,
    };
    let out_base = out.stream_position()?;
    let mut b = Blocking {
        stop,
        rows,
        out,
        out_base,
        out_at: out_base,
        scratch,
        scratch_at: Some(index_start),
        sums_at: checksum_start,
        sums: checksum_scratch,
    };
    let end = crate::parse::payload::ready(encode_lossy_bounded_io(
        info,
        meta,
        cfg,
        &mut b,
        index,
        index_start,
        checksum_start,
        max_index_bytes,
    ))?;
    let to = b.out_base + end;
    if b.out_at != to {
        b.out.seek(SeekFrom::Start(to))?;
    }
    b.out.flush()?;
    Ok((b.scratch, b.sums))
}
