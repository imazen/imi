//! Native component candidate: shared numeric heads/row engines and late validation.
use alloc::{boxed::Box, vec::Vec};
use std::io::Read;

use super::ComponentHeader;
use crate::decode_resources::{DecodePolicy, feed::Budget};
use crate::parse::chunks::{self, Kind, Rules};
use crate::parse::header::{DecodeSpeed, Features};
use crate::parse::limits::Admission;
use crate::parse::records::Walk;
use crate::read::{Src, input::Input};
use crate::{ByteReader, Error, HashAlgorithm, Hasher, ImageInfo, SampleType};
use zenpixels::buffer::{ByteStorage, ByteStorageMut, PixelStorage};
use zenpixels::plane::{AnyPlane, Plane};

const UNIT: u64 = u64::MAX - 4;

/// Publication from the review-only native component reader.
/// Rows and groups remain provisional until `Complete`; errors retain earlier
/// publications. A callback's return applies backpressure to the next read.
#[non_exhaustive]
pub enum ComponentEvent<'a> {
    Declaration(&'a ComponentHeader<'a>),
    Row {
        component: u8,
        y: u32,
        samples: AnyPlane<'a>,
    },
    Group {
        luma_top: u32,
        luma_end: u32,
    },
    Complete {
        encoded_integrity_verified: bool,
        decoded_attestation: bool,
    },
}

/// An output event may suspend before the decoder consumes later bytes.
/// Blocking callbacks return an already-ready result without requiring Send.
pub(super) trait Output {
    fn event(
        &mut self,
        event: ComponentEvent<'_>,
    ) -> impl core::future::Future<Output = Result<(), Error>> + Send;
    fn row(
        &mut self,
        component: u8,
        y: u32,
        storage: &mut PixelStorage,
        start: usize,
        width: usize,
    ) -> impl core::future::Future<Output = Result<(), Error>> + Send;
}
struct Callback<F>(F);
impl<F> Output for Callback<F>
where
    F: for<'a> FnMut(ComponentEvent<'a>) -> Result<(), Error>,
{
    fn event(
        &mut self,
        event: ComponentEvent<'_>,
    ) -> impl core::future::Future<Output = Result<(), Error>> + Send {
        core::future::ready((self.0)(event))
    }
    fn row(
        &mut self,
        component: u8,
        y: u32,
        storage: &mut PixelStorage,
        start: usize,
        width: usize,
    ) -> impl core::future::Future<Output = Result<(), Error>> + Send {
        core::future::ready(storage_row(storage, start, width).and_then(|samples| {
            (self.0)(ComponentEvent::Row {
                component,
                y,
                samples,
            })
        }))
    }
}

/// Decode the unallocated native component candidate from a forward reader.
///
/// Numeric heads/records use the existing lossless/lossy engines. Output is
/// native typed code planes, without chroma upsampling or CMS. The reader asks
/// only for the current operation's bytes and publishes completed groups before
/// requesting later input. Explicit EOF, index agreement and requested checks
/// precede `Complete`. This lab API and syntax require review before adoption.
pub fn decode_reader<R: Read>(
    reader: R,
    policy: DecodePolicy,
    output: impl FnMut(ComponentEvent<'_>) -> Result<(), Error>,
) -> Result<(), Error> {
    let budget = Budget::new(policy, None, zencodec::ResourceLimits::none())?;
    let mut output = Callback(output);
    crate::parse::payload::ready(decode::<_, true>(reader, policy, &budget, &mut output))
        .map_err(|error| budget.failure().unwrap_or(error))
}

struct Admitted<R, const COUNT_INPUT: bool> {
    reader: R,
    budget: Budget,
}
impl<R: Input, const COUNT_INPUT: bool> Input for Admitted<R, COUNT_INPUT> {
    async fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.reader.read(out).await?;
        if COUNT_INPUT {
            self.budget
                .input(n)
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::OutOfMemory))?;
        }
        Ok(n)
    }
    fn account(&mut self, bytes: usize) -> Result<(), Error> {
        self.budget.capacity(bytes)
    }
    fn reserve<T>(&mut self, out: &mut Vec<T>, additional: usize) -> Result<(), Error> {
        reserve(out, additional, &self.budget)
    }
}

