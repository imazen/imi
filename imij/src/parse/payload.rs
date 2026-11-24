//! The `DATA` payload's head (SPEC 2, 2.8): the input it is read from
//! and the head grammar the header's coding selects.
//!
//! The lossless, lossy and native head parsers share [`AsyncHeadInput`]
//! grammar. [`ReadyInput`] adapts [`HeadInput`] for in-memory payloads
//! ([`ByteReader`]) and blocking streams ([`crate::read`]). A suspendable
//! input retains the same parser future: no grammar copy or repeated table
//! parsing is needed to find the head's end.

use crate::parse::header::{Coding, Header};
use crate::{ByteReader, Error};

/// The bytes a payload head is read from. A slice [`HeadInput::bytes`]
/// returns borrows the input until its next read.
pub(crate) trait HeadInput {
    fn u8(&mut self) -> Result<u8, Error>;
    fn peek_u8(&mut self) -> Result<u8, Error>;
    fn u16(&mut self) -> Result<u16, Error>;
    fn u32(&mut self) -> Result<u32, Error>;
    fn varint(&mut self) -> Result<u64, Error>;
    fn bytes(&mut self, n: usize) -> Result<&[u8], Error>;
    /// The position for the codestream trace (lab).
    fn mark(&self) -> crate::trace::Mark;
    /// Bytes read so far (from the start of a slice; a stream's position).
    fn pos(&self) -> u64;
}

/// Suspendable reads for the shared DATA-head grammar. `Pending` means only
/// temporary input exhaustion; implementations report explicit EOF as an error.
/// No executor or I/O runtime is required by the parser.
pub(crate) trait AsyncHeadInput {
    async fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1).await?[0])
    }
    async fn peek_u8(&mut self) -> Result<u8, Error>;
    async fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.bytes(2).await?.try_into().unwrap()))
    }
    async fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.bytes(4).await?.try_into().unwrap()))
    }
    async fn varint(&mut self) -> Result<u64, Error> {
        // Retain partial bytes in the future, then use the canonical decoder.
        // The tenth byte always finishes validation (including overflow).
        let mut bytes = [0; 10];
        for i in 0..bytes.len() {
            let b = self.u8().await?;
            bytes[i] = b;
            if b < 0x80 || i == 9 {
                let mut iter = bytes[..=i].iter();
                return crate::read_varint(|| Ok(*iter.next().unwrap()));
            }
        }
        unreachable!("ten bytes always finish a u64 varint")
    }
    async fn bytes(&mut self, n: usize) -> Result<&[u8], Error>;
    fn mark(&self) -> crate::trace::Mark;
    fn pos(&self) -> u64;

    async fn var_usize(&mut self) -> Result<usize, Error> {
        usize::try_from(self.varint().await?).map_err(|_| Error::TooLarge)
    }
}

/// Existing slice and blocking-reader inputs always complete in one poll.
pub(crate) struct ReadyInput<'a, I>(pub(crate) &'a mut I);

impl<I: HeadInput> AsyncHeadInput for ReadyInput<'_, I> {
    async fn u8(&mut self) -> Result<u8, Error> {
        self.0.u8()
    }
    async fn peek_u8(&mut self) -> Result<u8, Error> {
        self.0.peek_u8()
    }
    async fn u16(&mut self) -> Result<u16, Error> {
        self.0.u16()
    }
    async fn u32(&mut self) -> Result<u32, Error> {
        self.0.u32()
    }
    async fn varint(&mut self) -> Result<u64, Error> {
        self.0.varint()
    }
    async fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        self.0.bytes(n)
    }
    fn mark(&self) -> crate::trace::Mark {
        self.0.mark()
    }
    fn pos(&self) -> u64 {
        self.0.pos()
    }
}

