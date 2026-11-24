//! Adapt existing incremental numeric sessions; never collect a Gray file.
use super::*;
use crate::stream_encode::{
    BoundedLossySession, BoundedLossyStep, BoundedStream, GroupEncodeConfig,
    NativeGroupEncodeSession as GroupEncodeSession,
};
use crate::{ImageInfo, Metadata, SampleType};
use alloc::boxed::Box;
use std::io::SeekFrom;

enum Driver {
    Lossless(Box<GroupEncodeSession>),
    Lossy(BoundedLossySession),
}

pub(super) struct Producer {
    driver: Driver,
    info: ImageInfo,
    pub records: u64,
    pub coding: u8,
    pub features: Features,
    pub speed: crate::DecodeSpeed,
    pub head: Vec<u8>,
    record: Vec<u8>,
    input: Vec<u8>,
    limit: usize,
    scratch_limit: u64,
    scratch_start: u64,
    output_at: u64,
    emitted: u64,
    requested_y: u32,
}

fn copy(bytes: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<(), Error> {
    if bytes.len() > limit {
        return Err(Error::TooLarge);
    }
    out.clear();
    out.try_reserve_exact(bytes.len())
        .map_err(|_| Error::TooLarge)?;
    if out.capacity() > limit {
        return Err(Error::TooLarge);
    }
    out.extend_from_slice(bytes);
    Ok(())
}

fn fill(
    info: &ImageInfo,
    role: u8,
    source: &mut impl ComponentSource,
    rect: GroupRect,
    output: &mut [u8],
    stop: &dyn how_far::Stop,
) -> Result<(), StreamEncodeError> {
    stop.check().map_err(|_| Error::Cancelled)?;
    let plane = source.group(role, rect)?;
    if plane.width() != rect.width as usize || plane.height() != rect.height as usize {
        return Err(Error::InvalidInput("component source rectangle shape").into());
    }
    let width = rect.width as usize;
    let max = (1u32
        << info
            .channel_bits(0)
            .ok_or(Error::InvalidInput("component code depth"))?)
        - 1;
    match plane {
        AnyPlane::U8(plane) if info.sample == SampleType::U8 => {
            for (y, out) in output.chunks_exact_mut(width).enumerate() {
                stop.check().map_err(|_| Error::Cancelled)?;
                let row = plane
                    .row(y)
                    .ok_or(Error::InvalidInput("component source row"))?;
                if row.iter().any(|&v| u32::from(v) > max) {
                    return Err(Error::InvalidInput(
                        "component source code outside declared depth",
                    )
                    .into());
                }
                out.copy_from_slice(row);
            }
        }
        AnyPlane::U16(plane) if info.sample == SampleType::U16 => {
            for (y, out) in output.chunks_exact_mut(width * 2).enumerate() {
                stop.check().map_err(|_| Error::Cancelled)?;
                for (&value, bytes) in plane
                    .row(y)
                    .ok_or(Error::InvalidInput("component source row"))?
                    .iter()
                    .zip(out.as_chunks_mut::<2>().0)
                {
                    if u32::from(value) > max {
                        return Err(Error::InvalidInput(
                            "component source code outside declared depth",
                        )
                        .into());
                    }
                    bytes.copy_from_slice(&value.to_ne_bytes());
                }
            }
        }
        _ => return Err(Error::InvalidInput("component source sample type").into()),
    }
    Ok(())
}

fn scratch<N: Read + Write + Seek>(
    io: &mut N,
    start: u64,
    limit: u64,
    offset: u64,
    bytes: &[u8],
) -> Result<(), StreamEncodeError> {
    if offset
        .checked_add(bytes.len() as u64)
        .is_none_or(|n| n > limit)
    {
        return Err(Error::TooLarge.into());
    }
    io.seek(SeekFrom::Start(
        start.checked_add(offset).ok_or(Error::TooLarge)?,
    ))?;
    io.write_all(bytes)?;
    Ok(())
}
fn scratch_read<N: Read + Write + Seek>(
    io: &mut N,
    start: u64,
    limit: u64,
    offset: u64,
    bytes: &mut [u8],
) -> Result<(), StreamEncodeError> {
    if offset
        .checked_add(bytes.len() as u64)
        .is_none_or(|n| n > limit)
    {
        return Err(Error::TooLarge.into());
    }
    io.flush()?;
    io.seek(SeekFrom::Start(
        start.checked_add(offset).ok_or(Error::TooLarge)?,
    ))
    .map_err(StreamEncodeError::Read)?;
    io.read_exact(bytes).map_err(StreamEncodeError::Read)
}

impl Producer {
    pub fn new(
        cfg: &ComponentEncodeConfig<'_>,
        role: usize,
        working: u64,
        scratch_limit: u64,
    ) -> Result<Self, StreamEncodeError> {
        let encoder = &cfg.encoders[role];
        if encoder.checksum() || encoder.unit_ordinals() {
            return Err(Error::Unsupported(
                "component numeric checksums/ordinals are outer bindings",
            )
            .into());
        }
        let (w, h) = cfg
            .layout
            .plane_size(role, cfg.width, cfg.height)
            .ok_or(Error::TooLarge)?;
        let PlaneEncoding::Code(encoding) = cfg.layout.planes()[role].encoding() else {
            unreachable!()
        };
        let mut info = ImageInfo::new(
            w,
            h,
            crate::Channels::Gray,
            if encoding.storage() == ChannelType::U8 {
                SampleType::U8
            } else {
                SampleType::U16
            },
        );
        info = info.with_significant_bits(&[encoding.code_bits()])?;
        info.cicp = crate::Cicp {
            color_primaries: 2,
            transfer_characteristics: 2,
            matrix_coefficients: 0,
            full_range: true,
        };
        let (driver, head, features, records) = match encoder.coder() {
            Coder::Lossy(_) => {
                if role == 3 || encoder.row_groups().is_some() {
                    return Err(Error::Unsupported(
                        "component alpha must be exact; lossy uses stripe controls",
                    )
                    .into());
                }
                let count = crate::lossy::bounded::component_one_pass_records(
                    &info,
                    &encoder.lossy_config(),
                )?;
                let session = BoundedLossySession::one_pass(
                    info.clone(),
                    &Metadata::default(),
                    encoder,
                    scratch_limit,
                    working,
                )?;
                (
                    Driver::Lossy(session),
                    Vec::new(),
                    Features::EMPTY,
                    count as u64,
                )
            }
            _ => {
                if !encoder.is_exact() {
                    return Err(Error::Unsupported(
                        "component group producer requires exact coding",
                    )
                    .into());
                }
                let mut groups = GroupEncodeConfig::required(encoder)?;
                if groups.rows == 0 || groups.columns == 0 {
                    return Err(
                        Error::InvalidInput("component group geometry must be nonzero").into(),
                    );
                }
                let input = (groups.rows.min(h) as u64)
                    .checked_mul(u64::from(groups.columns.min(w)))
                    .and_then(|n| n.checked_mul(info.sample.bytes() as u64))
                    .ok_or(Error::TooLarge)?;
                groups.max_working_bytes = groups.max_working_bytes.min(
                    usize::try_from(working.checked_sub(input).ok_or(Error::TooLarge)?)
                        .map_err(|_| Error::TooLarge)?,
                );
                let count = u64::from(w.div_ceil(groups.columns))
                    .checked_mul(u64::from(h.div_ceil(groups.rows)))
                    .ok_or(Error::TooLarge)?;
                let (session, head) = GroupEncodeSession::nested(info.clone(), groups)?;
                let features = crate::v1::head_features(&info, &head);
                (Driver::Lossless(Box::new(session)), head, features, count)
            }
        };
        Ok(Self {
            driver,
            info,
            records,
            coding: 0,
            features,
            speed: encoder
                .decode_speed()
                .unwrap_or(crate::DecodeSpeed::Balanced),
            head,
            record: Vec::new(),
            input: Vec::new(),
            limit: usize::try_from(working).map_err(|_| Error::TooLarge)?,
            scratch_limit,
            scratch_start: 0,
            output_at: 0,
            emitted: 0,
            requested_y: 0,
        })
    }

    pub fn prime<N: Read + Write + Seek>(
        &mut self,
        role: u8,
        source: &mut impl ComponentSource,
        io: &mut N,
        stop: &dyn how_far::Stop,
    ) -> Result<(), StreamEncodeError> {
        let Driver::Lossy(session) = &mut self.driver else {
            return Ok(());
        };
        self.scratch_start = io.seek(SeekFrom::End(0))?;
        let mut part = 0;
        loop {
            stop.check().map_err(|_| Error::Cancelled)?;
            match session.step()? {
                BoundedLossyStep::Rows { pass, y0, rows } => {
                    if pass != 2 || y0 != self.requested_y {
                        return Err(Error::Corrupt("component producer source order").into());
                    }
                    let height = rows.len() / self.info.row_bytes();
                    fill(
                        &self.info,
                        role,
                        source,
                        GroupRect {
                            x: 0,
                            y: y0,
                            width: self.info.width,
                            height: height as u32,
                        },
                        rows,
                        stop,
                    )?;
                    self.requested_y += height as u32;
                }
                BoundedLossyStep::Write {
                    stream: BoundedStream::Output,
                    offset,
                    bytes,
                } => {
                    if offset != self.output_at {
                        return Err(Error::Corrupt("component producer output order").into());
                    }
                    self.output_at = self
                        .output_at
                        .checked_add(bytes.len() as u64)
                        .ok_or(Error::TooLarge)?;
                    match part {
                        0 => {
                            let mut r = ByteReader::new(bytes);
                            let header = crate::parse::header::read_header(&mut r)?;
                            if r.remaining() != 0
                                || header.info != self.info
                                || header.coding != crate::Coding::Lossy
                            {
                                return Err(
                                    Error::Corrupt("component producer numeric header").into()
                                );
                            }
                            self.features = header.features;
                            self.speed = header.decode_speed;
                            self.coding = match header.lossy_color {
                                Some(crate::LossyColor::Xyb) => 1,
                                Some(crate::LossyColor::YCoCg) => 2,
                                _ => {
                                    return Err(
                                        Error::Unsupported("component numeric color").into()
                                    );
                                }
                            };
                        }
                        1 => {
                            if bytes != b"DATA\0" {
                                return Err(Error::Corrupt("component producer DATA").into());
                            }
                        }
                        2 => {
                            copy(bytes, &mut self.head, 65536)?;
                            return Ok(());
                        }
                        _ => unreachable!(),
                    }
                    part += 1;
                }
                BoundedLossyStep::Write {
                    stream: BoundedStream::IndexScratch,
                    offset,
                    bytes,
                } => scratch(io, self.scratch_start, self.scratch_limit, offset, bytes)?,
                BoundedLossyStep::Read {
                    stream: BoundedStream::IndexScratch,
                    offset,
                    buf,
                } => scratch_read(io, self.scratch_start, self.scratch_limit, offset, buf)?,
                _ => return Err(Error::Corrupt("component producer preparation request").into()),
            }
        }
    }

    pub fn record<N: Read + Write + Seek>(
        &mut self,
        role: u8,
        source: &mut impl ComponentSource,
        io: &mut N,
        stop: &dyn how_far::Stop,
    ) -> Result<&[u8], StreamEncodeError> {
        if self.emitted == self.records {
            return Err(Error::Corrupt("component producer excess record").into());
        }
        match &mut self.driver {
            Driver::Lossless(session) => {
                let rect = session
                    .next_group()
                    .ok_or(Error::Corrupt("component producer missing group"))?;
                let len = (rect.width as usize)
                    .checked_mul(rect.height as usize)
                    .and_then(|n| n.checked_mul(self.info.sample.bytes()))
                    .ok_or(Error::TooLarge)?;
                if len > self.limit {
                    return Err(Error::TooLarge.into());
                }
                self.input.clear();
                self.input
                    .try_reserve_exact(len)
                    .map_err(|_| Error::TooLarge)?;
                self.input.resize(len, 0);
                fill(&self.info, role, source, rect, &mut self.input, stop)?;
                session.push_group(&self.input, rect.width as usize * self.info.sample.bytes())?;
                copy(session.output(), &mut self.record, self.limit)?;
                session.consume_output(session.output().len())?;
            }
            Driver::Lossy(session) => loop {
                stop.check().map_err(|_| Error::Cancelled)?;
                match session.step()? {
                    BoundedLossyStep::Rows { pass, y0, rows } => {
                        if pass != 2 || y0 != self.requested_y {
                            return Err(Error::Corrupt("component producer source order").into());
                        }
                        let height = rows.len() / self.info.row_bytes();
                        fill(
                            &self.info,
                            role,
                            source,
                            GroupRect {
                                x: 0,
                                y: y0,
                                width: self.info.width,
                                height: height as u32,
                            },
                            rows,
                            stop,
                        )?;
                        self.requested_y += height as u32;
                    }
                    BoundedLossyStep::Write {
                        stream: BoundedStream::Output,
                        offset,
                        bytes,
                    } => {
                        if offset != self.output_at {
                            return Err(Error::Corrupt("component producer output order").into());
                        }
                        self.output_at = self
                            .output_at
                            .checked_add(bytes.len() as u64)
                            .ok_or(Error::TooLarge)?;
                        copy(bytes, &mut self.record, self.limit)?;
                        break;
                    }
                    BoundedLossyStep::Write {
                        stream: BoundedStream::IndexScratch,
                        offset,
                        bytes,
                    } => scratch(io, self.scratch_start, self.scratch_limit, offset, bytes)?,
                    BoundedLossyStep::Read {
                        stream: BoundedStream::IndexScratch,
                        offset,
                        buf,
                    } => scratch_read(io, self.scratch_start, self.scratch_limit, offset, buf)?,
                    _ => return Err(Error::Corrupt("component producer record request").into()),
                }
            },
        }
        self.emitted += 1;
        Ok(&self.record)
    }

    pub fn finish<N: Read + Write + Seek>(
        &mut self,
        _role: u8,
        _source: &mut impl ComponentSource,
        io: &mut N,
        stop: &dyn how_far::Stop,
    ) -> Result<(), StreamEncodeError> {
        if self.emitted != self.records {
            return Err(Error::Corrupt("component producer incomplete records").into());
        }
        let Driver::Lossy(session) = &mut self.driver else {
            if let Driver::Lossless(session) = &self.driver
                && !session.is_finished()
            {
                return Err(Error::Corrupt("component group producer incomplete").into());
            }
            return Ok(());
        };
        loop {
            stop.check().map_err(|_| Error::Cancelled)?;
            match session.step()? {
                BoundedLossyStep::Write {
                    stream: BoundedStream::Output,
                    offset,
                    bytes,
                } => {
                    if offset != self.output_at {
                        return Err(Error::Corrupt("component producer tail order").into());
                    }
                    self.output_at = self
                        .output_at
                        .checked_add(bytes.len() as u64)
                        .ok_or(Error::TooLarge)?;
                }
                BoundedLossyStep::Write {
                    stream: BoundedStream::IndexScratch,
                    offset,
                    bytes,
                } => scratch(io, self.scratch_start, self.scratch_limit, offset, bytes)?,
                BoundedLossyStep::Read {
                    stream: BoundedStream::IndexScratch,
                    offset,
                    buf,
                } => scratch_read(io, self.scratch_start, self.scratch_limit, offset, buf)?,
                BoundedLossyStep::Done { len }
                    if len == self.output_at && self.requested_y == self.info.height =>
                {
                    return Ok(());
                }
                _ => return Err(Error::Corrupt("component producer finalization request").into()),
            }
        }
    }
}
