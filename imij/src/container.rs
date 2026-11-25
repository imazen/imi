//! Chunks (SPEC 1.1): writing the header and the chunks before `DATA`,
//! editing finished files (the DATA locator, header extensions, inserted
//! chunks), and the chunk walk of in-memory files. The header itself is
//! read in [`crate::parse::header`].
//!
//! A chunk is a tag (4 bytes), `varint(len + 1)` and the payload. Length 0
//! marks an open chunk whose payload delimits itself (only critical chunks
//! may be open; `DATA` is one, so a writer can stream the pixel data without
//! knowing its size). Tag letter case, as PNG: first letter upper =
//! critical (a decoder that does not know it must fail), lower = ancillary
//! (skipped when unknown); second letter lower = private; fourth letter
//! lower = safe to copy into an edited file whose pixels changed.

use alloc::vec::Vec;

use crate::parse::header::{
    Coding, DecodeSpeed, EXT_LOCATOR, F_ALL, F_ALPHA, F_BITS, F_CHANNELS, F_CICP, F_CODING,
    F_FEATURES, F_ORIENT, F_SAMPLE, F_SPEED, Features, HEADER_VERSION, Header, Locator,
    ParsedHeader, SIGNATURE, implied_features, read_header, read_header_sized,
};
use crate::{ByteReader, Channels, Cicp, Error, ImageInfo, SampleType, put_varint};

/// An ISO 21496-1 gain map (`gMAP` chunk): the base image is the `DATA`
/// image; `image` turns it into the alternate rendition.
#[derive(Clone, Debug, PartialEq)]
pub struct GainMap {
    /// ISO 21496-1 gain map metadata (stored in its binary form, the bare
    /// payload JXL's `jhgm` also carries; zencodec parses and writes it).
    pub params: zencodec::GainMapParams,
    /// Color of the alternate rendition, when it differs from the base.
    pub alt_cicp: Option<Cicp>,
    pub alt_icc: Option<Vec<u8>>,
    /// The gain map image: a complete IMI file (any size, lossless or lossy).
    pub image: Vec<u8>,
}

/// A chunk the library does not interpret (written and read back as is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Ancillary tag: first letter lower case (see the module docs).
    pub tag: [u8; 4],
    pub data: Vec<u8>,
}

use crate::parse::chunks::{
    ANIM, CLLI, CSUM, DATA, DWHI, EXIF, FRAM, GMAP, ICCP, Kind, MAX_PACKED_METADATA, MCMP, MDCV,
    PRVW, Rules, XMP, is_critical,
};
// The science records (`cALB`, `aXES`, `cORD`, `sMDT`, `cATG`, SPEC 1.5) are not
// listed: the container carries them as ordinary ancillary chunks in
// `Metadata::chunks`, and `science::ScienceRecords` reads and writes them.
#[cfg_attr(not(any(feature = "encode", feature = "std")), allow(dead_code))]
const KNOWN: [[u8; 4]; 19] = [
    DATA,
    ICCP,
    EXIF,
    XMP,
    MDCV,
    CLLI,
    DWHI,
    PRVW,
    GMAP,
    ANIM,
    FRAM,
    MCMP,
    CSUM,
    *b"pDAT",
    *b"pIPG",
    *b"pREC",
    *b"pIDX",
    *b"sLOC",
    crate::pixelsum::PSUM,
];

/// A file's `cSUM` chunk, found by [`parse_checked`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct ChecksumChunk<'a> {
    /// File offset and length of the whole chunk.
    pub at: usize,
    pub len: usize,
    /// The pieces to check; None for an algorithm this decoder does not know.
    pub pieces: Option<crate::checksum::Pieces<'a>>,
}

impl ChecksumChunk<'_> {
    /// Whether a rewriter can recompute this checksum; rewriters refuse one
    /// they cannot rather than drop or copy it.
    pub(crate) fn rewritable(c: Option<ChecksumChunk<'_>>) -> Result<bool, Error> {
        match c {
            None => Ok(false),
            Some(ChecksumChunk { pieces: None, .. }) => {
                Err(Error::Unsupported("unknown file checksum algorithm"))
            }
            Some(_) => Ok(true),
        }
    }
}

pub(crate) fn put_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(tag);
    put_varint(out, data.len() as u64 + 1);
    out.extend_from_slice(data);
}

/// Writes signature, header (masked form: only fields that differ from
/// their defaults) and the metadata chunks; the caller appends the `DATA`
/// chunk (see [`put_data`]). `color` is the lossy internal color space and
/// must be `Some` exactly for lossy coding. Encoders write through
/// [`write_head_features`]; this form serves tests and lab tools.
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) fn write_head(
    out: &mut Vec<u8>,
    info: &ImageInfo,
    meta: &crate::Metadata,
    coding: Coding,
    color: Option<crate::LossyColor>,
    speed: DecodeSpeed,
) -> Result<(), Error> {
    write_head_impl(
        out,
        info,
        meta,
        coding,
        color,
        speed,
        Features::EMPTY,
        false,
    )
}

/// [`write_head`] for a file whose payload uses `features` (SPEC 0.1): they
/// go in the header's feature mask, with the sample type's own feature.
#[cfg(feature = "encode")]
pub(crate) fn write_head_features(
    out: &mut Vec<u8>,
    info: &ImageInfo,
    meta: &crate::Metadata,
    coding: Coding,
    color: Option<crate::LossyColor>,
    speed: DecodeSpeed,
    features: Features,
) -> Result<(), Error> {
    write_head_impl(out, info, meta, coding, color, speed, features, false)
}

/// [`write_head`] in the full form: every header field written explicitly.
/// Decoders accept it; encoders write the masked form.
#[cfg(all(test, feature = "encode"))]
pub(crate) fn write_full_head(
    out: &mut Vec<u8>,
    info: &ImageInfo,
    meta: &crate::Metadata,
    coding: Coding,
    color: Option<crate::LossyColor>,
    speed: DecodeSpeed,
) -> Result<(), Error> {
    write_head_impl(out, info, meta, coding, color, speed, Features::EMPTY, true)
}

#[cfg(feature = "encode")]
fn write_head_impl(
    out: &mut Vec<u8>,
    info: &ImageInfo,
    meta: &crate::Metadata,
    coding: Coding,
    color: Option<crate::LossyColor>,
    speed: DecodeSpeed,
    features: Features,
    full: bool,
) -> Result<(), Error> {
    write_head_form(
        out,
        info,
        meta,
        coding,
        color,
        speed,
        features,
        full,
        HEADER_VERSION,
        false,
    )
}

/// Bounded group producers do not require a resident whole-image raster.
/// Version 4 remains an explicit adapter, never the default.
#[cfg_attr(not(feature = "encode"), allow(dead_code))]
pub(crate) fn write_group_head_features(
    out: &mut Vec<u8>,
    info: &ImageInfo,
    meta: &crate::Metadata,
    coding: Coding,
    color: Option<crate::LossyColor>,
    speed: DecodeSpeed,
    features: Features,
    version: u8,
) -> Result<(), Error> {
    if version != HEADER_VERSION && version != 4 {
        return Err(Error::Unsupported("group header version"));
    }
    if version == 4 && features.bits() & (0x3f << 26) != 0 {
        return Err(Error::InvalidInput("reserved V4 syntax features"));
    }
    write_head_form(
        out, info, meta, coding, color, speed, features, false, version, true,
    )
}