pub(super) fn reserve<T>(
    out: &mut Vec<T>,
    additional: usize,
    budget: &Budget,
) -> Result<(), Error> {
    let needed = out.len().checked_add(additional).ok_or(Error::TooLarge)?;
    if needed <= out.capacity() {
        return Ok(());
    }
    let target = needed.max(out.capacity().saturating_mul(2)).max(8);
    budget.capacity(
        (target - out.capacity())
            .checked_mul(size_of::<T>())
            .ok_or(Error::TooLarge)?,
    )?;
    out.try_reserve_exact(target - out.len())
        .map_err(|_| Error::TooLarge)?;
    if out.capacity() > target {
        budget.capacity(
            (out.capacity() - target)
                .checked_mul(size_of::<T>())
                .ok_or(Error::TooLarge)?,
        )?;
    }
    Ok(())
}

fn resize<T: Default + Clone>(out: &mut Vec<T>, len: usize, budget: &Budget) -> Result<(), Error> {
    reserve(out, len.saturating_sub(out.len()), budget)?;
    out.resize(len, T::default());
    Ok(())
}

enum Driver {
    Lossless {
        parsed: Box<crate::v1::Parsed<'static>>,
        map: Box<crate::v1::OutMap>,
    },
    Lossy(Box<crate::lossy::rows::LossyRows<'static>>),
}
#[derive(Default)]
struct Record {
    kind: u64,
    bytes: Vec<u8>,
}
struct Records<'a> {
    records: &'a [Record],
    at: usize,
}
impl crate::lossy::rows::Records for Records<'_> {
    fn record(&mut self, request: crate::lossy::rows::Rec) -> Result<&[u8], Error> {
        let record = self
            .records
            .get(self.at)
            .ok_or(Error::Corrupt("missing component numeric record"))?;
        if record.kind != request.kind() {
            return Err(Error::Corrupt("component numeric record kind"));
        }
        self.at += 1;
        Ok(&record.bytes)
    }
}

/// A checked native row borrowing the upstream sample-typed allocation.
pub(super) fn storage_row(
    storage: &PixelStorage,
    start: usize,
    width: usize,
) -> Result<AnyPlane<'_>, Error> {
    let end = start.checked_add(width).ok_or(Error::TooLarge)?;
    match storage {
        PixelStorage::U8(values) => Ok(AnyPlane::U8(
            Plane::new(
                values
                    .get(start..end)
                    .ok_or(Error::Corrupt("component row extent"))?,
                width,
                1,
                width,
            )
            .map_err(|_| Error::TooLarge)?,
        )),
        PixelStorage::U16(values) => Ok(AnyPlane::U16(
            Plane::new(
                values
                    .get(start..end)
                    .ok_or(Error::Corrupt("component row extent"))?,
                width,
                1,
                width,
            )
            .map_err(|_| Error::TooLarge)?,
        )),
        _ => Err(Error::Unsupported("component sample storage")),
    }
}

fn resize_storage(storage: &mut PixelStorage, len: usize, budget: &Budget) -> Result<(), Error> {
    match storage {
        PixelStorage::U8(values) => resize(values, len, budget),
        PixelStorage::U16(values) => resize(values, len, budget),
        _ => Err(Error::Unsupported("component sample storage")),
    }
}

struct Numeric {
    info: ImageInfo,
    driver: Driver,
    cursor: super::numeric::Cursor,
    walk: Walk,
    ordinals: bool,
    queue: Vec<Record>,
    used: usize,
    raster: crate::pixelsum::Raster,
    packed: PixelStorage,
    published: u32,
}
impl Numeric {
    fn new(
        header: &ComponentHeader<'_>,
        role: usize,
        features: Features,
        head: &[u8],
        budget: &Budget,
    ) -> Result<Self, Error> {
        let parsed = super::numeric::Head::parse(header, role, features, head, budget)?;
        let cursor = parsed.cursor();
        let super::numeric::Head {
            info,
            coding,
            hint,
            ordinals,
            ..
        } = parsed;
        let raster = crate::pixelsum::Raster::new(HashAlgorithm::Xxh3_128, &info, 0)?;
        let driver = match coding {
            super::numeric::Coding::Lossless(parsed) => {
                let map = Box::new(crate::v1::OutMap::new(&info, &parsed.layout));
                Driver::Lossless { parsed, map }
            }
            super::numeric::Coding::Lossy(parsed) => {
                Driver::Lossy(Box::new(crate::lossy::rows::LossyRows::new(parsed)?))
            }
        };
        let walk = Walk::with_ordinals(crate::parse::header::HEADER_VERSION, hint, ordinals);
        let packed = match info.sample {
            SampleType::U8 => PixelStorage::U8(Vec::new()),
            SampleType::U16 => PixelStorage::U16(Vec::new()),
            _ => return Err(Error::Unsupported("component sample storage")),
        };
        Ok(Self {
            info,
            driver,
            cursor,
            walk,
            ordinals,
            queue: Vec::new(),
            used: 0,
            raster,
            packed,
            published: 0,
        })
    }

