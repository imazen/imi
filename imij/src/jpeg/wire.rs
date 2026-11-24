//! Bounded native-component MCU bands; component entropy stays in the existing
//! Gray coefficient codec. Source grids and quantizers live outside carriers.

use super::{ChromaSiting, CoefficientImage, ComponentColor, ComponentInfo, QuantizedComponent};
use crate::parse::{limits::Admission, payload::HeadInput, records};
use crate::{ByteReader, Error, ImageInfo};
#[cfg(feature = "encode")]
use crate::{
    Metadata,
    parse::header::{Coding, DecodeSpeed, Feature, Features},
};
use alloc::{collections::VecDeque, vec::Vec};

/// Coefficient-independent image description. Coded grids include every source
/// block, including MCU edge padding. This is small enough to hold while a
/// producer supplies only one bounded band of coefficient rows at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeHeader {
    pub width: u32,
    pub height: u32,
    pub color: ComponentColor,
    pub siting: ChromaSiting,
    pub tables: [Option<[u16; 64]>; 4],
    pub components: Vec<ComponentInfo>,
}

impl NativeHeader {
    pub fn image_info(&self) -> ImageInfo {
        ImageInfo::new(
            self.width,
            self.height,
            if self.color == ComponentColor::Gray {
                crate::Channels::Gray
            } else {
                crate::Channels::Rgb
            },
            crate::SampleType::U8,
        )
    }

    pub fn validate(&self) -> Result<(), Error> {
        let n = if self.color == ComponentColor::Gray {
            1
        } else {
            3
        };
        if self.width == 0
            || self.height == 0
            || self.components.len() != n
            || self.tables.iter().flatten().any(|t| t.contains(&0))
        {
            return Err(Error::InvalidInput("native band header"));
        }
        self.image_info()
            .checked_buffer_len_u64()
            .ok_or(Error::TooLarge)?;
        (self.width as usize)
            .checked_mul(n)
            .ok_or(Error::TooLarge)?;
        if self.components.iter().any(|p| {
            !(1..=4).contains(&p.horizontal_sampling) || !(1..=4).contains(&p.vertical_sampling)
        }) {
            return Err(Error::InvalidInput("native sampling factors"));
        }
        let y = self.components[0];
        if n == 3 {
            let cb = self.components[1];
            let cr = self.components[2];
            let same = |a: ComponentInfo, b: ComponentInfo| {
                a.horizontal_sampling == b.horizontal_sampling
                    && a.vertical_sampling == b.vertical_sampling
            };
            if !same(cb, cr)
                || !(same(y, cb)
                    || (y.horizontal_sampling == 2 * cb.horizontal_sampling
                        && y.vertical_sampling == 2 * cb.vertical_sampling))
            {
                return Err(Error::Unsupported("native sampling: Gray, 444 or 420"));
            }
        }
        for (i, p) in self.components.iter().enumerate() {
            let (w, h) = self.component_size(i);
            if self.components[..i].iter().any(|q| q.id == p.id)
                || self
                    .tables
                    .get(p.quant_table as usize)
                    .is_none_or(Option::is_none)
                || p.blocks_wide < w.div_ceil(8) as u32
                || p.blocks_high < h.div_ceil(8) as u32
                || p.blocks_wide as u64
                    > (self.width as u64).div_ceil(8 * y.horizontal_sampling as u64)
                        * p.horizontal_sampling as u64
                || p.blocks_high as u64
                    > (self.height as u64).div_ceil(8 * y.vertical_sampling as u64)
                        * p.vertical_sampling as u64
            {
                return Err(Error::InvalidInput("native coded component grid"));
            }
            p.blocks_wide.checked_mul(8).ok_or(Error::TooLarge)?;
        }
        Ok(())
    }

    pub(crate) fn component_size(&self, i: usize) -> (usize, usize) {
        let p = self.components[i];
        let y = self.components[0];
        (
            ((self.width as u64 * p.horizontal_sampling as u64)
                .div_ceil(y.horizontal_sampling as u64)) as usize,
            ((self.height as u64 * p.vertical_sampling as u64).div_ceil(y.vertical_sampling as u64))
                as usize,
        )
    }
    pub(crate) fn mcu_rows(&self) -> usize {
        (self.height as usize).div_ceil(8 * self.components[0].vertical_sampling as usize)
    }
    pub(crate) fn band_count(&self, rows: u16) -> usize {
        self.mcu_rows().div_ceil(rows as usize)
    }
    pub(crate) fn band_grid(&self, band: usize, rows: u16, i: usize) -> (usize, usize) {
        let p = self.components[i];
        let start = band * rows as usize * p.vertical_sampling as usize;
        (
            start,
            (p.blocks_high as usize - start).min(rows as usize * p.vertical_sampling as usize),
        )
    }
    pub(crate) fn pixel_band_rows(&self, rows: u16) -> usize {
        8 * rows as usize * self.components[0].vertical_sampling as usize
    }
    pub(crate) fn plan(&self, rows: u16) -> Result<(u128, u128, u128), Error> {
        let bands = self.band_count(rows) as u128;
        let blocks: u128 = self
            .components
            .iter()
            .map(|p| p.blocks_wide as u128 * p.blocks_high as u128)
            .sum();
        let band_blocks: u128 = self
            .components
            .iter()
            .map(|p| {
                p.blocks_wide as u128
                    * (rows as u128 * p.vertical_sampling as u128).min(p.blocks_high as u128)
            })
            .sum();
        // Three native sample bands and one transient signed coefficient band;
        // existing carrier/DC scratch, models and a bounded compressed band.
        // Parsed band slices and shared hint/walk/trailer span storage are
        // proportional to band count even though coefficient buffers are not.
        let index_bytes = bands * (2 * core::mem::size_of::<&[u8]>() as u128 + 3 * 8);
        let memory = index_bytes
            + band_blocks * (3 * 64 * 4 + 64 * 2 + 2048 + 3 * 64 * 8)
            + 3 * crate::lossy::decoder_table_bytes()
            + 2 * 1024 * 1024;
        Ok((
            memory,
            bands * (1 + self.components.len() as u128 * 4) + blocks * 2,
            blocks * 64 * 12 + bands * 3 * crate::lossy::decoder_table_bytes(),
        ))
    }
}