fn write_head_form(
    out: &mut Vec<u8>,
    info: &ImageInfo,
    meta: &crate::Metadata,
    coding: Coding,
    color: Option<crate::LossyColor>,
    speed: DecodeSpeed,
    features: Features,
    full: bool,
    version: u8,
    bounded: bool,
) -> Result<(), Error> {
    // The sample type's feature (i8/i16, 64-bit) joins the declared ones;
    // what the coding and sample type imply needs no mask bit (SPEC 0.1).
    let features = match info.sample.required_feature() {
        Some(need) => features.with(need),
        None => features,
    }
    .difference(implied_features(coding, info.sample));
    if info.width == 0 || info.height == 0 {
        return Err(Error::InvalidInput("dimensions must be nonzero"));
    }
    // Native band producers do not allocate a whole-image pixel buffer.
    if bounded {
        if info.checked_buffer_len_u64().is_none() {
            return Err(Error::TooLarge);
        }
    } else if info.checked_buffer_len().is_none()
        && !features.contains(crate::Feature::NativeComponents)
    {
        return Err(Error::TooLarge);
    }
    info.validate_precision()?;
    if info.channels == crate::Channels::Scalar {
        if version < 3 {
            return Err(Error::Unsupported(
                "scalar channels require header version 3 or later",
            ));
        }
        if info.cicp != crate::Cicp::UNSPECIFIED {
            return Err(Error::InvalidInput("CICP on scalar channels"));
        }
    }
    let features = if info.channels.is_native_model() {
        if coding != Coding::Lossless {
            return Err(Error::Unsupported("pixel model requires lossless coding"));
        }
        features.with(crate::Feature::PixelModels)
    } else {
        features
    };
    let alpha_mode = info.alpha_wire()?;
    let features = if alpha_mode >= 2 {
        features.with(crate::Feature::AlphaInterpretation)
    } else {
        features
    };
    let coding_byte = match (coding, color) {
        (Coding::Lossless, None) => 0u8,
        (Coding::Lossy, Some(crate::LossyColor::Xyb)) => 1,
        (Coding::Lossy, Some(crate::LossyColor::YCoCg)) => 2,
        _ => {
            return Err(Error::InvalidInput(
                "lossy color must be given exactly for lossy coding",
            ));
        }
    };
    let orient = match meta.orientation {
        0 => 1,
        o @ 1..=8 => o,
        _ => return Err(Error::InvalidInput("orientation must be 1..=8")),
    };
    // SPEC 1.0: the feature mask right after `present`, then the present
    // fields in bit order, then the dimensions.
    let present = if full {
        F_ALL
    } else {
        let mut p = 0;
        p |= u64::from(coding_byte != 0) * F_CODING;
        p |= u64::from(info.sample != SampleType::U8) * F_SAMPLE;
        p |= u64::from(!features.is_empty()) * F_FEATURES;
        p |= u64::from(info.channels != Channels::Rgb) * F_CHANNELS;
        p |= u64::from(orient != 1) * F_ORIENT;
        p |= u64::from(speed != DecodeSpeed::Balanced) * F_SPEED;
        p |= u64::from(info.cicp != info.channels.default_cicp()) * F_CICP;
        p |= u64::from(info.has_custom_precision()) * F_BITS;
        p |= u64::from(alpha_mode != 0) * F_ALPHA;
        p
    };
    out.extend_from_slice(&SIGNATURE);
    let mut h = Vec::with_capacity(24);
    h.push(version);
    put_varint(&mut h, present);
    if present & F_FEATURES != 0 {
        put_varint(&mut h, features.bits());
    }
    if present & F_CODING != 0 {
        h.push(coding_byte);
    }
    if present & F_SAMPLE != 0 {
        h.push(info.sample as u8);
    }
    if present & F_CHANNELS != 0 {
        h.push(info.channels as u8);
    }
    if present & F_ORIENT != 0 {
        h.push(orient);
    }
    if present & F_SPEED != 0 {
        h.push(speed as u8);
    }
    if present & F_CICP != 0 {
        h.extend_from_slice(&[
            info.cicp.color_primaries,
            info.cicp.transfer_characteristics,
            info.cicp.matrix_coefficients,
            info.cicp.full_range as u8,
        ]);
    }
    if present & F_BITS != 0 {
        for c in 0..info.channels.count() {
            h.push(info.channel_bits(c).unwrap());
        }
    }
    if present & F_ALPHA != 0 {
        h.push(alpha_mode);
    }
    put_varint(&mut h, info.width as u64);
    put_varint(&mut h, info.height as u64);
    put_varint(out, h.len() as u64);
    out.extend_from_slice(&h);
    if let Some(x) = &meta.icc {
        put_chunk(out, &ICCP, x);
    }
    if let Some(x) = &meta.mastering_display {
        put_chunk(out, &MDCV, x);
    }
    if let Some(x) = &meta.content_light_level {
        put_chunk(out, &CLLI, x);
    }
    if let Some(x) = meta.diffuse_white {
        put_chunk(out, &DWHI, &x.nits().to_be_bytes());
    }
    if let Some(x) = &meta.exif {
        put_chunk(out, &EXIF, x);
    }
    if let Some(x) = &meta.xmp {
        put_chunk(out, &XMP, x);
    }
    if let Some(g) = &meta.gain_map {
        if g.params.validate().is_err() {
            return Err(Error::InvalidInput("invalid gain map parameters"));
        }
        if g.image.get(..8) != Some(&SIGNATURE[..]) {
            return Err(Error::InvalidInput("gain map image must be an IMI file"));
        }
        put_chunk(out, &GMAP, &gain_map_bytes(g));
    }
    for c in &meta.chunks {
        if is_critical(&c.tag)
            || KNOWN.contains(&c.tag)
            || !c.tag.iter().all(u8::is_ascii_alphabetic)
        {
            return Err(Error::InvalidInput(
                "extra chunks need an unused ancillary tag (four letters, first lower case)",
            ));
        }
        let tag = if crate::item_metadata::is_tag(&c.tag) {
            if version >= 4 {
                crate::item_metadata::TAG
            } else {
                crate::item_metadata::LEGACY_TAG
            }
        } else {
            c.tag
        };
        put_chunk(out, &tag, &c.data);
    }
    Ok(())
}

/// Appends the `DATA` chunk with its length.
#[cfg(feature = "encode")]
pub(crate) fn put_data(out: &mut Vec<u8>, payload: &[u8]) {
    begin_data(out, payload.len());
    out.extend_from_slice(payload);
}

/// Start a bounded DATA chunk when the caller already has independently framed units.
#[cfg(feature = "encode")]
pub(crate) fn begin_data(out: &mut Vec<u8>, payload_len: usize) {
    out.extend_from_slice(&DATA);
    put_varint(out, payload_len as u64 + 1);
}

#[cfg_attr(not(any(feature = "encode", feature = "std")), allow(dead_code))]
fn gain_map_bytes(g: &GainMap) -> Vec<u8> {
    let mut b = Vec::with_capacity(g.image.len() + 128);
    b.push(0); // version
    let meta =
        zencodec::gainmap::serialize_iso21496_fmt(&g.params, zencodec::Iso21496Format::JxlJhgm);
    put_varint(&mut b, meta.len() as u64);
    b.extend_from_slice(&meta);
    put_varint(
        &mut b,
        u64::from(g.alt_cicp.is_some()) | (u64::from(g.alt_icc.is_some()) << 1),
    );
    if let Some(c) = &g.alt_cicp {
        b.extend_from_slice(&[
            c.color_primaries,
            c.transfer_characteristics,
            c.matrix_coefficients,
            c.full_range as u8,
        ]);
    }
    if let Some(icc) = &g.alt_icc {
        put_varint(&mut b, icc.len() as u64);
        b.extend_from_slice(icc);
    }
    b.extend_from_slice(&g.image);
    b
}

/// A gain map chunk; None for a version this decoder does not know or a
/// chunk it cannot parse (the chunk is ancillary: the base image still
/// decodes, and the bytes are kept as a raw chunk).
fn parse_gain_map(body: &[u8]) -> Option<GainMap> {
    gain_map_inner(body).ok().flatten()
}

fn gain_map_inner(body: &[u8]) -> Result<Option<GainMap>, Error> {
    let mut r = ByteReader::new(body);
    if r.u8()? != 0 {
        return Ok(None);
    }
    let n = r.var_usize()?;
    let Ok(params) =
        zencodec::gainmap::parse_iso21496_fmt(r.bytes(n)?, zencodec::Iso21496Format::JxlJhgm)
    else {
        return Ok(None);
    };
    let flags = r.varint()?;
    if flags & !3 != 0 {
        return Ok(None);
    }
    let alt_cicp = if flags & 1 != 0 {
        let c = r.bytes(4)?;
        Some(Cicp {
            color_primaries: c[0],
            transfer_characteristics: c[1],
            matrix_coefficients: c[2],
            full_range: c[3] != 0,
        })
    } else {
        None
    };
    let alt_icc = if flags & 2 != 0 {
        let n = r.var_usize()?;
        Some(r.bytes(n)?.to_vec())
    } else {
        None
    };
    let image = r.bytes(r.remaining())?.to_vec();
    if image.get(..8) != Some(&SIGNATURE[..]) {
        return Ok(None);
    }
    Ok(Some(GainMap {
        params,
        alt_cicp,
        alt_icc,
        image,
    }))
}