/// Drive a parser or row driver instantiated with always-ready adapters. This does not block on
/// `Pending` or restart the parser; such a result is an internal adapter bug.
pub(crate) fn ready<T>(future: impl core::future::Future<Output = T>) -> T {
    use core::task::{Context, Poll, Waker};
    match core::pin::pin!(future)
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("synchronous codec adapter suspended"),
    }
}

impl HeadInput for ByteReader<'_> {
    fn peek_u8(&mut self) -> Result<u8, Error> {
        self.data
            .get(self.pos)
            .copied()
            .ok_or(Error::Corrupt("truncated"))
    }
    #[inline]
    fn u8(&mut self) -> Result<u8, Error> {
        ByteReader::u8(self)
    }
    #[inline]
    fn u16(&mut self) -> Result<u16, Error> {
        ByteReader::u16(self)
    }
    #[inline]
    fn u32(&mut self) -> Result<u32, Error> {
        ByteReader::u32(self)
    }
    #[inline]
    fn varint(&mut self) -> Result<u64, Error> {
        ByteReader::varint(self)
    }
    #[inline]
    fn bytes(&mut self, n: usize) -> Result<&[u8], Error> {
        ByteReader::bytes(self, n)
    }
    #[inline]
    fn mark(&self) -> crate::trace::Mark {
        ByteReader::mark(self)
    }
    #[inline]
    fn pos(&self) -> u64 {
        self.pos as u64
    }
}