    async fn publish(
        &mut self,
        role: u8,
        budget: &Budget,
        output: &mut impl Output,
    ) -> Result<(), Error> {
        let stripe = self.cursor.stripe - 1;
        let y = stripe * self.cursor.rows;
        let rows = self.cursor.rows.min(self.info.height as usize - y);
        let row_bytes = self.info.row_bytes();
        let check = || budget.check();
        match &mut self.driver {
            Driver::Lossless { parsed, map } => {
                resize_storage(
                    &mut self.packed,
                    (self.info.width as usize)
                        .checked_mul(rows)
                        .ok_or(Error::TooLarge)?,
                    budget,
                )?;
                for (group, record) in self.queue[..self.used].iter().enumerate() {
                    let (x, width, _) = parsed.group(group);
                    let mut sink = crate::v1::Strided {
                        out: &mut self.packed.as_bytes_mut()[x * self.info.sample.bytes()..],
                        stride: row_bytes,
                        n: width * self.info.sample.bytes(),
                    };
                    crate::v1::decode_unit_rows(
                        parsed,
                        map,
                        stripe * self.cursor.groups + group,
                        &record.bytes,
                        &mut sink,
                        &check,
                    )?;
                }
            }
            Driver::Lossy(driver) => {
                let mut records = Records {
                    records: &self.queue[..self.used],
                    at: 0,
                };
                crate::parse::payload::ready(driver.stripe_async::<_, true>(
                    stripe,
                    &mut records,
                    &check,
                    budget,
                ))?;
                if records.at != self.used {
                    return Err(Error::Corrupt("unused component numeric records"));
                }
                resize_storage(&mut self.packed, self.info.width as usize, budget)?;
            }
        }
        for j in 0..rows {
            budget.check()?;
            let start = match &self.driver {
                Driver::Lossless { .. } => j * self.info.width as usize,
                Driver::Lossy(driver) => {
                    driver.pack_row(j, self.packed.as_bytes_mut());
                    0
                }
            };
            let byte_start = start * self.info.sample.bytes();
            self.raster
                .row(&self.packed.as_bytes()[byte_start..][..row_bytes])?;
            let max = (1u32 << self.info.significant_bits[0]) - 1;
            let invalid = match storage_row(&self.packed, start, self.info.width as usize)? {
                AnyPlane::U8(plane) => plane.row(0).unwrap().iter().any(|&v| u32::from(v) > max),
                AnyPlane::U16(plane) => plane.row(0).unwrap().iter().any(|&v| u32::from(v) > max),
                _ => return Err(Error::Unsupported("component sample storage")),
            };
            if invalid {
                return Err(Error::Corrupt("component code outside declared depth"));
            }
            output
                .row(
                    role,
                    (y + j) as u32,
                    &mut self.packed,
                    start,
                    self.info.width as usize,
                )
                .await?;
            self.published += 1;
        }
        self.used = 0;
        Ok(())
    }
}