/// `file` with `add` merged into its header's feature mask (SPEC 0.1),
/// except what its own fields imply; every other byte is kept. Returns
/// `file` unchanged when its feature set already covers them.
#[cfg(feature = "encode")]
pub(crate) fn require_features(file: Vec<u8>, add: Features) -> Result<Vec<u8>, Error> {
    let mut r = ByteReader::new(&file);
    let hp = read_header_sized(&mut r)?;
    if hp.declared.covers(add) {
        return Ok(file);
    }
    let header_end = r.pos;
    let head_at = header_end - hp.len;
    // Version, `present`, then the mask when bit 2 is set (SPEC 1.0).
    let mut h = ByteReader::new(&file[head_at..header_end]);
    // The file keeps its own header version: its records are numbered for it.
    let version = h.u8()?;
    let present = h.varint()?;
    if present & F_FEATURES != 0 {
        crate::parse::header::read_feature_mask(&mut h)?;
    }
    let mut head = Vec::with_capacity(hp.len + 4);
    head.push(version);
    put_varint(&mut head, present | F_FEATURES);
    put_varint(&mut head, hp.mask.union(add.difference(hp.declared)).bits());
    head.extend_from_slice(&file[head_at + h.pos..header_end]);
    let mut out = Vec::new();
    out.try_reserve_exact(file.len() + 4)
        .map_err(|_| Error::TooLarge)?;
    out.extend_from_slice(&SIGNATURE);
    put_varint(&mut out, head.len() as u64);
    out.extend_from_slice(&head);
    out.extend_from_slice(&file[header_end..]);
    Ok(out)
}

/// A parsed file: header, its metadata chunks (borrowed: [`File::metadata`]
/// copies them out for the routes that return metadata; pixel routes never
/// copy or expand them), and the `DATA` payload (to the end of the file when
/// the chunk is open).
pub(crate) struct File<'a> {
    pub encoded_len: usize,
    pub header: Header,
    /// Metadata chunks ([`Kind::Meta`]), checked by the chunk rules, in file
    /// order (a prefix read keeps none).
    pub metadata: Vec<([u8; 4], &'a [u8])>,
    pub data: &'a [u8],
    /// Open V4 DATA terminates at the last geometry-declared tile end.
    pub data_open: bool,
    /// Preview files (`pRVW`), in file order.
    pub previews: Vec<&'a [u8]>,
    /// Animation / pages control (`aNIM`).
    pub anim: Option<&'a [u8]>,
    /// Further frames or pages (`fRAM`), in file order.
    pub frames: Vec<&'a [u8]>,
    /// The header's declared features: what the payload may use.
    pub declared: Features,
    /// The `cSUM` chunk (None when absent or ignored with [`Verify::Off`]).
    /// Its pieces were checked unless parsed with [`Verify::Defer`].
    pub checksum: Option<ChecksumChunk<'a>>,
    pub pixelsum: crate::pixelsum::Table<'a>,
    /// Final directory chunk and complete chunk length. V4 interprets its body;
    /// legacy routes retain their existing hint validation.
    pub trailing_index: Option<(&'a [u8], u64)>,
}

use crate::checksum::Verify;
use crate::parse::chunks;

/// Visits the chunks of a file. `data_len(header, declared, payload)` returns
/// the length of an open `DATA` payload (`declared`: the header's feature
/// mask). With `prefix`, a file that ends
/// cleanly between chunks before `DATA` is accepted (previews of a partial
/// download).
pub(crate) fn parse<'a>(
    data: &'a [u8],
    prefix: bool,
    data_len: &dyn Fn(&Header, Features, &'a [u8]) -> Result<usize, Error>,
) -> Result<File<'a>, Error> {
    parse_checked(
        data,
        prefix,
        data_len,
        &crate::parse::limits::Unrestricted,
        Verify::Now(1),
    )
}

/// [`parse`] under an admission policy and checksum mode. With `prefix`,
/// the pieces that lie wholly before `DATA` are checked when the table
/// precedes them.
pub(crate) fn parse_checked<'a>(
    data: &'a [u8],
    prefix: bool,
    data_len: &dyn Fn(&Header, Features, &'a [u8]) -> Result<usize, Error>,
    admission: &impl crate::parse::limits::Admission,
    verify: Verify,
) -> Result<File<'a>, Error> {
    parse_checked_version::<false>(data, prefix, data_len, admission, verify, u64::MAX)
}

pub(crate) fn parse_v4_checked<'a>(
    data: &'a [u8],
    data_len: &dyn Fn(&Header, Features, &'a [u8]) -> Result<usize, Error>,
    admission: &impl crate::parse::limits::Admission,
    verify: Verify,
    max_header_bytes: u64,
) -> Result<File<'a>, Error> {
    parse_checked_version::<true>(data, false, data_len, admission, verify, max_header_bytes)
}