impl CoefficientImage {
    pub fn native_header(&self) -> Result<NativeHeader, Error> {
        self.validate()?;
        Ok(NativeHeader {
            width: self.width,
            height: self.height,
            color: self.color,
            siting: self.siting,
            tables: self.tables,
            components: self.components.iter().map(|p| p.info).collect(),
        })
    }

    /// Encodes coefficient arrays as independent bounded MCU row bands.
    /// For a producer that holds only one band, use `BandEncoder` directly.
    #[cfg(feature = "encode")]
    pub fn encode(
        &self,
        metadata: &Metadata,
        mcu_rows_per_band: u16,
        entropy: crate::LossyEntropy,
        effort: f32,
    ) -> Result<Vec<u8>, Error> {
        let header = self.native_header()?;
        let mut out = Vec::new();
        let mut writer = BandEncoder::new(
            header,
            metadata,
            mcu_rows_per_band,
            entropy,
            effort,
            |bytes: &[u8]| {
                out.try_reserve(bytes.len()).map_err(|_| Error::TooLarge)?;
                out.extend_from_slice(bytes);
                Ok(())
            },
        )?;
        for band in 0..writer.header.band_count(mcu_rows_per_band) {
            let rows: Vec<&[i16]> = self
                .components
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let (start, n) = writer.header.band_grid(band, mcu_rows_per_band, i);
                    let width = p.info.blocks_wide as usize * 64;
                    &p.coefficients[start * width..(start + n) * width]
                })
                .collect();
            writer.push_band(&rows)?;
        }
        writer.finish()?;
        let data_at = crate::container::data_at(&out)?;
        let f = crate::v1::parse_file(
            &out,
            &crate::parse::limits::Structure,
            crate::checksum::Verify::Off,
        )?;
        let mut r = ByteReader::new(f.data);
        let mut head = read_head(f.header.info, &mut r, &crate::parse::limits::Structure)?;
        let head_end = r.pos;
        let count = head.bands();
        let spans =
            records::UnitMap::native().spans(&mut r, f.header.version, count, head.hint.take())?;
        let mut list = Vec::new();
        crate::v1::put_unit_table(&mut list, count, &spans);
        records::put_record(
            &mut list,
            crate::registry::HEAD_NATIVE_COMPONENTS,
            &description(&head.header, head.rows),
        );
        let mut payload = Vec::new();
        payload.push(0);
        crate::put_varint(&mut payload, list.len() as u64);
        payload.extend_from_slice(&list);
        payload.extend_from_slice(&f.data[head_end..]);
        let mut file = out[..data_at].to_vec();
        crate::container::put_data(&mut file, &payload);
        crate::container::locate(crate::container::finish_unit_table(file)?)
    }
}

/// Emits a historical v2 DATA stream, one bounded native MCU row band at a time.
/// The sink receives header/metadata once, one complete band per `push_band`,
/// and the standard trailing unit index at `finish`. No full-height coefficient
/// or pixel buffer is retained. A sink error makes this writer unusable.
#[cfg(feature = "encode")]
pub struct BandEncoder<F: FnMut(&[u8]) -> Result<(), Error>> {
    header: NativeHeader,
    rows: u16,
    entropy: crate::LossyEntropy,
    effort: f32,
    sink: F,
    next: usize,
    spans: Vec<u64>,
    data_len: u64,
    failed: bool,
}

fn rows_valid(rows: u16) -> Result<(), Error> {
    if (1..=256).contains(&rows) {
        Ok(())
    } else {
        Err(Error::InvalidInput("native MCU rows per band: 1..256"))
    }
}

#[cfg(feature = "encode")]
fn description(header: &NativeHeader, rows: u16) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&[
        0,
        if header.color == ComponentColor::Gray {
            0
        } else {
            1
        },
        0,
        match header.siting {
            ChromaSiting::Unspecified => 0,
            ChromaSiting::Centered => 1,
            ChromaSiting::Cosited => 2,
        },
    ]);
    crate::put_varint(&mut d, rows as u64);
    let mask = header
        .tables
        .iter()
        .enumerate()
        .fold(0u8, |m, (i, t)| m | ((t.is_some() as u8) << i));
    crate::put_varint(&mut d, u64::from(mask));
    for t in header.tables.iter().flatten() {
        for v in t {
            d.extend_from_slice(&v.to_le_bytes());
        }
    }
    d.push(header.components.len() as u8);
    for p in &header.components {
        d.extend_from_slice(&[
            p.id,
            p.horizontal_sampling,
            p.vertical_sampling,
            p.quant_table,
        ]);
        crate::put_varint(&mut d, p.blocks_wide as u64);
        crate::put_varint(&mut d, p.blocks_high as u64);
    }
    d
}