/// A payload's head: the lossless head (models, geometry), or the lossy one
/// (tables and the nested images' heads). No records yet. Boxed: the heads
/// are large and pass through several calls before a driver takes them.
pub(crate) enum Head {
    #[cfg(feature = "native-jpeg")]
    Native(alloc::boxed::Box<crate::jpeg::wire::NativeHead>),
    Lossless(alloc::boxed::Box<crate::v1::Parsed<'static>>),
    Lossy(alloc::boxed::Box<crate::lossy::LossyParsed<'static>>),
}

impl Head {
    /// Where its main records sit, and its stripe count (SPEC 2.7).
    pub(crate) fn units(&self) -> (crate::parse::records::UnitMap, usize) {
        match self {
            #[cfg(feature = "native-jpeg")]
            Head::Native(p) => (crate::parse::records::UnitMap::native(), p.bands()),
            Head::Lossless(p) => (
                crate::parse::records::UnitMap::lossless(p.ngroups()),
                p.stripe_count(),
            ),
            Head::Lossy(p) => (p.unit_map(), p.stripe_count()),
        }
    }

    /// The unit table hint, taken out of the head.
    pub(crate) fn take_hint(&mut self) -> Option<alloc::vec::Vec<u64>> {
        match self {
            #[cfg(feature = "native-jpeg")]
            Head::Native(p) => p.hint.take(),
            Head::Lossless(p) => p.hint.take(),
            Head::Lossy(p) => p.hint.take(),
        }
    }

    /// The unit table reservation (head record 12), if any.
    pub(crate) fn reserve(&self) -> Option<crate::parse::records::Reserve> {
        match self {
            #[cfg(feature = "native-jpeg")]
            Head::Native(p) => p.reserve,
            Head::Lossless(p) => p.reserve,
            Head::Lossy(p) => p.reserve,
        }
    }

    /// Whether main unit records carry 0-based ordinals (head record 37,
    /// SPEC 2.7).
    pub(crate) fn unit_ordinals(&self) -> bool {
        match self {
            #[cfg(feature = "native-jpeg")]
            Head::Native(_) => false,
            Head::Lossless(p) => p.unit_ordinals,
            Head::Lossy(p) => p.unit_ordinals,
        }
    }
}

/// Reads the head of the `DATA` payload `input` starts at: the header's
/// coding selects the head grammar and carries the lossy internal color
/// (SPEC 2.8: the payload repeats neither); `declared`: the header's
/// feature mask.
pub(crate) fn read_head<I: HeadInput>(
    header: &Header,
    declared: crate::parse::header::Features,
    input: &mut I,
    admission: &impl crate::parse::limits::Admission,
) -> Result<Head, Error> {
    ready(read_head_async(
        header,
        declared,
        &mut ReadyInput(input),
        admission,
    ))
}

pub(crate) async fn read_head_async<I: AsyncHeadInput>(
    header: &Header,
    declared: crate::parse::header::Features,
    input: &mut I,
    admission: &impl crate::parse::limits::Admission,
) -> Result<Head, Error> {
    if header.version == 4 {
        return Err(Error::Unsupported(
            "V4 requires its declared payload driver",
        ));
    }
    let info = header.info.clone();
    if header.lossy_color == Some(crate::LossyColor::YCoCg) && input.peek_u8().await? == 0 {
        if !declared.contains(crate::Feature::NativeComponents) {
            return Err(Error::Corrupt("bad stripe height"));
        }
        if header.version != crate::parse::header::HEADER_VERSION_2 {
            return Err(Error::Unsupported("native components retired in v3"));
        }
        #[cfg(not(feature = "native-jpeg"))]
        return Err(Error::Unsupported("feature native_components"));
        #[cfg(feature = "native-jpeg")]
        return Ok(Head::Native(alloc::boxed::Box::new(
            crate::jpeg::wire::read_head_async(info, input, admission).await?,
        )));
    }
    Ok(match (header.coding, header.lossy_color) {
        (Coding::Lossless, _) => Head::Lossless(alloc::boxed::Box::new(
            crate::v1::parse_head_async(info, input, admission, declared).await?,
        )),
        (Coding::Lossy, Some(color)) => Head::Lossy(alloc::boxed::Box::new(
            crate::lossy::parse_lossy_head_async(info, color, declared, input, admission).await?,
        )),
        (Coding::Lossy, None) => return Err(Error::Corrupt("lossy coding without a color")),
    })
}

/// A trailing unit index (SPEC 2.7) in the last chunk of a file (`payload`:
/// its payload; `chunk_len`: its whole length) after the `DATA` payload
/// `data`: when the head says check tail and the footer names the chunk,
/// the index is checked as a hint is, against every record (`Corrupt` on a
/// mismatch); otherwise there is no index and nothing is checked.
pub(crate) fn check_trailing_index(
    header: &Header,
    declared: crate::parse::header::Features,
    data: &[u8],
    payload: &[u8],
    chunk_len: u64,
) -> Result<(), Error> {
    use crate::parse::records;
    let mut r = ByteReader::new(data);
    let head = read_head(header, declared, &mut r, &crate::parse::limits::Structure)?;
    if !head.reserve().is_some_and(|x| x.check_tail()) {
        return Ok(());
    }
    let Some(index) = records::index_footer(payload, chunk_len) else {
        return Ok(());
    };
    let (map, stripes) = head.units();
    let n = map
        .count(stripes)
        .map_err(|_| Error::Corrupt("unit table length"))?;
    let entries = records::index_entries(index, data.len() as u64, r.pos as u64, n)?;
    map.spans_with_ordinals(
        &mut r,
        header.version,
        stripes,
        Some(entries),
        head.unit_ordinals(),
    )?;
    Ok(())
}

#[cfg(all(test, feature = "encode"))]
pub(crate) mod fragmented_tests;

/// Recognize the legacy local discriminator even in builds without its decoder.
pub(crate) fn native_selected(
    color: Option<crate::LossyColor>,
    declared: crate::Features,
    body: &[u8],
) -> Result<bool, Error> {
    let native = color == Some(crate::LossyColor::YCoCg) && body.first() == Some(&0);
    if native && !declared.contains(crate::Feature::NativeComponents) {
        return Err(Error::Corrupt("bad stripe height"));
    }
    if native && !cfg!(feature = "native-jpeg") {
        return Err(Error::Unsupported("feature native_components"));
    }
    Ok(native)
}
