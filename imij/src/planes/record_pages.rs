//! Direct record pages: one small root, bounded independently checked leaves.
use super::*;
use crate::{ByteReader, HashAlgorithm, Hasher, put_varint};

pub(super) const RECORD_TAG: [u8; 4] = *b"pREC";
pub(super) const PAGE_TAG: [u8; 4] = *b"pIPG";
pub(super) const MAX_PAGE_BYTES: usize = 64 << 10;

#[derive(Clone)]
pub(super) struct Page {
    pub data_offset: u64,
    pub data_bytes: u64,
    pub offset: u64,
    pub len: u64,
    pub digest: [u8; 16],
}
pub(super) struct RecordPages {
    pub packed_lengths: bool,
    pub count: u32,
    pub per_page: u32,
    pub data_end: u64,
    pub pages: Vec<Page>,
    // Length declarations preserve the immutable attachment_len API. Page
    // lookups validate them; these are not a second offset authority.
    pub(super) lengths: Vec<(u32, u64)>,
}
fn valid_count(n: u32) -> bool {
    (16..=4096).contains(&n) && n.is_power_of_two()
}
impl RecordPages {
    pub fn check_page_records(n: u32) -> Result<(), Error> {
        if !valid_count(n) {
            return Err(Error::InvalidInput(
                "record page count must be a power of two in 16..4096",
            ));
        }
        Ok(())
    }
    pub fn attachment_len(&self, id: u32) -> Option<u64> {
        self.lengths
            .binary_search_by_key(&id, |r| r.0)
            .ok()
            .map(|i| self.lengths[i].1)
    }
    pub fn encode(&self, limits: PlaneSetLimits) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        out.try_reserve_exact(40 + self.pages.len() * 36 + self.lengths.len() * 15)
            .map_err(|_| Error::TooLarge)?;
        for n in [
            if self.packed_lengths { 2 } else { 1 },
            u64::from(self.count),
            u64::from(self.per_page),
            self.data_end,
        ] {
            put_varint(&mut out, n)
        }
        for p in &self.pages {
            put_varint(&mut out, p.data_bytes);
            put_varint(&mut out, p.len);
            out.extend_from_slice(&p.digest)
        }
        put_varint(&mut out, self.lengths.len() as u64);
        for &(id, len) in &self.lengths {
            put_varint(&mut out, id.into());
            put_varint(&mut out, len)
        }
        if out.len() > limits.max_index_bytes {
            return Err(Error::TooLarge);
        }
        Ok(out)
    }
    pub fn read(
        raw: &[u8],
        primary_len: u64,
        index_offset: u64,
        limits: PlaneSetLimits,
    ) -> Result<Self, Error> {
        let mut r = ByteReader::new(raw);
        let packed_lengths = match r.varint()? {
            1 => false,
            2 => true,
            _ => return Err(Error::Unsupported("plane record page schema")),
        };
        let count = r.var_u32()?;
        let per_page = r.var_u32()?;
        let data_end = r.varint()?;
        if !valid_count(per_page) || count == 0 {
            return Err(Error::Corrupt("plane record page geometry"));
        }
        if count as usize > limits.max_records {
            return Err(Error::TooLarge);
        }
        let n = count.div_ceil(per_page) as usize;
        if n.checked_mul(core::mem::size_of::<Page>())
            .is_none_or(|v| v > limits.max_index_bytes)
        {
            return Err(Error::TooLarge);
        }
        if n > r.remaining() / 18 {
            return Err(Error::Corrupt("plane page count exceeds root"));
        }
        if data_end < primary_len || data_end >= index_offset {
            return Err(Error::Corrupt("plane page data bounds"));
        }
        let mut pages = Vec::new();
        pages.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        let (mut data_offset, mut offset) = (primary_len, data_end);
        for i in 0..n {
            let data_bytes = r.varint()?;
            let len = r.varint()?;
            let digest = r.bytes(16)?.try_into().unwrap();
            let records =
                (count - u32::try_from(i).map_err(|_| Error::TooLarge)? * per_page).min(per_page);
            if data_bytes < u64::from(records) * 6 || len > MAX_PAGE_BYTES as u64 || len == 0 {
                return Err(Error::Corrupt("plane page extent"));
            }
            pages.push(Page {
                data_offset,
                data_bytes,
                offset,
                len,
                digest,
            });
            data_offset = data_offset.checked_add(data_bytes).ok_or(Error::TooLarge)?;
            offset = offset
                .checked_add(wire::chunk_len(len)?)
                .ok_or(Error::TooLarge)?;
            if data_offset > data_end || offset > index_offset {
                return Err(Error::Corrupt("plane page root bounds"));
            }
        }
        if data_offset != data_end || offset != index_offset {
            return Err(Error::Corrupt("plane page root coverage"));
        }
        let n = r.var_usize()?;
        if n > limits.max_attachments {
            return Err(Error::TooLarge);
        }
        if n > r.remaining() / 2 {
            return Err(Error::Corrupt("attachment lengths exceed page root"));
        }
        let mut lengths = Vec::new();
        lengths.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        for _ in 0..n {
            let id = r.var_u32()?;
            let len = r.varint()?;
            if id >= count || lengths.last().is_some_and(|&(prev, _)| prev >= id) {
                return Err(Error::Corrupt("plane page attachment identity"));
            }
            if len > limits.max_attachment_bytes {
                return Err(Error::TooLarge);
            }
            lengths.push((id, len));
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("plane page root trailing bytes"));
        }
        Ok(Self {
            packed_lengths,
            count,
            per_page,
            data_end,
            pages,
            lengths,
        })
    }
    pub fn validate_attachments(&self, attachments: &[AttachmentDescriptor]) -> Result<(), Error> {
        if self.lengths.len() != attachments.len()
            || attachments
                .iter()
                .any(|a| self.attachment_len(a.record).is_none())
        {
            return Err(Error::Corrupt("plane page attachment length declarations"));
        }
        Ok(())
    }
    fn digest(&self, page: usize, packed: &[u8]) -> Result<[u8; 16], Error> {
        let p = self
            .pages
            .get(page)
            .ok_or(Error::Corrupt("plane page ID"))?;
        let first = page as u64 * u64::from(self.per_page);
        let count = (u64::from(self.count) - first).min(self.per_page.into());
        page_digest(first, count, p, packed)
    }

    pub fn decode_page(
        &self,
        page: usize,
        framed: &[u8],
        limits: PlaneSetLimits,
    ) -> Result<Vec<Record>, Error> {
        let p = self
            .pages
            .get(page)
            .ok_or(Error::Corrupt("plane page ID"))?;
        let packed = wire::payload(framed, &PAGE_TAG)?;
        if packed.len() as u64 != p.len || self.digest(page, packed)? != p.digest {
            return Err(Error::Corrupt("plane page checksum"));
        }
        let first = page as u64 * u64::from(self.per_page);
        let count = (u64::from(self.count) - first).min(self.per_page.into()) as usize;
        let raw = crate::metadata_pack::unpack(packed, MAX_PAGE_BYTES)?;
        let mut r = ByteReader::new(&raw);
        if r.varint()? != first || r.var_usize()? != count {
            return Err(Error::Corrupt("plane page record identity"));
        }
        let first_len = r.varint()?;
        let compact = if first_len == 0 && self.packed_lengths {
            let minimum = r.varint()?;
            let width = u32::from(r.u8()?);
            if minimum == 0 || width > 32 {
                return Err(Error::Corrupt("packed plane lengths"));
            }
            let bits = count * width as usize;
            let raw_bits = r.bytes(bits.div_ceil(8))?;
            if !bits.is_multiple_of(8)
                && raw_bits.last().is_some_and(|last| last >> (bits % 8) != 0)
            {
                return Err(Error::Corrupt("packed plane length padding"));
            }
            Some((minimum, width, crate::bits::BitReader::new(raw_bits)))
        } else {
            None
        };
        let mut records = Vec::new();
        records
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        let mut offset = p.data_offset;
        let mut compact = compact;
        for i in 0..count {
            let len = match &mut compact {
                Some((minimum, width, bits)) => minimum
                    .checked_add(bits.read_refill(*width).into())
                    .ok_or(Error::TooLarge)?,
                None if i == 0 => first_len,
                None => r.varint()?,
            };
            let id = first + i as u64;
            let raw_len = len
                .checked_sub(crate::varint_len(id) as u64)
                .ok_or(Error::Corrupt("plane bound record length"))?;
            if self.attachment_len(id as u32).is_some_and(|v| v != raw_len) {
                return Err(Error::Corrupt("plane attachment length disagreement"));
            }
            if raw_len > limits.max_record_bytes.max(limits.max_attachment_bytes) {
                return Err(Error::TooLarge);
            }
            records.push(Record { offset, len });
            offset = offset
                .checked_add(wire::chunk_len(len)?)
                .ok_or(Error::TooLarge)?;
            if offset > p.data_offset + p.data_bytes {
                return Err(Error::Corrupt("plane page record bounds"));
            }
        }
        if r.remaining() != 0 || offset != p.data_offset + p.data_bytes {
            return Err(Error::Corrupt("plane page record coverage"));
        }
        Ok(records)
    }
    /// Producer records already have bound IDs in their pREC payloads.
    pub fn build(
        dir: &Directory,
        per_page: u32,
        data_end: u64,
        limits: PlaneSetLimits,
    ) -> Result<(Self, Vec<u8>), Error> {
        Self::check_page_records(per_page)?;
        let count = u32::try_from(dir.records.len()).map_err(|_| Error::TooLarge)?;
        if count == 0 {
            return Err(Error::InvalidInput("paged record directory is empty"));
        }
        let mut pages = Vec::new();
        let mut encoded = Vec::new();
        let mut payloads = Vec::new();
        let n = dir.records.len().div_ceil(per_page as usize);
        pages.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        payloads.try_reserve_exact(n).map_err(|_| Error::TooLarge)?;
        let mut offset = data_end;
        let mut packed_lengths = false;
        for (i, records) in dir.records.chunks(per_page as usize).enumerate() {
            let first = i * per_page as usize;
            let (packed, compact) =
                pack_lengths(first as u64, records.iter().map(|record| record.len), true)?;
            packed_lengths |= compact;
            if packed.len() > MAX_PAGE_BYTES {
                return Err(Error::TooLarge);
            }
            let end = records
                .last()
                .unwrap()
                .offset
                .checked_add(wire::chunk_len(records.last().unwrap().len)?)
                .ok_or(Error::TooLarge)?;
            pages.push(Page {
                data_offset: records[0].offset,
                data_bytes: end - records[0].offset,
                offset,
                len: packed.len() as u64,
                digest: [0; 16],
            });
            offset = offset
                .checked_add(wire::chunk_len(packed.len() as u64)?)
                .ok_or(Error::TooLarge)?;
            payloads.push(packed);
        }
        let mut lengths = Vec::new();
        lengths
            .try_reserve_exact(dir.attachments.len())
            .map_err(|_| Error::TooLarge)?;
        for a in &dir.attachments {
            let record = dir
                .records
                .get(a.record as usize)
                .ok_or(Error::Corrupt("plane attachment record ID"))?;
            let raw = record
                .len
                .checked_sub(crate::varint_len(a.record as u64) as u64)
                .ok_or(Error::Corrupt("plane attachment bound length"))?;
            lengths.push((a.record, raw));
        }
        lengths.sort_unstable_by_key(|v| v.0);
        let mut value = Self {
            packed_lengths,
            count,
            per_page,
            data_end,
            pages,
            lengths,
        };
        for (i, packed) in payloads.iter().enumerate() {
            value.pages[i].digest = value.digest(i, packed)?;
            wire::chunk(&mut encoded, &PAGE_TAG, packed)?;
            if encoded.len() as u64 > limits.max_file_bytes {
                return Err(Error::TooLarge);
            }
        }
        Ok((value, encoded))
    }
}