#[cfg(feature = "encode")]
impl<F: FnMut(&[u8]) -> Result<(), Error>> BandEncoder<F> {
    pub fn new(
        header: NativeHeader,
        metadata: &Metadata,
        mcu_rows_per_band: u16,
        entropy: crate::LossyEntropy,
        effort: f32,
        mut sink: F,
    ) -> Result<Self, Error> {
        header.validate()?;
        rows_valid(mcu_rows_per_band)?;
        crate::effort::Effort::new(effort)?;
        let mut prefix = Vec::new();
        crate::container::write_head_features(
            &mut prefix,
            &header.image_info(),
            metadata,
            Coding::Lossy,
            Some(crate::LossyColor::YCoCg),
            DecodeSpeed::Balanced,
            Features::EMPTY.with(Feature::NativeComponents),
        )?;
        // This experiment writes only its historical v2 envelope. Ordinary
        // nested coefficient carriers use the current lossless/lossy grammar.
        let mut reader = ByteReader::new(&prefix);
        reader.bytes(8)?;
        reader.varint()?;
        let version_at = reader.pos;
        prefix[version_at] = crate::parse::header::HEADER_VERSION_2;
        let mut records = Vec::new();
        records::put_record(
            &mut records,
            records::H_UNIT_RESERVE,
            &[records::RESERVE_TAIL],
        );
        records::put_record(
            &mut records,
            crate::registry::HEAD_NATIVE_COMPONENTS,
            &description(&header, mcu_rows_per_band),
        );
        let mut head = Vec::new();
        head.push(0);
        crate::put_varint(&mut head, records.len() as u64);
        head.extend_from_slice(&records);
        prefix.extend_from_slice(b"DATA");
        crate::put_varint(&mut prefix, 0);
        prefix.extend_from_slice(&head);
        sink(&prefix)?;
        let spans = Vec::new();
        Ok(Self {
            header,
            rows: mcu_rows_per_band,
            entropy,
            effort,
            sink,
            next: 0,
            spans,
            data_len: head.len() as u64,
            failed: false,
        })
    }

    /// Components in source order, natural frequency order. Each slice contains
    /// exactly the coded block rows of this MCU band, including edge blocks.
    pub fn push_band(&mut self, components: &[&[i16]]) -> Result<(), Error> {
        if self.failed
            || self.next >= self.header.band_count(self.rows)
            || components.len() != self.header.components.len()
        {
            return Err(Error::InvalidInput("native encoder band state"));
        }
        // Validate every input slice before encoding or writing any bytes.
        for (i, q) in components.iter().enumerate() {
            let (_, n) = self.header.band_grid(self.next, self.rows, i);
            let count = n
                .checked_mul(self.header.components[i].blocks_wide as usize)
                .and_then(|n| n.checked_mul(64))
                .ok_or(Error::TooLarge)?;
            if q.len() != count {
                return Err(Error::InvalidInput("native band coefficient count"));
            }
        }
        let mut payload = Vec::new();
        crate::put_varint(&mut payload, self.next as u64);
        crate::put_varint(&mut payload, (self.next * self.rows as usize) as u64);
        crate::put_varint(
            &mut payload,
            (self.header.mcu_rows() - self.next * self.rows as usize).min(self.rows as usize)
                as u64,
        );
        for (i, q) in components.iter().enumerate() {
            let p = self.header.components[i];
            let (start, n) = self.header.band_grid(self.next, self.rows, i);
            for v in [p.blocks_wide as u64, start as u64, n as u64] {
                crate::put_varint(&mut payload, v);
            }
            let carrier = crate::lossy::encode_native_component(
                p.blocks_wide.checked_mul(8).ok_or(Error::TooLarge)?,
                (n as u32).checked_mul(8).ok_or(Error::TooLarge)?,
                self.header.tables[p.quant_table as usize].as_ref().unwrap(),
                q,
                self.entropy,
                self.effort,
            )?;
            crate::put_varint(&mut payload, carrier.len() as u64);
            payload.extend_from_slice(&carrier);
        }
        let mut record = Vec::new();
        records::put_record(&mut record, 31, &payload);
        self.spans.try_reserve(1).map_err(|_| Error::TooLarge)?;
        if let Err(e) = (self.sink)(&record) {
            self.failed = true;
            return Err(e);
        }
        self.data_len = self
            .data_len
            .checked_add(record.len() as u64)
            .ok_or(Error::TooLarge)?;
        self.spans.push(record.len() as u64);
        self.next += 1;
        Ok(())
    }

    pub fn bands_remaining(&self) -> usize {
        self.header.band_count(self.rows) - self.next
    }

    /// Coded width and absolute source block rows needed for the next band.
    pub fn next_component_grid(
        &self,
        component: usize,
    ) -> Result<(u32, core::ops::Range<u32>), Error> {
        if component >= self.header.components.len() || self.bands_remaining() == 0 {
            return Err(Error::InvalidInput("native next band component"));
        }
        let (start, n) = self.header.band_grid(self.next, self.rows, component);
        Ok((
            self.header.components[component].blocks_wide,
            start as u32..(start + n) as u32,
        ))
    }

    /// Copies the next band's payload without entropy re-encoding, translating
    /// record ids to the writer's version. Source head tables/grids, band size
    /// and index must match;
    /// callers may alternate copied and newly encoded bands in one archive.
    pub fn copy_band(
        &mut self,
        source: &super::NativeArchive<'_>,
        band: usize,
    ) -> Result<(), Error> {
        self.copy_band_record(
            source.header(),
            source.header_version(),
            source.mcu_rows_per_band(),
            band,
            source.band_record(band)?,
        )
    }