fn parse_checked_version<'a, const V4: bool>(
    data: &'a [u8],
    prefix: bool,
    data_len: &dyn Fn(&Header, Features, &'a [u8]) -> Result<usize, Error>,
    admission: &impl crate::parse::limits::Admission,
    verify: Verify,
    max_header_bytes: u64,
) -> Result<File<'a>, Error> {
    admission.check()?;
    let mut r = ByteReader::new(data);
    let hp = if V4 {
        crate::parse::header::read_header_v4(&mut r, max_header_bytes)?
    } else {
        read_header_sized(&mut r)?
    };
    if V4 && admission.decodes() {
        crate::parse::header::check_decodable(&hp.header, hp.declared)?;
    }
    let is_v4 = hp.header.version == 4;
    let mixed_pages = hp.mixed_pages();
    let ParsedHeader {
        header,
        declared,
        len: header_bytes,
        checksum: announced,
        ..
    } = hp;
    let announced = if verify == Verify::Off {
        None
    } else {
        announced.map(|a| a.announced)
    };
    admission.coding_profile(&header)?;
    admission.header(&header.info, header_bytes)?;
    // The checksum record is read with the header: a malformed one is
    // corrupt whatever follows (SPEC 1.4).
    if let Some(a) = announced {
        a.log2()?;
    }
    let mut rules = Rules::new(mixed_pages, verify != Verify::Off)
        .with_sequence_items(declared.contains(crate::parse::header::Feature::SequenceItems));
    let mut metadata = Vec::new();
    let mut pixelsum = crate::pixelsum::Table::Absent;
    let mut previews = Vec::new();
    let (mut anim, mut frames) = (None, Vec::new());
    let mut body: Option<&'a [u8]> = None;
    let mut data_open = false;
    let mut chunk: Option<(usize, usize, &'a [u8])> = None;
    // The last chunk when it is a trailing unit index: its payload and whole
    // length (SPEC 2.7).
    let mut index: Option<(&'a [u8], u64)> = None;
    let mut index_seen = false;
    let mut data_at = data.len();
    // A rule or parse error with the table read before it: a mismatching
    // piece under the bytes read explains it (SPEC 1.4).
    let walked = (|| -> Result<(), Error> {
        while r.remaining() > 0 {
            let at = r.pos;
            let tracing = is_v4 && crate::trace::on();
            let trace_labels =
                tracing.then(|| match r.data.get(at..).and_then(|tail| tail.get(..4)) {
                    Some(b"pREC") => (
                        "V4:1.3",
                        "pREC bound native record",
                        "bound record ID and native payload",
                    ),
                    Some(b"pIPG") => (
                        "V4:1.3",
                        "pIPG native record page",
                        "packed native record lengths",
                    ),
                    Some(b"pIDX") => (
                        "V4:1.2",
                        "pIDX native directory",
                        "packed native directory metadata",
                    ),
                    Some(b"sLOC") => (
                        "V4:1.1",
                        "sLOC plane directory locator",
                        "plane directory locator",
                    ),
                    _ => ("1.1", "container chunk", "chunk payload"),
                });
            if let Some((section, label, _)) = trace_labels {
                crate::trace::trace_enter_addr("chunk", section, label, r.mark());
            }
            let (tag, len) = chunks::read_head(&mut r)?;
            admission.chunk_start()?;
            let kind = rules.head(tag, len)?;
            if prefix && kind == Kind::Data {
                data_at = at;
                break;
            }
            if let Some((section, _, label)) = trace_labels {
                crate::trace::trace_enter_addr("payload", section, label, r.mark());
            }
            let payload = match len {
                Some(n) => chunks::payload(&mut r, n)?,
                None => {
                    let n = data_len(&header, declared, &data[r.pos..])?;
                    r.bytes(n)?
                }
            };
            if tracing {
                #[cfg(all(feature = "lab", feature = "std"))]
                match &tag {
                    b"pREC" => {
                        let mut record = ByteReader::new(payload);
                        if let Ok(id) = record.varint() {
                            crate::trace::tnote!("native record ID", alloc::format!("{id}"));
                        }
                    }
                    b"pIPG" => {
                        let _ = trace_native_page(payload);
                    }
                    b"pIDX" => {
                        let _ = trace_native_directory(payload);
                    }
                    _ => {}
                }
                crate::trace::exit_addr(r.mark());
                crate::trace::exit_addr(r.mark());
            }
            // The rules a payload's first bytes show come before the
            // admission charges it (every route reports them alike); a
            // prefix read (`header`, `previews`) returns no metadata, so it
            // does not check an `mCMP` record's syntax (SPEC 1.1).
            if kind.has_body_rules(&tag) {
                rules.identity(tag, payload)?;
            }
            admission.chunk(tag, payload)?;
            if !prefix {
                chunks::record(tag, payload)?;
            }
            index = (kind == Kind::UnitIndex).then(|| (payload, (r.pos - at) as u64));
            index_seen |= kind == Kind::UnitIndex;
            if tag == crate::pixelsum::PSUM {
                pixelsum = crate::pixelsum::parse(payload)?;
                continue;
            }
            match kind {
                Kind::Data => {
                    body = Some(payload);
                    data_open = len.is_none();
                }
                Kind::Preview => previews.push(payload),
                Kind::Anim => anim = Some(payload),
                Kind::Frame => frames.push(payload),
                Kind::Checksum => chunk = Some((at, r.pos - at, payload)),
                // A prefix read (`header`, `previews`) returns no metadata.
                Kind::Meta if !prefix => {
                    metadata.try_reserve(1).map_err(|_| Error::TooLarge)?;
                    metadata.push((tag, payload));
                }
                Kind::Meta | Kind::Ignored | Kind::Plane | Kind::UnitIndex => {}
            }
        }
        Ok(())
    })();
    if let Err(e) = walked {
        // V4 integrity verification belongs to its budgeted file driver.
        // Never perform an uncharged hash rescan after admission has failed.
        if is_v4 {
            return Err(e);
        }
        return Err(crate::checksum::explain_in_memory(
            data, announced, chunk, r.pos, e,
        ));
    }
    if is_v4 && index_seen && index.is_none() {
        return Err(Error::Corrupt("V4 directory is not the final chunk"));
    }
    let checksum = if prefix {
        prefix_checksum(data, data_at, announced, chunk)?
    } else {
        let pieces = crate::checksum::pieces(data, announced, chunk)?;
        if let (Some(p), Verify::Now(threads)) = (&pieces, verify) {
            p.verify_all(threads)?;
        }
        chunk.map(|(at, len, _)| ChecksumChunk { at, len, pieces })
    };
    let finished = if prefix {
        rules.finish_prefix()
    } else {
        rules.finish()
    };
    if let Err(e) = finished {
        if is_v4 {
            return Err(e);
        }
        let end = if prefix { data_at } else { data.len() };
        return Err(crate::checksum::explain_in_memory(
            data, announced, chunk, end, e,
        ));
    }
    // A trailing unit index is checked against the records as a hint is
    // (SPEC 2.7), by every route that parses the whole file.
    if let (Some((payload, len)), false) = (index, prefix) {
        let checked = admission.hint(payload.len()).and_then(|()| {
            if is_v4 {
                return crate::v4::file::check_tail_locator(payload, len).map(|_| ());
            }
            crate::parse::payload::check_trailing_index(
                &header,
                declared,
                body.unwrap(),
                payload,
                len,
            )
        });
        if let Err(e) = checked {
            if is_v4 {
                return Err(e);
            }
            return Err(crate::checksum::explain_in_memory(
                data,
                announced,
                chunk,
                data.len(),
                e,
            ));
        }
    }
    if matches!(pixelsum, crate::pixelsum::Table::Known(..)) {
        pixelsum.check_count(crate::pixelsum::image_count(anim)?)?;
        if !prefix {
            pixelsum.check_count(frames.len() as u64 + 1)?;
        }
    }
    let encoded_len = data.len();
    let data = if prefix { &[][..] } else { body.unwrap() };
    Ok(File {
        encoded_len,
        header,
        metadata,
        data,
        data_open,
        previews,
        anim,
        frames,
        declared,
        checksum,
        pixelsum,
        trailing_index: index,
    })
}

#[cfg(all(feature = "lab", feature = "std"))]
fn trace_native_page(payload: &[u8]) -> Result<(), Error> {
    let raw = crate::metadata_pack::unpack(payload, 64 << 10)?;
    let mut r = ByteReader::new(&raw);
    let first = r.varint()?;
    let count = r.varint()?;
    let first_len = r.varint()?;
    crate::trace::tnote!("first native record", alloc::format!("{first}"));
    crate::trace::tnote!("records in page", alloc::format!("{count}"));
    if first_len == 0 {
        let base = r.varint()?;
        let width = r.u8()?;
        crate::trace::tnote!("length encoding", "base plus packed residuals".into());
        crate::trace::tnote!("base record bytes", alloc::format!("{base}"));
        crate::trace::tnote!("length residual bits", alloc::format!("{width}"));
    } else {
        crate::trace::tnote!("length encoding", "varints".into());
    }
    Ok(())
}

#[cfg(all(feature = "lab", feature = "std"))]
fn trace_native_directory(payload: &[u8]) -> Result<(), Error> {
    let raw = crate::metadata_pack::unpack(payload, 4 << 20)?;
    crate::parse::records::head_records(&raw, |id, payload| {
        if id == crate::registry::DIRECTORY_RECORD_PAGES {
            let mut r = ByteReader::new(payload);
            let schema = r.varint()?;
            let count = r.varint()?;
            let size = r.varint()?;
            crate::trace::tnote!("native page schema", alloc::format!("{schema}"));
            crate::trace::tnote!("native directory records", alloc::format!("{count}"));
            crate::trace::tnote!("nominal records per page", alloc::format!("{size}"));
        }
        Ok(())
    })
}

impl<'a> File<'a> {
    /// Borrow only one image's compressed span and directory. The surrounding
    /// consumer retains sequence controls, metadata and bitmap verification.
    pub(crate) fn image_view(&self) -> Self {
        Self {
            encoded_len: self.encoded_len,
            header: self.header.clone(),
            declared: self.declared,
            data: self.data,
            data_open: self.data_open,
            trailing_index: self.trailing_index,
            metadata: Vec::new(),
            previews: Vec::new(),
            anim: None,
            frames: Vec::new(),
            checksum: None,
            pixelsum: crate::pixelsum::Table::Absent,
        }
    }
}

/// A prefix read: checks the pieces that lie wholly before `DATA` (at file
/// offset `data_at`) when the table precedes them; nothing is required.
fn prefix_checksum<'a>(
    data: &'a [u8],
    data_at: usize,
    announced: Option<crate::checksum::Announced>,
    chunk: Option<(usize, usize, &'a [u8])>,
) -> Result<Option<ChecksumChunk<'a>>, Error> {
    let (Some(a), Some((at, len, payload))) = (announced, chunk) else {
        return Ok(None);
    };
    crate::checksum::same_algorithm(a.algorithm_id()?, payload)?;
    let Some(table) = crate::checksum::parse_payload(payload)? else {
        return Ok(None);
    };
    table.check(a.log2()?, None)?;
    let covered = crate::checksum::Covered {
        data,
        at,
        chunk_len: len,
    };
    let end = covered.offset(data_at);
    if end > table.covered_len {
        return Err(Error::Corrupt("cSUM length does not match the file"));
    }
    let pieces = crate::checksum::Pieces { covered, table };
    pieces.verify(pieces.inside(0..end))?;
    Ok(Some(ChecksumChunk {
        at,
        len,
        pieces: Some(pieces),
    }))
}

