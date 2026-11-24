//! Executing spatial-request experiment over the production pyramid/region APIs.
//!
//! Coordinates are stored samples at the explicitly selected level. The source
//! must provide one immutable representation. Cancellation/view validity is
//! checked separately, including immediately before returning private output.
//! Header facts are not complete ICC/metadata interpretation or a privacy proof.
use imij::{
    ImageInfo,
    planes::{ReadAt, Rect, SliceReader},
    region::RegionReader,
};
use imij_set::{PyramidCatalog, PyramidLevel, Reader, TileOrder};
use std::io;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn zeroed(length: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(io::Error::other)?;
    bytes.resize(length, 0);
    Ok(bytes)
}

#[derive(Clone, Copy)]
pub struct Selection {
    pub level: u32,
    pub rect: Rect,
}
#[derive(Clone, Copy)]
pub enum Delivery {
    NativeRanges,
    WholeEntries,
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub tiles: usize,
    pub output_bytes: usize,
    pub staged_encoded_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            tiles: 4096,
            output_bytes: 64 * 1024 * 1024,
            staged_encoded_bytes: 16 * 1024 * 1024,
        }
    }
}
pub struct StoredRegion {
    /// Header facts only; metadata payloads and auxiliary dependencies were not
    /// inspected. No rendering interpretation or display conversion is implied.
    pub info: ImageInfo,
    pub orientation: u8,
    pub bytes: Vec<u8>,
    pub stride: usize,
}
/// One image/pyramid in one already-selected source or nested archive window.
pub struct PyramidReader<S> {
    source: S,
    archive: Reader<S>,
    catalog: PyramidCatalog,
    limits: Limits,
}
impl<S: ReadAt + Clone> PyramidReader<S> {
    pub fn open(source: S, limits: Limits) -> io::Result<Self> {
        let mut archive = Reader::open(source.clone())?;
        let entry = archive
            .get(imij_set::CATALOG_KEY)?
            .ok_or_else(|| invalid("pyramid catalog missing"))?;
        let catalog = PyramidCatalog::decode(&archive.read_entry_bounded(&entry, 64 * 1024)?)?;
        Ok(Self {
            source,
            archive,
            catalog,
            limits,
        })
    }
    /// Stage the whole requested output privately. Failed reads, cancellation,
    /// mixed layouts or stale view/source checks return no partial region.
    pub fn decode(
        &mut self,
        selection: Selection,
        delivery: Delivery,
        mut check: impl FnMut() -> io::Result<()>,
    ) -> io::Result<StoredRegion> {
        check()?;
        let level = self
            .catalog
            .level(selection.level)
            .ok_or_else(|| invalid("unknown pyramid level"))?
            .clone();
        let r = selection.rect;
        let end_x = r
            .x()
            .checked_add(r.width())
            .filter(|&v| v <= level.width && r.width() != 0)
            .ok_or_else(|| invalid("region outside level"))?;
        let end_y = r
            .y()
            .checked_add(r.height())
            .filter(|&v| v <= level.height && r.height() != 0)
            .ok_or_else(|| invalid("region outside level"))?;
        let columns = (end_x - 1) / level.tile_width - r.x() / level.tile_width + 1;
        let rows = (end_y - 1) / level.tile_height - r.y() / level.tile_height + 1;
        if u64::from(columns) * u64::from(rows) > self.limits.tiles as u64 {
            return Err(invalid("spatial tile allowance"));
        }
        // Even a single one-byte channel cannot fit. Refuse before index work.
        let pixels = u64::from(r.width()) * u64::from(r.height());
        if pixels > self.limits.output_bytes as u64 {
            return Err(invalid("spatial output allowance"));
        }
        let keys = level.keys_for_rect(r.x(), r.y(), r.width(), r.height())?;
        let entries = self
            .archive
            .get_many(&keys)?
            .into_iter()
            .map(|e| e.ok_or_else(|| invalid("pyramid tile missing")))
            .collect::<io::Result<Vec<_>>>()?;
        let mut encoded = Vec::new();
        if matches!(delivery, Delivery::WholeEntries) {
            let mut total = 0usize;
            for entry in &entries {
                // The native reader checks snapshot provenance and bounds.
                let _source = self.archive.entry_source(entry)?;
                let size =
                    usize::try_from(entry.len()).map_err(|_| invalid("tile length overflow"))?;
                total = total
                    .checked_add(size)
                    .filter(|&n| n <= self.limits.staged_encoded_bytes)
                    .ok_or_else(|| invalid("encoded staging allowance"))?;
            }
            encoded
                .try_reserve_exact(entries.len())
                .map_err(io::Error::other)?;
            for entry in &entries {
                encoded.push(zeroed(entry.len() as usize)?);
            }
            let mut reads: Vec<_> = entries
                .iter()
                .zip(&mut encoded)
                .map(|(entry, bytes)| (entry.offset(), bytes.as_mut_slice()))
                .collect();
            check()?;
            self.source
                .read_many_at(&mut reads)
                .map_err(io::Error::other)?;
            for (entry, bytes) in entries.iter().zip(&encoded) {
                if entry.checksum().algorithm().digest(bytes) != entry.checksum() {
                    return Err(invalid("pyramid entry checksum"));
                }
            }
        }
        let mut output = None;
        for (i, entry) in entries.iter().enumerate() {
            check()?;
            if matches!(delivery, Delivery::WholeEntries) {
                let mut tile =
                    RegionReader::open(SliceReader::new(&encoded[i])).map_err(io::Error::other)?;
                decode_tile(
                    &mut tile,
                    &level,
                    entry.key(),
                    r,
                    self.limits.output_bytes,
                    &mut output,
                )?;
            } else {
                let mut tile = RegionReader::open(self.archive.entry_source(entry)?)
                    .map_err(io::Error::other)?;
                decode_tile(
                    &mut tile,
                    &level,
                    entry.key(),
                    r,
                    self.limits.output_bytes,
                    &mut output,
                )?;
            }
        }
        check()?;
        output.ok_or_else(|| invalid("empty spatial result"))
    }
}