    /// Copies a complete span obtained through a range reader. The source
    /// description and MCU band size must match this writer exactly. Framing,
    /// band geometry and carrier declarations are checked before sink writes.
    /// `source_version` must be 2. Version 3 rejects this experiment.
    pub fn copy_band_record(
        &mut self,
        source: &NativeHeader,
        source_version: u8,
        rows: u16,
        band: usize,
        record: &[u8],
    ) -> Result<(), Error> {
        crate::parse::header::check_header_version(source_version)?;
        if source_version != crate::parse::header::HEADER_VERSION_2 {
            return Err(Error::Unsupported("native components retired in v3"));
        }
        if self.failed
            || band != self.next
            || self.bands_remaining() == 0
            || *source != self.header
            || rows != self.rows
        {
            return Err(Error::InvalidInput(
                "native range-copy geometry or band index",
            ));
        }
        let mut payload = [&[][..]];
        records::split_span(record, source_version, &[records::R_NATIVE], &mut payload)?;
        let head = NativeHead {
            info: self.header.image_info(),
            header: self.header.clone(),
            rows: self.rows,
            hint: None,
            reserve: None,
        };
        for (i, c) in carriers(&head, band, payload[0])?.iter().enumerate() {
            let p = self.header.components[i];
            let (_, n) = self.header.band_grid(band, self.rows, i);
            crate::lossy::parse_native_component_checked(
                c,
                p.blocks_wide.checked_mul(8).ok_or(Error::TooLarge)?,
                (n as u32).checked_mul(8).ok_or(Error::TooLarge)?,
                self.header.tables[p.quant_table as usize].as_ref().unwrap(),
                &crate::parse::limits::Structure,
            )?;
        }
        let next_len = self
            .data_len
            .checked_add(record.len() as u64)
            .ok_or(Error::TooLarge)?;
        self.spans.try_reserve(1).map_err(|_| Error::TooLarge)?;
        let write = (self.sink)(record);
        if let Err(e) = write {
            self.failed = true;
            return Err(e);
        }
        self.data_len = next_len;
        self.spans.push(record.len() as u64);
        self.next += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), Error> {
        if self.failed || self.next != self.header.band_count(self.rows) {
            return Err(Error::InvalidInput("native encoder incomplete bands"));
        }
        (self.sink)(&records::index_chunk(&self.spans, self.data_len))
    }
}

#[derive(Clone)]
pub(crate) struct NativeHead {
    pub info: ImageInfo,
    pub header: NativeHeader,
    pub rows: u16,
    pub hint: Option<Vec<u64>>,
    pub reserve: Option<records::Reserve>,
}
impl NativeHead {
    pub(crate) fn bands(&self) -> usize {
        self.header.band_count(self.rows)
    }
}

fn read_description(info: &ImageInfo, data: &[u8]) -> Result<(NativeHeader, u16), Error> {
    let mut r = ByteReader::new(data);
    if r.u8()? != 0 {
        return Err(Error::Unsupported("native component head version"));
    }
    let color = match r.u8()? {
        0 => ComponentColor::Gray,
        1 => ComponentColor::YCbCr,
        _ => return Err(Error::Unsupported("native component color")),
    };
    if r.u8()? != 0 {
        return Err(Error::Unsupported("native reconstruction policy"));
    }
    let siting = match r.u8()? {
        0 => ChromaSiting::Unspecified,
        1 => ChromaSiting::Centered,
        2 => ChromaSiting::Cosited,
        _ => return Err(Error::Unsupported("native source siting")),
    };
    let rows = u16::try_from(r.varint()?).map_err(|_| Error::Corrupt("native band rows"))?;
    rows_valid(rows).map_err(|_| Error::Corrupt("native band rows"))?;
    let mask = r.varint()?;
    if mask & !15 != 0 {
        return Err(Error::Corrupt("native table mask"));
    }
    let mut tables = [None; 4];
    for (i, slot) in tables.iter_mut().enumerate() {
        if mask & (1 << i) != 0 {
            let mut t = [0; 64];
            for v in &mut t {
                *v = r.u16()?;
            }
            *slot = Some(t);
        }
    }
    let count = r.u8()? as usize;
    if count != if color == ComponentColor::Gray { 1 } else { 3 } {
        return Err(Error::Corrupt("native component count"));
    }
    let mut components = Vec::new();
    components
        .try_reserve_exact(count)
        .map_err(|_| Error::TooLarge)?;
    for _ in 0..count {
        components.push(ComponentInfo {
            id: r.u8()?,
            horizontal_sampling: r.u8()?,
            vertical_sampling: r.u8()?,
            quant_table: r.u8()?,
            blocks_wide: u32::try_from(r.varint()?).map_err(|_| Error::TooLarge)?,
            blocks_high: u32::try_from(r.varint()?).map_err(|_| Error::TooLarge)?,
        });
    }
    if r.remaining() != 0 {
        return Err(Error::Corrupt("native head trailing bytes"));
    }
    let h = NativeHeader {
        width: info.width,
        height: info.height,
        color,
        siting,
        tables,
        components,
    };
    h.validate().map_err(|e| match e {
        Error::InvalidInput(_) => Error::Corrupt("native band header"),
        e => e,
    })?;
    if h.image_info().channels != info.channels
        || info.sample != crate::SampleType::U8
        || info.has_custom_precision()
        || info.alpha_mode != crate::AlphaMode::Straight
    {
        return Err(Error::Corrupt("native primary image type"));
    }
    Ok((h, rows))
}