pub(super) fn pack_lengths(
    first: u64,
    lengths: impl ExactSizeIterator<Item = u64> + Clone,
    allow_compact: bool,
) -> Result<(Vec<u8>, bool), Error> {
    let count = lengths.len();
    let mut raw = Vec::new();
    raw.try_reserve_exact(32 + count * 10)
        .map_err(|_| Error::TooLarge)?;
    put_varint(&mut raw, first);
    put_varint(&mut raw, count as u64);
    let prefix = raw.len();
    let (mut min, mut max) = (u64::MAX, 0);
    for len in lengths.clone() {
        min = min.min(len);
        max = max.max(len);
        put_varint(&mut raw, len);
    }
    let plain = crate::metadata_pack::pack(&raw)?;
    if allow_compact && count != 0 {
        let width = 64 - (max - min).leading_zeros();
        if width <= 32 {
            raw.truncate(prefix);
            raw.push(0);
            put_varint(&mut raw, min);
            raw.push(width as u8);
            let mut bits =
                crate::bits::BitWriter::try_with_capacity((count * width as usize).div_ceil(8))?;
            for len in lengths {
                bits.put((len - min) as u32, width);
            }
            raw.extend_from_slice(&bits.finish());
            let compact = crate::metadata_pack::pack(&raw)?;
            if compact.len() < plain.len() {
                return Ok((compact, true));
            }
        }
    }
    Ok((plain, false))
}