impl File<'_> {
    /// The file's metadata, copied out of its chunks (an `mCMP` record
    /// expanded): for the routes that return it.
    pub(crate) fn metadata(&self) -> Result<crate::Metadata, Error> {
        let mut meta = crate::Metadata {
            orientation: self.header.orientation,
            ..Default::default()
        };
        for &(tag, payload) in &self.metadata {
            keep_metadata(&mut meta, tag, payload)?;
        }
        Ok(meta)
    }
}

/// Keeps a metadata chunk ([`Kind::Meta`], already checked by the chunk
/// rules) in `meta`: an `mCMP` record expanded into the chunk it
/// compresses, a gain map parsed (kept raw when it does not parse), and
/// unknown ancillary chunks as [`Chunk`]s.
pub(crate) fn keep_metadata(
    meta: &mut crate::Metadata,
    tag: [u8; 4],
    payload: &[u8],
) -> Result<(), Error> {
    if tag == crate::pixelsum::PSUM {
        return Ok(());
    }
    match tag {
        MCMP => {
            // The chunk rules checked the record's syntax: expand it.
            let decoded = crate::metadata_pack::expand(&payload[4..], MAX_PACKED_METADATA)?;
            // Transfer ownership; do not temporarily retain two expanded copies.
            match payload[..4].try_into().unwrap() {
                ICCP => meta.icc = Some(decoded),
                EXIF => meta.exif = Some(decoded),
                _ => meta.xmp = Some(decoded),
            }
        }
        ICCP => meta.icc = Some(payload.to_vec()),
        EXIF => meta.exif = Some(payload.to_vec()),
        XMP => meta.xmp = Some(payload.to_vec()),
        DWHI => meta.diffuse_white = Some(chunks::diffuse_white(payload)?),
        MDCV => meta.mastering_display = payload.try_into().ok(),
        CLLI => {
            meta.content_light_level = match payload {
                // Pre-freeze files stored whole nits in two u16 words.
                [a, b, c, d] => {
                    let max_cll = u32::from(u16::from_be_bytes([*a, *b])) * 10_000;
                    let max_fall = u32::from(u16::from_be_bytes([*c, *d])) * 10_000;
                    let mut bytes = [0; 8];
                    bytes[..4].copy_from_slice(&max_cll.to_be_bytes());
                    bytes[4..].copy_from_slice(&max_fall.to_be_bytes());
                    Some(bytes)
                }
                _ => payload.try_into().ok(), // length checked by Rules
            };
        }
        GMAP => match parse_gain_map(payload) {
            Some(g) => meta.gain_map = Some(g),
            None => meta.chunks.push(Chunk {
                tag,
                data: payload.to_vec(),
            }),
        },
        t => meta.chunks.push(Chunk {
            tag: t,
            data: payload.to_vec(),
        }),
    }
    Ok(())
}

/// File offset of the `DATA` chunk; the chunks before it must be bounded.
pub(crate) fn data_at(file: &[u8]) -> Result<usize, Error> {
    let mut r = ByteReader::new(file);
    read_header(&mut r)?;
    Ok(chunks::walk_to_data(&mut r, |_, _| {})?.0)
}

/// Rewrites a file with `chunks` inserted just before `DATA`.
#[cfg(feature = "encode")]
pub(crate) fn insert_before_data(
    file: &[u8],
    chunks: &[([u8; 4], &[u8])],
) -> Result<Vec<u8>, Error> {
    let at = data_at(file)?;
    let mut out =
        Vec::with_capacity(file.len() + chunks.iter().map(|c| c.1.len() + 8).sum::<usize>());
    out.extend_from_slice(&file[..at]);
    for (t, d) in chunks {
        put_chunk(&mut out, t, d);
    }
    out.extend_from_slice(&file[at..]);
    // The chunks before DATA changed: refresh the DATA locator.
    locate(out)
}

/// Writes, refreshes or removes the DATA locator of a complete file: a file
/// whose DATA head has a unit table hint gets one, any other file loses a
/// stale one. Writers call it last, after every chunk before DATA is in
/// place.
pub(crate) fn locate(file: Vec<u8>) -> Result<Vec<u8>, Error> {
    let mut r = ByteReader::new(&file);
    let hp = read_header_sized(&mut r)?;
    if hp.header.version == 4 {
        return with_extension(file, EXT_LOCATOR, None);
    }
    let mut lens = Vec::new();
    let (at, _) = chunks::walk_to_data(&mut r, |a, b| lens.push((b - a) as u64))?;
    let payload_at = r.pos;
    let end = crate::v1::head_end_with_hint(&hp.header, hp.declared, &file[payload_at..])?;
    let prefix = end.map(|e| (payload_at - at + e) as u64);
    let rec = prefix.map(|prefix| Locator {
        prefix,
        chunks: lens,
    });
    with_extension(file, EXT_LOCATOR, rec.map(|l| l.bytes()).as_deref())
}

/// Settles the unit table of a complete file's `DATA` payload (SPEC 2.7) as
/// this thread's writer form asks ([`crate::v1::UnitTableForm`]): a
/// reservation filled in place with the records' spans (or, when a span does
/// not fit its width, marked check tail), or marked check tail; with check
/// tail, the trailing `uIDX` chunk is appended as the file's last chunk.
/// The file keeps its length up to its end. The hint and the empty forms
/// are returned as they are, so the reference writers' files never change
/// here. Writers call it before the DATA locator and the checksum, which
/// cover the head.
#[cfg(feature = "encode")]
pub(crate) fn finish_unit_table(file: Vec<u8>) -> Result<Vec<u8>, Error> {
    use crate::parse::records::{RESERVE_TAIL, filled_reserve, index_chunk};
    use crate::v1::UnitTableForm;
    let fill = match crate::v1::unit_table_form() {
        UnitTableForm::Hint | UnitTableForm::None | UnitTableForm::ReservedEmpty(_) => {
            return Ok(file);
        }
        UnitTableForm::Reserved(w) => Some(w),
        UnitTableForm::Trailing | UnitTableForm::ReservedTail(_) => None,
    };
    let mut r = ByteReader::new(&file);
    let hp = read_header_sized(&mut r)?;
    let (_, len) = chunks::walk_to_data(&mut r, |_, _| {})?;
    let at = r.pos;
    let end = match len {
        Some(n) => at + usize::try_from(n).map_err(|_| Error::TooLarge)?,
        None => file.len(),
    };
    let body = &file[at..end];
    let mut pr = ByteReader::new(body);
    let head = crate::parse::payload::read_head(
        &hp.header,
        hp.declared,
        &mut pr,
        &crate::parse::limits::Structure,
    )?;
    let Some(res) = head.reserve() else {
        return Ok(file);
    };
    let (map, stripes) = head.units();
    let spans = map.spans_with_ordinals(
        &mut pr,
        hp.header.version,
        stripes,
        None,
        head.unit_ordinals(),
    )?;
    let data_len = body.len() as u64;
    let rec = at + res.at as usize..at + res.at as usize + res.len;
    let mut file = file;
    if let Some(bytes) = fill.and_then(|w| filled_reserve(&spans, w)) {
        if bytes.len() != res.len {
            return Err(Error::InvalidInput("unit table reservation length"));
        }
        file[rec].copy_from_slice(&bytes);
        return Ok(file);
    }
    file[rec.start] = RESERVE_TAIL;
    file.extend_from_slice(&index_chunk(&spans, data_len));
    Ok(file)
}