pub(crate) fn read_head<I: HeadInput>(
    info: ImageInfo,
    input: &mut I,
    admission: &impl Admission,
) -> Result<NativeHead, Error> {
    crate::parse::payload::ready(read_head_async(
        info,
        &mut crate::parse::payload::ReadyInput(input),
        admission,
    ))
}

pub(crate) async fn read_head_async<I: crate::parse::payload::AsyncHeadInput>(
    info: ImageInfo,
    input: &mut I,
    admission: &impl Admission,
) -> Result<NativeHead, Error> {
    if input.u8().await? != 0 {
        return Err(Error::Corrupt("native DATA discriminator"));
    }
    let n = input.var_usize().await?;
    let max = (info.height as usize)
        .div_ceil(8)
        .checked_mul(10)
        .and_then(|n| n.checked_add(1024))
        .ok_or(Error::TooLarge)?;
    if n > max {
        return Err(Error::Corrupt("native head record length"));
    }
    admission.hint(n)?;
    let at = input.pos();
    let list = input.bytes(n).await?;
    let (mut desc, mut table, mut reservation) = (None, None, None);
    records::head_records_at(list, |id, rec, pos| {
        match id {
            records::H_UNIT_TABLE => table = Some(rec),
            records::H_UNIT_RESERVE => reservation = Some((rec, at + pos as u64)),
            crate::registry::HEAD_NATIVE_COMPONENTS => desc = Some(read_description(&info, rec)?),
            id if id & 1 == 0 => {}
            _ => return Err(Error::Unsupported("unknown native head record")),
        }
        Ok(())
    })?;
    let (header, rows) = desc.ok_or(Error::Corrupt("missing native component declaration"))?;
    if table.is_some() && reservation.is_some() {
        return Err(Error::Corrupt("duplicate unit tables"));
    }
    let count = header.band_count(rows);
    let mut hint = table.map(|r| records::hint_entries(r, count)).transpose()?;
    let reserve = if let Some((rec, at)) = reservation {
        let (state, entries) = records::reserve_entries(rec, count)?;
        if entries.is_some() {
            hint = entries;
        }
        Some(records::Reserve {
            state,
            at,
            len: rec.len(),
        })
    } else {
        None
    };
    admission.native(&header, rows)?;
    Ok(NativeHead {
        info,
        header,
        rows,
        hint,
        reserve,
    })
}

pub(crate) fn carriers<'a>(
    head: &NativeHead,
    band: usize,
    payload: &'a [u8],
) -> Result<Vec<&'a [u8]>, Error> {
    let mut r = ByteReader::new(payload);
    let start = band * head.rows as usize;
    let n = (head.header.mcu_rows() - start).min(head.rows as usize);
    if r.varint()? != band as u64 || r.varint()? != start as u64 || r.varint()? != n as u64 {
        return Err(Error::Corrupt("native band MCU interval"));
    }
    let mut out = Vec::new();
    out.try_reserve_exact(head.header.components.len())
        .map_err(|_| Error::TooLarge)?;
    for (i, p) in head.header.components.iter().enumerate() {
        let (a, b) = head.header.band_grid(band, head.rows, i);
        if r.varint()? != p.blocks_wide as u64 || r.varint()? != a as u64 || r.varint()? != b as u64
        {
            return Err(Error::Corrupt("native band component grid"));
        }
        let n = r.varint()?;
        if n > r.remaining() as u64 {
            return Err(Error::Corrupt("native carrier length"));
        }
        out.push(r.bytes(n as usize)?);
    }
    if r.remaining() != 0 {
        return Err(Error::Corrupt("native band trailing bytes"));
    }
    Ok(out)
}

pub(crate) struct NativeParsed<'a> {
    pub head: NativeHead,
    pub bands: Vec<&'a [u8]>,
    pub payload_len: usize,
}

pub(crate) fn parse<'a>(
    info: ImageInfo,
    version: u8,
    body: &'a [u8],
    admission: &impl Admission,
) -> Result<NativeParsed<'a>, Error> {
    let mut r = ByteReader::new(body);
    let mut head = read_head(info, &mut r, admission)?;
    let n = head.bands();
    records::check_count(n, r.remaining() as u64)?;
    let mut bands = Vec::new();
    bands.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
    let mut split = records::Split::new(&mut r, version, head.hint.take(), n)?;
    for band in 0..n {
        admission.check()?;
        let payload = split.next(records::R_NATIVE)?;
        let cs = carriers(&head, band, payload)?;
        for (i, c) in cs.iter().enumerate() {
            let p = head.header.components[i];
            let (_, bh) = head.header.band_grid(band, head.rows, i);
            crate::lossy::parse_native_component_checked(
                c,
                p.blocks_wide.checked_mul(8).ok_or(Error::TooLarge)?,
                (bh as u32).checked_mul(8).ok_or(Error::TooLarge)?,
                head.header.tables[p.quant_table as usize].as_ref().unwrap(),
                &crate::parse::limits::Checked(admission),
            )?;
        }
        bands.push(payload);
    }
    split.finish()?;
    Ok(NativeParsed {
        head,
        bands,
        payload_len: r.pos,
    })
}