fn decode_tile<S: ReadAt>(
    tile: &mut RegionReader<S>,
    level: &PyramidLevel,
    key: u64,
    request: Rect,
    limit: usize,
    output: &mut Option<StoredRegion>,
) -> io::Result<()> {
    let index = key - level.key_base;
    let (tx, ty) = match level.order {
        TileOrder::RowMajor => (
            (index % u64::from(level.cols())) as u32,
            (index / u64::from(level.cols())) as u32,
        ),
        TileOrder::Morton => imij_set::morton_decode_2d(index),
        _ => return Err(invalid("unsupported tile order")),
    };
    let (x, y) = (tx * level.tile_width, ty * level.tile_height);
    let (width, height) = (
        (level.width - x).min(level.tile_width),
        (level.height - y).min(level.tile_height),
    );
    if tile.info().width != width || tile.info().height != height {
        return Err(invalid("tile geometry disagrees with catalog"));
    }
    let mut info = tile.info().clone();
    info.width = request.width();
    info.height = request.height();
    let orientation = tile.header().orientation;
    let bpp = info.channels.count() * info.sample.bytes();
    if let Some(previous) = output {
        if previous.info != info || previous.orientation != orientation {
            return Err(invalid("inconsistent tile sample interpretation"));
        }
    } else {
        let stride = (request.width() as usize)
            .checked_mul(bpp)
            .ok_or_else(|| invalid("output stride overflow"))?;
        let length = stride
            .checked_mul(request.height() as usize)
            .filter(|&n| n <= limit)
            .ok_or_else(|| invalid("spatial output allowance"))?;
        *output = Some(StoredRegion {
            info,
            orientation,
            bytes: zeroed(length)?,
            stride,
        });
    }
    let (left, top) = (x.max(request.x()), y.max(request.y()));
    let (right, bottom) = (
        (x + width).min(request.x() + request.width()),
        (y + height).min(request.y() + request.height()),
    );
    let rect = Rect::new(left - x, top - y, right - left, bottom - top);
    let output = output.as_mut().unwrap();
    let start = (top - request.y()) as usize * output.stride + (left - request.x()) as usize * bpp;
    // Native stride support writes directly into the private region canvas;
    // there is no extra decoded tile canvas or full-image decode/crop.
    tile.decode_region(rect, &mut output.bytes[start..], output.stride)
        .map_err(io::Error::other)
}