async fn prefix<R: Input>(
    src: &mut Src<R>,
    policy: DecodePolicy,
    budget: &Budget,
) -> Result<Vec<u8>, Error> {
    let (_, _, max_header, max_metadata, max_chunks, _) = policy.component_probe_limits();
    let mut pre = Vec::new();
    src.take_async(8, &mut pre).await?;
    if pre != crate::parse::header::SIGNATURE {
        return Err(Error::NotImij);
    }
    let size = src.varint_async(Some(&mut pre)).await?;
    if size > max_header {
        return Err(Error::TooLarge);
    }
    src.take_async(size, &mut pre).await?;
    // Reject critical interpretation before waiting for metadata or DATA. ICC
    // presence is checked separately after its required pre-DATA publication.
    super::inspect_declaration(&pre, policy)?;
    let mut rules = Rules::new(false, true);
    let mut metadata = 0u64;
    for _ in 0..max_chunks {
        budget.chunk_start()?;
        let mut tag = [0; 4];
        src.exact_async(&mut tag).await?;
        reserve(&mut pre, 4, budget)?;
        pre.extend_from_slice(&tag);
        let encoded = src.varint_async(Some(&mut pre)).await?;
        let len = encoded.checked_sub(1);
        let kind = rules.head(tag, len)?;
        if tag == chunks::DATA {
            if len.is_some() {
                return Err(Error::Unsupported("component candidate requires open DATA"));
            }
            return Ok(pre);
        }
        if kind != Kind::Meta {
            return Err(Error::Unsupported(
                "component candidate requires trailing integrity",
            ));
        }
        let size = len.ok_or(Error::Corrupt("open component metadata"))?;
        metadata = metadata
            .checked_add(size)
            .filter(|n| *n <= max_metadata)
            .ok_or(Error::TooLarge)?;
        let at = pre.len();
        src.take_async(size, &mut pre).await?;
        rules.body(tag, &pre[at..])?;
        budget.chunk(tag, &pre[at..])?;
    }
    Err(Error::TooLarge)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

pub(super) async fn decode<R: Input, const COUNT_INPUT: bool>(
    reader: R,
    policy: DecodePolicy,
    budget: &Budget,
    output: &mut impl Output,
) -> Result<(), Error> {
    let (limits, max_input, max_header, _, _, _) = policy.component_probe_limits();
    let mut src = Src::component(
        Admitted::<_, COUNT_INPUT> {
            reader,
            budget: (*budget).clone(),
        },
        max_input,
        max_input,
    );
    let mut pre = prefix(&mut src, policy, budget).await?;
    let header = super::inspect(&pre, policy)?;
    src.start_checksum(header.checksum, limits.verify_checksum, &pre)?;
    // The initial candidate requires its announced encoded integrity, while
    // honoring the ordinary explicit verification-off policy.
    if header.checksum.is_none() {
        return Err(Error::Unsupported(
            "component checksum announcement required",
        ));
    }
    output.event(ComponentEvent::Declaration(&header)).await?;
    let data_at = src.pos;
    let mut heads = Vec::new();
    reserve(&mut heads, 2, budget)?;
    let version = src.u8_async().await?;
    heads.push(version);
    let count = src.u8_async().await?;
    heads.push(count);
    if !matches!(version, 1..=3) || usize::from(count) != header.layout.planes().len() {
        return Err(Error::Unsupported("component DATA version/count"));
    }
    let mut planes = Vec::new();
    let mut common_rows = 1u64;
    for role in 0..count as usize {
        let found = src.u8_async().await?;
        reserve(&mut heads, 1, budget)?;
        heads.push(found);
        if found as usize != role {
            return Err(Error::Corrupt("component DATA head order"));
        }
        let features = src.varint_async(Some(&mut heads)).await?;
        let features = Features::from_bits(features)
            .ok_or(Error::Unsupported("component numeric features"))?;
        if features.bits() & !header.numeric_features.bits() != 0 {
            return Err(Error::Corrupt("undeclared component numeric features"));
        }
        if header.plane_lossy_color(role).is_some()
            && !features.contains(crate::parse::header::Feature::Lossy)
        {
            return Err(Error::Corrupt("component lossy feature missing"));
        }
        let speed = src.u8_async().await?;
        reserve(&mut heads, 1, budget)?;
        heads.push(speed);
        if DecodeSpeed::from_u8(speed).is_none() {
            return Err(Error::Unsupported("component numeric speed"));
        }
        let size = src.varint_async(Some(&mut heads)).await?;
        if size == 0 || size > max_header {
            return Err(Error::TooLarge);
        }
        let at = heads.len();
        src.take_async(size, &mut heads).await?;
        let plane = Numeric::new(&header, role, features, &heads[at..], budget)?;
        let scale = if matches!(role, 1 | 2)
            && header.layout.subsampling() == zenpixels::sample::ChromaSubsampling::Yuv420
        {
            2
        } else {
            1
        };
        let rows = (plane.cursor.rows as u64)
            .checked_mul(scale)
            .ok_or(Error::TooLarge)?;
        common_rows = (common_rows / gcd(common_rows, rows))
            .checked_mul(rows)
            .ok_or(Error::TooLarge)?;
        reserve(&mut planes, 1, budget)?;
        planes.push(plane);
    }
    let mut prefix_hash = Hasher::new(HashAlgorithm::Xxh3_128);
    prefix_hash.update(&pre);
    prefix_hash.update(&heads);
    let prefix_digest = prefix_hash.finish();
    // Header borrows `pre`; retain its bytes without another copy. `heads`
    // can be released after its physical prefix has been hashed.
    heads.clear();
    let mut entries: Vec<[u64; 10]> = Vec::new();
    let mut luma_top = 0u64;
    while planes.iter().any(|p| p.cursor.stripe != p.cursor.stripes) {
        budget.check()?;
        let start = src.pos;
        if src.varint_async(None).await? != UNIT {
            return Err(Error::Unsupported("component unit kind"));
        }
        let length = src.varint_async(None).await?;
        let end = src
            .pos
            .checked_add(length)
            .filter(|n| *n <= max_input)
            .ok_or(Error::TooLarge)?;
        src.component_boundary(Some(end))?;
        let role = src.varint_async(None).await?;
        let index = usize::try_from(role).map_err(|_| Error::TooLarge)?;
        let plane = planes
            .get_mut(index)
            .ok_or(Error::Corrupt("component unit role"))?;
        let ordinal = src.varint_async(None).await?;
        if ordinal != plane.cursor.ordinal {
            return Err(Error::Corrupt("component numeric ordinal"));
        }
        let expected = plane.cursor.expected()?;
        if version == 1 {
            for value in expected {
                if src.varint_async(None).await? != value {
                    return Err(Error::Corrupt("component unit rectangle/kind"));
                }
            }
        }
        let group_end = luma_top
            .saturating_add(common_rows)
            .min(u64::from(header.height));
        let sy = if matches!(index, 1 | 2)
            && header.layout.subsampling() == zenpixels::sample::ChromaSubsampling::Yuv420
        {
            2
        } else {
            1
        };
        if expected[2] >= group_end.div_ceil(sy) {
            return Err(Error::Corrupt("component group lookahead"));
        }
        let mut size = end
            .checked_sub(src.pos)
            .ok_or(Error::Corrupt("component association length"))?;
        let legacy_size = usize::try_from(size).map_err(|_| Error::TooLarge)?;
        let legacy_span = crate::varint_len(expected[0]) + crate::varint_len(size) + legacy_size;
        plane
            .walk
            .head(0, legacy_span as u64, expected[0], expected[0])?;
        if plane.ordinals {
            let at = src.pos;
            if src.varint_async(None).await? != ordinal {
                return Err(Error::Corrupt("component embedded numeric ordinal"));
            }
            size -= src.pos - at;
        }
        if plane.used == plane.queue.len() {
            reserve(&mut plane.queue, 1, budget)?;
            plane.queue.push(Record::default());
        }
        let record = &mut plane.queue[plane.used];
        record.kind = expected[0];
        record.bytes.clear();
        src.take_async(size, &mut record.bytes).await?;
        src.component_boundary(None)?;
        reserve(&mut entries, 1, budget)?;
        entries.push([
            start,
            end - start,
            role,
            ordinal,
            expected[0],
            expected[1],
            expected[2],
            expected[3],
            expected[4],
            legacy_size as u64,
        ]);
        plane.used += 1;
        if plane.cursor.advance() {
            plane.publish(role as u8, budget, output).await?;
        }
        let ready = planes.iter().enumerate().all(|(i, p)| {
            let sy = if matches!(i, 1 | 2)
                && header.layout.subsampling() == zenpixels::sample::ChromaSubsampling::Yuv420
            {
                2
            } else {
                1
            };
            u64::from(p.published) == group_end.div_ceil(sy)
        });
        if ready {
            output
                .event(ComponentEvent::Group {
                    luma_top: luma_top as u32,
                    luma_end: group_end as u32,
                })
                .await?;
            luma_top = group_end;
        }
    }
    if luma_top != u64::from(header.height) {
        return Err(Error::Corrupt("component group coverage"));
    }
    let data_len = src.pos - data_at;
    for plane in &planes {
        plane.walk.finish()?;
        if let Driver::Lossy(driver) = &plane.driver {
            driver.finish()?;
        }
    }
    let mut bitmap = None;
    if version == 3 {
        if src.tag_async().await? != Some(*b"pSUM") || src.varint_async(None).await? != 22 {
            return Err(Error::Corrupt("component bitmap table extent"));
        }
        let mut table = [0; 21];
        src.exact_async(&mut table).await?;
        if table[..5] != [2, 1, 16, 0, 1] {
            return Err(Error::Unsupported("component bitmap table version"));
        }
        bitmap = Some(<[u8; 16]>::try_from(&table[5..]).unwrap());
    }
    if src.tag_async().await? != Some(*b"cSUM") {
        return Err(Error::Corrupt("component trailing checksum required"));
    }
    let size = src
        .uncovered_varint()
        .await?
        .checked_sub(1)
        .ok_or(Error::Corrupt("open component checksum"))?;
    let mut table = Vec::new();
    crate::read::checksum_chunk(
        &mut src,
        if limits.verify_checksum {
            Kind::Checksum
        } else {
            Kind::Ignored
        },
        size,
        &mut table,
    )
    .await?;
    let index_at = src.pos;
    if src.tag_async().await? != Some(*b"uIDX") {
        return Err(Error::Corrupt("component trailing index required"));
    }
    let size = src
        .varint_async(None)
        .await?
        .checked_sub(1)
        .ok_or(Error::Corrupt("open component index"))?;
    table.clear();
    src.take_async(size, &mut table).await?;
    let mut index = ByteReader::new(&table);
    if index.u8()? != if version == 1 { 254 } else { 253 }
        || index.bytes(16)? != prefix_digest.as_bytes()
    {
        return Err(Error::Corrupt("component index/header binding"));
    }
    if index.varint()? != entries.len() as u64 {
        return Err(Error::Corrupt("component index count"));
    }
    for entry in &entries {
        if version == 1 {
            for value in entry {
                if index.varint()? != *value {
                    return Err(Error::Corrupt("component index span/association"));
                }
            }
        } else {
            for value in [entry[2], entry[3], entry[1]] {
                if index.varint()? != value {
                    return Err(Error::Corrupt("component compact index span/association"));
                }
            }
        }
    }
    if u64::from_le_bytes(index.bytes(8)?.try_into().unwrap()) != data_len
        || u64::from_le_bytes(index.bytes(8)?.try_into().unwrap()) != src.pos - index_at
        || index.bytes(4)? != b"uIDX"
        || index.remaining() != 0
    {
        return Err(Error::Corrupt("component index footer"));
    }
    // Ask for EOF only after all provisional output, never to release a group.
    if src.tag_async().await?.is_some() {
        return Err(Error::Corrupt("trailing component bytes"));
    }
    src.finish_checksum()?;
    if let Some(expected) = bitmap {
        let mut root = Hasher::new(HashAlgorithm::Xxh3_128);
        root.update(b"IMIJ/pSUM/v2\0\0\0\0");
        root.update(&header.width.to_le_bytes());
        root.update(&header.height.to_le_bytes());
        root.update(&0u64.to_le_bytes());
        root.update(&[count]);
        for (role, plane) in planes.iter().enumerate() {
            root.update(&[role as u8]);
            root.update(&plane.info.width.to_le_bytes());
            root.update(&plane.info.height.to_le_bytes());
            root.update(&[plane.info.sample as u8]);
            root.update(plane.raster.finish()?.as_bytes());
        }
        if root.finish().as_bytes() != expected {
            return Err(crate::pixelsum::MISMATCH);
        }
    }
    output
        .event(ComponentEvent::Complete {
            encoded_integrity_verified: limits.verify_checksum
                && header.checksum.is_some_and(|a| a.algorithm().is_some()),
            decoded_attestation: version == 3,
        })
        .await?;
    // Keep the borrowed declaration alive through the final callback.
    pre.clear();
    Ok(())
}