/// Reads native coefficients from a coefficient-preserving IMI file. This
/// allocating access API holds all coefficients; `BandEncoder` does not.
pub fn decode_coefficients(data: &[u8]) -> Result<CoefficientImage, Error> {
    let f = crate::v1::parse_file(
        data,
        &crate::parse::limits::Unrestricted,
        crate::checksum::Verify::Now(1),
    )?;
    if !crate::parse::payload::native_selected(f.header.lossy_color, f.declared, f.data)? {
        return Err(Error::InvalidInput("file has no native component envelope"));
    }
    let p = parse(
        f.header.info,
        f.header.version,
        f.data,
        &crate::parse::limits::Unrestricted,
    )?;
    let h = &p.head.header;
    let mut components: Vec<QuantizedComponent> = h
        .components
        .iter()
        .map(|&info| QuantizedComponent {
            info,
            coefficients: Vec::new(),
        })
        .collect();
    for band in 0..p.head.bands() {
        for (i, c) in carriers(&p.head, band, p.bands[band])?.iter().enumerate() {
            let d = h.components[i];
            let (_, n) = h.band_grid(band, p.head.rows, i);
            let q = crate::lossy::decode_native_component(
                c,
                d.blocks_wide * 8,
                n as u32 * 8,
                h.tables[d.quant_table as usize].as_ref().unwrap(),
            )?;
            components[i]
                .coefficients
                .try_reserve(q.len())
                .map_err(|_| Error::TooLarge)?;
            components[i].coefficients.extend_from_slice(&q);
        }
    }
    Ok(CoefficientImage {
        width: h.width,
        height: h.height,
        color: h.color,
        siting: h.siting,
        tables: h.tables,
        components,
    })
}

struct BandPixels {
    index: usize,
    planes: Vec<Vec<f32>>,
}

pub(crate) struct NativeStream<'a> {
    pub parsed: NativeParsed<'a>,
    pub y: usize,
    cache: VecDeque<BandPixels>,
    chroma: [Vec<f32>; 2],
}
impl<'a> NativeStream<'a> {
    pub(crate) fn new(parsed: NativeParsed<'a>) -> Result<Self, Error> {
        let h = &parsed.head.header;
        if h.siting == ChromaSiting::Cosited
            && h.components.len() == 3
            && h.components[0].horizontal_sampling != h.components[1].horizontal_sampling
        {
            return Err(Error::Unsupported("co-sited native 420 reconstruction"));
        }
        Ok(Self {
            parsed,
            y: 0,
            cache: VecDeque::new(),
            chroma: [Vec::new(), Vec::new()],
        })
    }
    fn load(&mut self, band: usize, check: &impl Fn() -> Result<(), Error>) -> Result<(), Error> {
        self.push_payload(band, self.parsed.bands[band], check)
    }
    pub(crate) fn push_payload(
        &mut self,
        band: usize,
        payload: &[u8],
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let head = &self.parsed.head;
        let h = &head.header;
        let mut planes = Vec::new();
        for (i, c) in carriers(head, band, payload)?.iter().enumerate() {
            check()?;
            let p = h.components[i];
            let (start, n) = h.band_grid(band, head.rows, i);
            let q = crate::lossy::decode_native_component_checked(
                c,
                p.blocks_wide * 8,
                n as u32 * 8,
                h.tables[p.quant_table as usize].as_ref().unwrap(),
                &crate::parse::limits::Callback(check),
            )?;
            let (w, height) = h.component_size(i);
            let rows = (height - start * 8).min(n * 8);
            let mut plane = Vec::new();
            plane
                .try_reserve_exact(w * rows)
                .map_err(|_| Error::TooLarge)?;
            plane.resize(w * rows, 0f32);
            let table = h.tables[p.quant_table as usize].as_ref().unwrap();
            for by in 0..rows.div_ceil(8) {
                check()?;
                for bx in 0..w.div_ceil(8) {
                    let at = (by * p.blocks_wide as usize + bx) * 64;
                    let block = crate::lossy::jpeg::idct(q[at..at + 64].try_into().unwrap(), table);
                    for y in 0..(rows - by * 8).min(8) {
                        for x in 0..(w - bx * 8).min(8) {
                            plane[(by * 8 + y) * w + bx * 8 + x] =
                                block[y * 8 + x].clamp(-128., 127.);
                        }
                    }
                }
            }
            planes.push(plane);
        }
        self.cache.push_back(BandPixels {
            index: band,
            planes,
        });
        Ok(())
    }
    fn row(&self, i: usize, y: usize) -> &[f32] {
        let h = &self.parsed.head.header;
        let rows = self.parsed.head.rows as usize * h.components[i].vertical_sampling as usize * 8;
        let band = y / rows;
        let width = h.component_size(i).0;
        let p = self.cache.iter().find(|p| p.index == band).unwrap();
        let at = (y - band * rows) * width;
        &p.planes[i][at..at + width]
    }
    pub(crate) fn next_row_checked(
        &mut self,
        out: &mut [u8],
        check: &impl Fn() -> Result<(), Error>,
    ) -> Result<bool, Error> {
        self.next_row_from(out, check, |s, band, check| s.load(band, check))
    }
    fn next_row_from<C: Fn() -> Result<(), Error>>(
        &mut self,
        out: &mut [u8],
        check: &C,
        mut load: impl FnMut(&mut Self, usize, &C) -> Result<(), Error>,
    ) -> Result<bool, Error> {
        check()?;
        let info = &self.parsed.head.info;
        if out.len() < info.row_bytes() {
            return Err(Error::InvalidInput("native output row"));
        }
        if self.y >= info.height as usize {
            return Ok(false);
        }
        let band = self.y
            / self
                .parsed
                .head
                .header
                .pixel_band_rows(self.parsed.head.rows);
        while self.cache.front().is_some_and(|p| p.index + 1 < band) {
            self.cache.pop_front();
        }
        let upto = (band + 1).min(self.parsed.head.bands() - 1);
        let mut next = self.cache.back().map_or(0, |p| p.index + 1);
        while next <= upto {
            load(self, next, check)?;
            next += 1;
        }
        let h = &self.parsed.head.header;
        let w = h.width as usize;
        let sampled = h.components.len() == 3
            && h.components[0].horizontal_sampling != h.components[1].horizontal_sampling;
        let mut chroma = core::mem::take(&mut self.chroma);
        if sampled {
            for (k, c) in chroma.iter_mut().enumerate() {
                c.try_reserve(w.saturating_sub(c.len()))
                    .map_err(|_| Error::TooLarge)?;
                c.resize(w, 0f32);
                let i = k + 1;
                let native_h = h.component_size(i).1;
                let near = self.y / 2;
                let far = if self.y.is_multiple_of(2) {
                    near.saturating_sub(1)
                } else {
                    (near + 1).min(native_h - 1)
                };
                crate::lossy::jpeg::triangle_row(self.row(i, near), self.row(i, far), c);
            }
        }
        let yrow = self.row(0, self.y);
        let matrix = if h.color == ComponentColor::Gray {
            [[1., 0., 0., 128.], [0.; 4], [0.; 4]]
        } else {
            [
                [1., 0., 1.402, 128.],
                [1., -0.344136, -0.714136, 128.],
                [1., 1.772, 0., 128.],
            ]
        };
        for x in 0..w {
            let values = if h.color == ComponentColor::Gray {
                [yrow[x], 0., 0.]
            } else if sampled {
                [yrow[x], chroma[0][x], chroma[1][x]]
            } else {
                [yrow[x], self.row(1, self.y)[x], self.row(2, self.y)[x]]
            };
            let rgb = crate::lossy::color::inverse_linear_px(&matrix, [1.; 3], values);
            if h.color == ComponentColor::Gray {
                out[x] = rgb[0];
            } else {
                out[x * 3..x * 3 + 3].copy_from_slice(&rgb);
            }
        }
        self.chroma = chroma;
        self.y += 1;
        check()?;
        Ok(true)
    }
    pub(crate) fn next_row(&mut self, out: &mut [u8]) -> Result<bool, Error> {
        self.next_row_checked(out, &|| Ok(()))
    }
}