pub(super) fn page_digest(
    first: u64,
    count: u64,
    p: &Page,
    packed: &[u8],
) -> Result<[u8; 16], Error> {
    let mut context = Vec::new();
    context.try_reserve_exact(40).map_err(|_| Error::TooLarge)?;
    for n in [first, count, p.data_offset, p.data_bytes] {
        put_varint(&mut context, n)
    }
    let mut hash = Hasher::new(HashAlgorithm::Xxh3_128);
    hash.update(b"IMI/v4/plane-record-page\0");
    hash.update(&context);
    hash.update(packed);
    Ok(hash.finish().as_bytes().try_into().unwrap())
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn lookup(raw: &[u8], packed_lengths: bool) -> Result<Vec<Record>, Error> {
        let packed = crate::metadata_pack::pack(raw)?;
        let mut page = Page {
            data_offset: 100,
            data_bytes: 25,
            offset: 125,
            len: packed.len() as u64,
            digest: [0; 16],
        };
        page.digest = page_digest(0, 2, &page, &packed)?;
        let root = RecordPages {
            packed_lengths,
            count: 2,
            per_page: 16,
            data_end: 125,
            pages: alloc::vec![page],
            lengths: Vec::new(),
        };
        let mut framed = Vec::new();
        wire::chunk(&mut framed, &PAGE_TAG, &packed)?;
        root.decode_page(0, &framed, PlaneSetLimits::default())
    }

    #[test]
    fn packed_lengths_validate_admitted_width_padding_and_coverage() {
        let result = lookup(&[0, 2, 0, 7, 1, 2], true).unwrap();
        assert_eq!((result[0].offset, result[0].len), (100, 7));
        assert_eq!((result[1].offset, result[1].len), (112, 8));
        assert!(lookup(&[0, 2, 0, 7, 1, 2], false).is_err());
        assert!(lookup(&[0, 2, 0, 0, 1, 2], true).is_err());
        assert!(lookup(&[0, 2, 0, 7, 33], true).is_err());
        assert!(lookup(&[0, 2, 0, 7, 1, 0x82], true).is_err());
        assert!(lookup(&[0, 2, 0, 7, 1], true).is_err());
        assert!(lookup(&[0, 2, 0, 7, 1, 0, 0], true).is_err());
    }
}