/// `file` with header extension `id` set to `rec` (or removed for `None`),
/// the records kept in strictly increasing id order (SPEC 1.0). Returns
/// `file` itself when nothing changes.
pub(crate) fn with_extension(file: Vec<u8>, id: u64, rec: Option<&[u8]>) -> Result<Vec<u8>, Error> {
    let mut r = ByteReader::new(&file);
    let hp = read_header_sized(&mut r)?;
    let header_end = r.pos;
    let head = &file[header_end - hp.len..header_end];
    let mut recs: Vec<(u64, &[u8])> = Vec::new();
    let mut h = ByteReader::new(&file[hp.ext_at..header_end]);
    while h.remaining() != 0 {
        let x = h.varint()?;
        let len = h.var_usize()?;
        let b = h.bytes(len)?;
        if x != id {
            recs.push((x, b));
        }
    }
    if let Some(rec) = rec {
        recs.insert(recs.partition_point(|r| r.0 < id), (id, rec));
    }
    let mut out_head = file[header_end - hp.len..hp.ext_at].to_vec();
    for (x, b) in &recs {
        put_varint(&mut out_head, *x);
        put_varint(&mut out_head, b.len() as u64);
        out_head.extend_from_slice(b);
    }
    if out_head == head {
        return Ok(file);
    }
    let mut out = Vec::new();
    out.try_reserve_exact(file.len() - hp.len + out_head.len() + 10)
        .map_err(|_| Error::TooLarge)?;
    out.extend_from_slice(&SIGNATURE);
    put_varint(&mut out, out_head.len() as u64);
    out.extend_from_slice(&out_head);
    out.extend_from_slice(&file[header_end..]);
    Ok(out)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::LossyColor;
    use crate::parse::header::{
        Feature, Profile, ProfileVersion, implied_features, profiles_of, supported_profiles,
    };
    use alloc::vec;

    /// Metadata is built on demand: a parse keeps its chunks borrowed, so
    /// the routes that return no metadata (`header`, `previews` and every
    /// pixel route) copy none and expand no `mCMP` record (the chunk rules
    /// validate it without expanding it); `probe` and `File::metadata` copy
    /// and expand.
    #[cfg(feature = "std")]
    #[test]
    fn metadata_is_built_on_demand() {
        let info = crate::ImageInfo::new(8, 4, crate::Channels::Rgb, crate::SampleType::U8);
        let px = vec![7u8; info.row_bytes() * 4];
        let meta = crate::Metadata {
            icc: Some(vec![3; 4096]),
            exif: Some(vec![5; 4096]),
            xmp: Some(b"<x:xmpmeta/>".repeat(400)),
            chunks: vec![Chunk {
                tag: *b"vnDr",
                data: vec![9; 4096],
            }],
            ..Default::default()
        };
        let plain =
            crate::encode(&px, info.row_bytes(), &info, &meta, &Default::default()).unwrap();
        let packed = crate::compress_metadata(&plain, 1 << 20).unwrap();
        assert!(packed.windows(4).any(|w| w == MCMP));
        let unpacked = || crate::metadata_pack::UNPACKED.with(core::cell::Cell::get);
        for file in [&plain, &packed] {
            let before = unpacked();
            let f = parse(file, true, &crate::v1::payload_len).unwrap();
            assert!(f.metadata.is_empty());
            assert_eq!(crate::header(file).unwrap().info, info);
            assert!(crate::previews(file).unwrap().is_empty());
            let f = parse(file, false, &crate::v1::payload_len).unwrap();
            assert_eq!(f.metadata.len(), 4);
            assert_eq!(crate::decode(file).unwrap().1, px);
            let mut row = vec![0u8; info.row_bytes()];
            let mut s = crate::StreamDecoder::new(file).unwrap();
            while s.next_row(&mut row).unwrap() {}
            assert_eq!(unpacked(), before, "a route without metadata expanded mCMP");
            let m = f.metadata().unwrap();
            assert_eq!(
                (m.icc, m.exif, m.xmp),
                (meta.icc.clone(), meta.exif.clone(), meta.xmp.clone())
            );
            assert_eq!(m.chunks, meta.chunks);
            assert_eq!(crate::probe(file).unwrap().1.chunks, meta.chunks);
        }
        assert!(unpacked() > 0);
    }

    fn envelope(body: &[u8]) -> Vec<u8> {
        let mut bytes = SIGNATURE.to_vec();
        put_varint(&mut bytes, body.len() as u64);
        bytes.extend_from_slice(body);
        bytes
    }

    fn header(bytes: &[u8]) -> Result<Header, Error> {
        read_header(&mut ByteReader::new(bytes))
    }

    fn color_of(coding: Coding) -> Option<LossyColor> {
        (coding == Coding::Lossy).then_some(LossyColor::Xyb)
    }

    #[test]
    fn full_and_masked_bytes() {
        let info = ImageInfo::new(64, 32, Channels::Rgb, SampleType::U8);
        let meta = crate::Metadata::default();
        let mut full = Vec::new();
        let mut masked = Vec::new();
        let args = (Coding::Lossless, None, DecodeSpeed::Balanced);
        write_full_head(&mut full, &info, &meta, args.0, args.1, args.2).unwrap();
        write_head(&mut masked, &info, &meta, args.0, args.1, args.2).unwrap();
        assert_eq!(&full[..8], b"IMIJ\r\n\x87\n");
        // Version 3; present 0x1ff; features 0; coding, sample, channels,
        // orientation, speed, CICP, bits, alpha mode; width, height.
        assert_eq!(
            full,
            envelope(&[
                3, 0xff, 0x03, 0, 0, 0, 3, 1, 1, 1, 13, 0, 1, 8, 8, 8, 0, 64, 32
            ])
        );
        assert_eq!(masked, envelope(&[3, 0, 64, 32]));
        assert_eq!(header(&full).unwrap(), header(&masked).unwrap());

        // Lossy YCoCg RGBA: channels and coding differ from their defaults.
        let info = ImageInfo::new(64, 32, Channels::Rgba, SampleType::U8);
        masked.clear();
        let c = Some(LossyColor::YCoCg);
        write_head(
            &mut masked,
            &info,
            &meta,
            Coding::Lossy,
            c,
            DecodeSpeed::Balanced,
        )
        .unwrap();
        // present = coding | channels; coding 2, channels 4.
        assert_eq!(masked, envelope(&[3, 0x09, 2, 4, 64, 32]));
        let h = header(&masked).unwrap();
        assert_eq!((h.coding, h.lossy_color), (Coding::Lossy, c));
    }

    #[test]
    fn full_and_masked_semantic_parity() {
        for channels in [
            Channels::Rgb,
            Channels::Rgba,
            Channels::Gray,
            Channels::GrayAlpha,
        ] {
            for sample in [
                SampleType::U8,
                SampleType::U16,
                SampleType::F16,
                SampleType::U32,
                SampleType::I32,
                SampleType::F32,
            ] {
                for cicp in [
                    Cicp::SRGB,
                    Cicp {
                        color_primaries: 9,
                        transfer_characteristics: 16,
                        matrix_coefficients: 1,
                        full_range: false,
                    },
                ] {
                    for alpha_mode in [crate::AlphaMode::Straight, crate::AlphaMode::Premultiplied]
                    {
                        for orientation in [0, 1, 8] {
                            for (coding, lossy_color) in [
                                (Coding::Lossless, None),
                                (Coding::Lossy, Some(LossyColor::Xyb)),
                                (Coding::Lossy, Some(LossyColor::YCoCg)),
                            ] {
                                for speed in [
                                    DecodeSpeed::Compact,
                                    DecodeSpeed::Balanced,
                                    DecodeSpeed::Fast,
                                    DecodeSpeed::Fastest,
                                ] {
                                    let mut significant_bits = [0; 4];
                                    if sample == SampleType::U16 {
                                        significant_bits[0] = 10;
                                    }
                                    let info = ImageInfo {
                                        width: 128,
                                        height: 511,
                                        channels,
                                        sample,
                                        cicp,
                                        alpha_mode,
                                        significant_bits,
                                    };
                                    let meta = crate::Metadata {
                                        orientation,
                                        ..Default::default()
                                    };
                                    let mut full = Vec::new();
                                    let mut masked = Vec::new();
                                    write_full_head(
                                        &mut full,
                                        &info,
                                        &meta,
                                        coding,
                                        lossy_color,
                                        speed,
                                    )
                                    .unwrap();
                                    write_head(
                                        &mut masked,
                                        &info,
                                        &meta,
                                        coding,
                                        lossy_color,
                                        speed,
                                    )
                                    .unwrap();
                                    let v01 = |profile| ProfileVersion {
                                        profile,
                                        major: 0,
                                        minor: 1,
                                    };
                                    let mut profiles = alloc::vec![v01(Profile::Lossless)];
                                    if info.sample.bytes() == 4 {
                                        profiles.push(v01(Profile::Extended));
                                    }
                                    if coding != Coding::Lossless {
                                        profiles.push(v01(Profile::Lossy));
                                    }
                                    // The fields imply the features (no mask
                                    // bit is written for them).
                                    let implied = implied_features(coding, info.sample);
                                    assert_eq!(profiles_of(implied), profiles);
                                    let expected = Header {
                                        version: HEADER_VERSION,
                                        profiles,
                                        features: implied,
                                        info,
                                        coding,
                                        lossy_color,
                                        decode_speed: speed,
                                        orientation: orientation.max(1),
                                        float_guarantees: crate::float_guarantees::EMPTY,
                                    };
                                    assert_eq!(header(&full).unwrap(), expected);
                                    assert_eq!(header(&masked).unwrap(), expected);
                                    assert!(masked.len() <= full.len());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn metadata_and_data_chunks_are_unchanged() {
        let info = ImageInfo::new(129, 65, Channels::Rgba, SampleType::U16);
        let meta = crate::Metadata {
            icc: Some(vec![1, 2, 3]),
            exif: Some(vec![4, 5]),
            xmp: Some(vec![6, 7]),
            mastering_display: Some([8; 24]),
            content_light_level: Some([9; 8]),
            orientation: 7,
            chunks: vec![Chunk {
                tag: *b"tESt",
                data: vec![10, 11],
            }],
            ..Default::default()
        };
        let mut full = Vec::new();
        let mut masked = Vec::new();
        let (c, s) = (Coding::Lossless, DecodeSpeed::Compact);
        write_full_head(&mut full, &info, &meta, c, None, s).unwrap();
        write_head(&mut masked, &info, &meta, c, None, s).unwrap();
        put_data(&mut full, &[12, 13]);
        put_data(&mut masked, &[12, 13]);
        let mut a = ByteReader::new(&full);
        let mut b = ByteReader::new(&masked);
        assert_eq!(read_header(&mut a).unwrap(), read_header(&mut b).unwrap());
        assert_eq!(&full[a.pos..], &masked[b.pos..]);
        for bytes in [&full, &masked] {
            let file = parse(bytes, false, &|_, _, _| unreachable!()).unwrap();
            assert_eq!(file.metadata().unwrap(), meta);
            assert_eq!(file.data, [12, 13]);
        }
    }

    #[test]
    fn rejects_truncation_reserved_fields_and_bad_values() {
        // Full form: features 0, lossy YCoCg, U16, RGBA, orientation 8,
        // Fastest, PQ, 10/10/10/16 bits, premultiplied, 1x4.
        let valid = [
            3, 0xff, 0x03, 0, 2, 1, 4, 8, 3, 9, 16, 0, 1, 10, 10, 10, 16, 1, 1, 4,
        ];
        assert!(header(&envelope(&valid)).is_ok());
        for len in 0..valid.len() {
            assert!(header(&envelope(&valid[..len])).is_err(), "{len}");
        }
        for (offset, values) in [
            (0, &[0, 1, 5, 255][..]),  // unassigned header versions
            (2, &[0x07, 0x7f][..]),    // present bits beyond the version's fields
            (4, &[3, 255][..]),        // coding
            (5, &[6, 7, 11, 255][..]), // sample type (i8/i16 without their feature)
            (6, &[0, 5, 255][..]),     // channels
            (7, &[0, 9, 255][..]),     // orientation
            (12, &[2, 255][..]),       // full-range flag
            (13, &[0, 17][..]),        // significant bits
            (17, &[2, 255][..]),       // alpha mode
            (18, &[0][..]),            // width
            (19, &[0][..]),            // height
        ] {
            for &value in values {
                let mut body = valid;
                body[offset] = value;
                assert!(header(&envelope(&body)).is_err(), "{body:?}");
            }
        }
        // Float samples declare all their bits (present = sample | bits).
        let float = [2, 0x82, 0x01, 2, 15, 15, 15, 1, 1];
        assert!(header(&envelope(&float)).is_err());
        let mut oversized = vec![2, 0];
        put_varint(&mut oversized, u64::from(u32::MAX) + 1);
        oversized.push(1);
        assert!(header(&envelope(&oversized)).is_err());
        let mut overflow = vec![2, (F_SAMPLE | F_CHANNELS) as u8, SampleType::F32 as u8];
        overflow.push(Channels::Rgba as u8);
        put_varint(&mut overflow, u64::from(u32::MAX));
        put_varint(&mut overflow, u64::from(u32::MAX));
        assert_eq!(header(&envelope(&overflow)), Err(Error::TooLarge));
    }

    #[test]
    fn extensions_and_unknown_speed() {
        // Even (ignorable) extensions are skipped, in increasing id order.
        let base = [2, F_SPEED as u8, 255, 64, 32];
        let mut ok = base.to_vec();
        ok.extend_from_slice(&[2, 3, 7, 7, 7, 4, 0]);
        let h = header(&envelope(&ok)).unwrap();
        assert_eq!(h.decode_speed, DecodeSpeed::Fastest);
        // An odd (critical) extension the decoder does not know.
        let mut critical = base.to_vec();
        critical.extend_from_slice(&[5, 0]);
        assert_eq!(
            header(&envelope(&critical)),
            Err(Error::Unsupported("unknown critical header record"))
        );
        let mut unordered = base.to_vec();
        unordered.extend_from_slice(&[4, 0, 2, 0]);
        assert!(header(&envelope(&unordered)).is_err());
        let mut truncated = base.to_vec();
        truncated.extend_from_slice(&[2, 5, 1]);
        assert!(header(&envelope(&truncated)).is_err());
    }

    #[test]
    fn writer_checks_dimensions_orientation_and_color() {
        let mut info = ImageInfo::new(0, 1, Channels::Rgb, SampleType::U8);
        let mut meta = crate::Metadata::default();
        let lossy = (
            Coding::Lossy,
            color_of(Coding::Lossy),
            DecodeSpeed::Balanced,
        );
        assert!(write_head(&mut Vec::new(), &info, &meta, lossy.0, lossy.1, lossy.2).is_err());
        info.width = 1;
        meta.orientation = 9;
        assert!(write_head(&mut Vec::new(), &info, &meta, lossy.0, lossy.1, lossy.2).is_err());
        meta.orientation = 1;
        assert!(write_head(&mut Vec::new(), &info, &meta, Coding::Lossy, None, lossy.2).is_err());
        let x = Some(LossyColor::Xyb);
        assert!(write_head(&mut Vec::new(), &info, &meta, Coding::Lossless, x, lossy.2).is_err());
    }

    #[test]
    fn supported_profiles_match_compiled_decoder() {
        // The 0.3 versions (the unit table) are in every version 2 decoder;
        // Lossy 0.4 (the dequantization features), Compact 0.2
        // (superblocks) and 0.3 (small transforms) are implemented too.
        let all = [
            (0, 1),
            (0, 2),
            (0, 3),
            (1, 1),
            (1, 2),
            (2, 1),
            (2, 2),
            (2, 3),
            (2, 4),
            (2, 5),
            (2, 6),
            (2, 7),
            (3, 1),
            (3, 2),
            (3, 3),
            (4, 1),
            (4, 2),
            (5, 1),
            (5, 2),
        ];
        let expected: Vec<ProfileVersion> = all
            .iter()
            .filter(|p| cfg!(feature = "extended") || (p.0 != 1 && !(p.0 == 0 && p.1 > 1)))
            .map(|&(p, minor)| ProfileVersion {
                profile: [
                    Profile::Lossless,
                    Profile::Extended,
                    Profile::Lossy,
                    Profile::Compact,
                    Profile::Scientific,
                    Profile::Bilevel,
                ][p],
                major: 0,
                minor,
            })
            .collect();
        assert_eq!(supported_profiles(), expected);
    }

    /// The feature mask: written right after `present` when the payload
    /// uses a feature, with the sample type's own feature joining it; one
    /// mask byte for ids 0-6.
    #[test]
    fn feature_mask_record() {
        let info = ImageInfo::new(3, 2, Channels::Rgb, SampleType::U8);
        let meta = crate::Metadata::default();
        let write = |info: &ImageInfo, coding, f: Features| {
            let mut out = Vec::new();
            let color = color_of(coding);
            write_head_features(
                &mut out,
                info,
                &meta,
                coding,
                color,
                DecodeSpeed::Balanced,
                f,
            )
            .unwrap();
            out
        };
        let parsed = |f: &[u8]| read_header_sized(&mut ByteReader::new(f)).unwrap();
        let plain = write(&info, Coding::Lossless, Features::EMPTY);
        assert_eq!(plain, envelope(&[3, 0, 3, 2]));
        assert_eq!(parsed(&plain).declared, Features::EMPTY);
        let seeds = Features::EMPTY
            .with(Feature::SeedBank1)
            .with(Feature::ExactBlocks);
        let lossy = write(&info, Coding::Lossy, seeds);
        // present = coding | features; features 3; coding 1.
        assert_eq!(lossy, envelope(&[3, 0x05, 0x03, 1, 3, 2]));
        assert_eq!(parsed(&lossy).mask, seeds);
        // The coding implies `lossy`: in the set, not in the mask.
        assert_eq!(parsed(&lossy).declared, seeds.with(Feature::Lossy));
        assert_eq!(
            write(&info, Coding::Lossy, seeds.with(Feature::Lossy)),
            lossy
        );
        // `samples_32` is implied by a 32-bit sample type and written for an
        // 8-bit lossy file that needs it (an i32 DC image): id 15 takes
        // three mask bytes.
        let s32 = Features::EMPTY.with(Feature::Samples32);
        let u32i = ImageInfo::new(3, 2, Channels::Rgb, SampleType::U32);
        assert_eq!(
            write(&u32i, Coding::Lossy, s32),
            envelope(&[3, 0x03, 1, SampleType::U32 as u8, 3, 2])
        );
        let dc32 = write(&info, Coding::Lossy, s32);
        assert_eq!(dc32, envelope(&[3, 0x05, 0x80, 0x80, 0x02, 1, 3, 2]));
        assert_eq!(parsed(&dc32).declared, s32.with(Feature::Lossy));
        // i16 samples declare `signed_samples` without being asked.
        let i16 = ImageInfo::new(3, 2, Channels::Rgb, SampleType::I16);
        let f = write(&i16, Coding::Lossless, Features::EMPTY);
        assert_eq!(f, envelope(&[3, 0x06, 0x04, SampleType::I16 as u8, 3, 2]));
        assert_eq!(
            parsed(&f).declared,
            Features::EMPTY.with(Feature::SignedSamples)
        );
        // A two-byte mask (id 9).
        let bias = write(
            &info,
            Coding::Lossless,
            Features::EMPTY.with(Feature::ReconBias),
        );
        assert_eq!(bias, envelope(&[3, 0x04, 0x80, 0x04, 3, 2]));
    }

    #[test]
    fn require_features_merges_into_any_header() {
        // Full and masked headers, with and without a mask, followed by an
        // ignorable record: the features join the mask and every other byte
        // is kept.
        let info = ImageInfo::new(5, 4, Channels::Rgba, SampleType::U16)
            .with_significant_bits(&[10, 10, 10, 1])
            .unwrap();
        let meta = crate::Metadata {
            icc: Some(alloc::vec![1, 2, 3]),
            ..Default::default()
        };
        let add = Features::EMPTY
            .with(Feature::NearLossless)
            .with(Feature::MixedPages);
        let span = |f: &[u8]| {
            let mut r = ByteReader::new(f);
            let p = read_header_sized(&mut r).unwrap();
            (r.pos - p.len, p.ext_at, r.pos)
        };
        for full in [false, true] {
            for listed in [Features::EMPTY, Features::EMPTY.with(Feature::SeedBank1)] {
                let mut f = Vec::new();
                write_head_impl(
                    &mut f,
                    &info,
                    &meta,
                    Coding::Lossless,
                    None,
                    DecodeSpeed::Fast,
                    listed,
                    full,
                )
                .unwrap();
                let (start, _, end) = span(&f);
                let mut h = f[start..end].to_vec();
                h.extend_from_slice(&[6, 2, 9, 9]);
                let mut g = SIGNATURE.to_vec();
                put_varint(&mut g, h.len() as u64);
                g.extend_from_slice(&h);
                g.extend_from_slice(&f[end..]);
                let out = require_features(g.clone(), add).unwrap();
                let p = read_header_sized(&mut ByteReader::new(&out)).unwrap();
                assert_eq!(p.declared, listed.union(add));
                assert!(p.mixed_pages());
                let q = read_header_sized(&mut ByteReader::new(&g)).unwrap();
                assert_eq!(p.header.info, q.header.info);
                assert_eq!(p.header.decode_speed, DecodeSpeed::Fast);
                let (s0, x0, e0) = span(&g);
                let (s1, x1, e1) = span(&out);
                // The fields after the mask, the records and the chunks are
                // kept byte for byte.
                assert_eq!(out[x1..e1], g[x0..e0]);
                assert!(out[s1..e1].ends_with(&[6, 2, 9, 9]));
                assert_eq!(out[e1..], g[e0..]);
                // The mask is one byte (ids below 7); a new field adds it.
                let grown = if full || !listed.is_empty() { 0 } else { 1 };
                assert_eq!(e1 - s1, e0 - s0 + grown);
                // Idempotent, and a covered request changes nothing.
                assert_eq!(require_features(out.clone(), add).unwrap(), out);
                assert_eq!(require_features(g.clone(), Features::EMPTY).unwrap(), g);
                // A feature the fields do not imply joins the mask.
                let lossy = Features::EMPTY.with(Feature::Lossy);
                let p = read_header_sized(&mut ByteReader::new(
                    &require_features(g.clone(), lossy).unwrap(),
                ))
                .unwrap();
                assert_eq!(p.mask, listed.with(Feature::Lossy));
            }
        }
        // What a header's fields imply never joins its mask.
        let u32i = ImageInfo::new(5, 4, Channels::Rgb, SampleType::U32);
        let mut f = Vec::new();
        write_head_impl(
            &mut f,
            &u32i,
            &meta,
            Coding::Lossy,
            Some(LossyColor::YCoCg),
            DecodeSpeed::Fast,
            Features::EMPTY,
            false,
        )
        .unwrap();
        let implied = Features::EMPTY
            .with(Feature::Lossy)
            .with(Feature::Samples32);
        assert_eq!(require_features(f.clone(), implied).unwrap(), f);
        let out = require_features(f, implied.with(Feature::MixedPages)).unwrap();
        let p = read_header_sized(&mut ByteReader::new(&out)).unwrap();
        assert_eq!(p.mask, Features::EMPTY.with(Feature::MixedPages));
        assert_eq!(p.declared, implied.with(Feature::MixedPages));
    }

    /// The lossy payload repeats no color: the header's coding is the one
    /// source of truth (SPEC 2.8). The same payload under a header naming
    /// the other color reads with that color.
    #[test]
    fn the_lossy_color_comes_from_the_header() {
        let info = ImageInfo::new(16, 16, Channels::Rgb, SampleType::U8);
        let px: Vec<u8> = (0..16 * 16 * 3).map(|i| (i * 7 % 251) as u8).collect();
        let mut cfg = crate::LossyConfig::new(80.0);
        cfg.color = LossyColor::YCoCg;
        let f = crate::encode_lossy(&px, 48, &info, &crate::Metadata::default(), &cfg).unwrap();
        assert_eq!(header(&f).unwrap().lossy_color, Some(LossyColor::YCoCg));
        // The coding byte: after the version, `present` and the mask.
        let mut r = ByteReader::new(&f[8..]);
        r.varint().unwrap();
        let head_at = 8 + r.pos;
        let mut h = ByteReader::new(&f[head_at..]);
        h.u8().unwrap();
        let present = h.varint().unwrap();
        assert_eq!(present & F_CODING, F_CODING);
        if present & F_FEATURES != 0 {
            h.varint().unwrap();
        }
        let at = head_at + h.pos;
        assert_eq!(f[at], 2);
        let mut xyb = f.clone();
        xyb[at] = 1;
        assert_eq!(header(&xyb).unwrap().lossy_color, Some(LossyColor::Xyb));
        let (_, a) = crate::decode(&f).unwrap();
        assert_ne!(crate::decode(&xyb).map(|d| d.1).ok(), Some(a));
    }
}