pub(crate) fn decode(
    parsed: NativeParsed<'_>,
    out: &mut [u8],
    stride: usize,
    check: &impl Fn() -> Result<(), Error>,
) -> Result<ImageInfo, Error> {
    let info = parsed.head.info.clone();
    let n = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("native output layout"))?;
    if out.len() < n {
        return Err(Error::InvalidInput("native output buffer"));
    }
    let mut stream = NativeStream::new(parsed)?;
    for y in 0..info.height as usize {
        stream.next_row_checked(&mut out[y * stride..y * stride + info.row_bytes()], check)?;
    }
    Ok(info)
}

/// [`decode`] reporting each band to a live `pulse` (one child per band,
/// `crate::progress::UnitProgress::plan_bands`).
pub(crate) fn decode_observed(
    parsed: NativeParsed<'_>,
    out: &mut [u8],
    stride: usize,
    check: &impl Fn() -> Result<(), Error>,
    pulse: &dyn how_far::Pulse,
) -> Result<ImageInfo, Error> {
    let info = parsed.head.info.clone();
    let n = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("native output layout"))?;
    if out.len() < n {
        return Err(Error::InvalidInput("native output buffer"));
    }
    let h = info.height as usize;
    let band_rows = parsed.head.header.pixel_band_rows(parsed.head.rows).max(1);
    let bands = h.div_ceil(band_rows);
    let progress = crate::progress::UnitProgress::plan_bands(
        pulse,
        (0..bands).map(|b| (b * band_rows, band_rows.min(h - b * band_rows))),
        info.width as usize,
    );
    let mut stream = NativeStream::new(parsed)?;
    for y in 0..h {
        let band = y / band_rows;
        if let Some(p) = &progress
            && y % band_rows == 0
        {
            if band > 0 {
                p.end(band - 1, &Ok(()));
            }
            if let Err(e) = p.begin(band) {
                p.end(band, &Err(e.clone()));
                return Err(e);
            }
        }
        let row =
            stream.next_row_checked(&mut out[y * stride..y * stride + info.row_bytes()], check);
        if let Err(e) = row {
            if let Some(p) = &progress {
                p.end(band, &Err(e.clone()));
            }
            return Err(e);
        }
    }
    if let Some(p) = &progress
        && bands > 0
    {
        p.end(bands - 1, &Ok(()));
    }
    Ok(info)
}

pub(crate) fn decode_region(
    head: &NativeHead,
    first: usize,
    units: &[&[u8]],
    rect: (usize, usize, usize, usize),
    out: &mut [u8],
    stride: usize,
) -> Result<(), Error> {
    let mut bands = Vec::new();
    bands
        .try_reserve_exact(head.bands())
        .map_err(|_| Error::TooLarge)?;
    bands.resize(head.bands(), &[][..]);
    for (i, &b) in units.iter().enumerate() {
        bands[first + i] = b;
        carriers(head, first + i, b)?;
    }
    let parsed = NativeParsed {
        head: head.clone(),
        bands,
        payload_len: 0,
    };
    let mut stream = NativeStream::new(parsed)?;
    stream.y = rect.1;
    stream.load(first, &|| Ok(()))?;
    let mut row = Vec::new();
    row.try_reserve_exact(head.info.row_bytes())
        .map_err(|_| Error::TooLarge)?;
    row.resize(head.info.row_bytes(), 0);
    let bpp = head.info.bytes_per_pixel();
    for y in 0..rect.3 {
        stream.next_row(&mut row)?;
        out[y * stride..y * stride + rect.2 * bpp]
            .copy_from_slice(&row[rect.0 * bpp..(rect.0 + rect.2) * bpp]);
    }
    Ok(())
}

#[cfg(feature = "std")]
pub(crate) async fn read_native<R: crate::read::input::Input>(
    src: &mut crate::read::Src<R>,
    mut head: NativeHead,
    admission: &impl Admission,
    on_row: &mut (impl crate::read::input::Rows + ?Sized),
) -> Result<(), Error> {
    src.begin_records(head.hint.take())?;
    let info = head.info.clone();
    let count = head.bands();
    let band_rows = head.header.pixel_band_rows(head.rows);
    let parsed = NativeParsed {
        head,
        bands: Vec::new(),
        payload_len: 0,
    };
    let mut stream = NativeStream::new(parsed)?;
    let mut payload = Vec::new();
    let mut row = Vec::new();
    row.try_reserve_exact(info.row_bytes())
        .map_err(|_| Error::TooLarge)?;
    row.resize(info.row_bytes(), 0);
    for band in 0..count {
        admission.check()?;
        src.record_admitted_async(&mut payload, records::R_NATIVE, &|n| {
            admission.native_buffer(n)
        })
        .await?;
        // All carrier geometry/models are validated before this band's rows.
        let cs = carriers(&stream.parsed.head, band, &payload)?;
        for (i, c) in cs.iter().enumerate() {
            let h = &stream.parsed.head.header;
            let p = h.components[i];
            let (_, n) = h.band_grid(band, stream.parsed.head.rows, i);
            crate::lossy::parse_native_component_checked(
                c,
                p.blocks_wide * 8,
                n as u32 * 8,
                h.tables[p.quant_table as usize].as_ref().unwrap(),
                &crate::parse::limits::Checked(admission),
            )?;
        }
        while stream.cache.front().is_some_and(|p| p.index + 2 < band) {
            stream.cache.pop_front();
        }
        stream.push_payload(band, &payload, &|| admission.check())?;
        if band > 0 {
            let end = (band * band_rows).min(info.height as usize);
            while stream.y < end {
                stream.next_row_checked(&mut row, &|| admission.check())?;
                on_row.row(&info, stream.y as u32 - 1, &row).await?;
            }
        }
    }
    while stream.y < info.height as usize {
        stream.next_row_checked(&mut row, &|| admission.check())?;
        on_row.row(&info, stream.y as u32 - 1, &row).await?;
    }
    Ok(())
}

#[cfg(feature = "std")]
pub(crate) fn decode_threads(
    parsed: NativeParsed<'_>,
    out: &mut [u8],
    stride: usize,
    threads: usize,
) -> Result<ImageInfo, Error> {
    let workers = crate::par::worker_count(threads, parsed.head.bands());
    if workers < 2 {
        return decode(parsed, out, stride, &|| Ok(()));
    }
    let info = parsed.head.info.clone();
    let extent = info
        .checked_stride_len(stride)
        .ok_or(Error::InvalidInput("native output layout"))?;
    if out.len() < extent {
        return Err(Error::InvalidInput("native output buffer"));
    }
    let rows = parsed.head.header.pixel_band_rows(parsed.head.rows);
    let bands_per_worker = parsed.head.bands().div_ceil(workers);
    let worker_rows = rows.checked_mul(bands_per_worker).ok_or(Error::TooLarge)?;
    let chunk = worker_rows.checked_mul(stride).ok_or(Error::TooLarge)?;
    let mut items = Vec::new();
    items
        .try_reserve_exact(workers)
        .map_err(|_| Error::TooLarge)?;
    for (i, dst) in out[..extent].chunks_mut(chunk).enumerate() {
        items.push((i * worker_rows, dst));
    }
    for result in crate::par::par_map(items, workers, |_, (y, dst)| {
        let height = worker_rows.min(info.height as usize - y);
        let first = (y / rows).saturating_sub(1);
        let end = ((y + height).div_ceil(rows) + 1).min(parsed.head.bands());
        decode_region(
            &parsed.head,
            first,
            &parsed.bands[first..end],
            (0, y, info.width as usize, height),
            dst,
            stride,
        )
    }) {
        result?;
    }
    Ok(info)
}

/// Native component rows with immutable owned compressed input.
pub(crate) struct OwnedNative {
    source: Vec<u8>,
    spans: Vec<core::ops::Range<usize>>,
    stream: NativeStream<'static>,
}
impl OwnedNative {
    pub(crate) fn new(
        source: Vec<u8>,
        head: NativeHead,
        spans: Vec<core::ops::Range<usize>>,
        payload_len: usize,
    ) -> Result<Self, Error> {
        Ok(Self {
            source,
            spans,
            stream: NativeStream::new(NativeParsed {
                head,
                bands: Vec::new(),
                payload_len,
            })?,
        })
    }
    pub(crate) fn next_row_checked(
        &mut self,
        out: &mut [u8],
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<bool, Error> {
        let Self {
            source,
            spans,
            stream,
        } = self;
        stream.next_row_from(out, check, |s, band, check| {
            s.push_payload(band, &source[spans[band].clone()], check)
        })
    }
}
