//! A browser-owned range cache serves the native region reader's positional
//! reads. Opening walks dependent headers, so a missing range there returns an
//! error and the async JS driver fills it and retries. Regions are planned:
//! `regionRanges` names every byte `decodeRegion` reads, the driver supplies
//! them, and the decode runs once. The native reader and its checked index
//! remain the format authority.
use how_far_along::{Execution, Outcome, Phase, PhaseSpec, Report, Total};
use imij::planes::{Rect, ZencodecReadAt};
#[cfg(feature = "full")]
use js_sys::Uint8ClampedArray;
use js_sys::{Array, Float64Array, Function, Uint8Array};
use wasm_bindgen::prelude::*;
use zencodec::source::ReadAt;

/// Actual how-far phase snapshots, driven by observed browser work. Transfer
/// totals are revised as decoder-triggered byte ranges become known; no ETA or
/// synthetic decoder-internal progress is inferred from elapsed time.
#[wasm_bindgen]
pub struct ViewProgress {
    root: Phase,
    parts: [Phase; 4],
    planned: u64,
    transferred: u64,
}

#[wasm_bindgen]
impl ViewProgress {
    #[wasm_bindgen(constructor)]
    pub fn new(regions: u32, opening: bool) -> Result<ViewProgress, JsError> {
        let mut root = Phase::new("Visible window", Total::Unknown);
        let parts = root
            .split(
                Execution::Unspecified,
                [
                    PhaseSpec::new("Image index", 1, Total::Exact(u64::from(opening))),
                    PhaseSpec::new("HTTP ranges", 1, Total::Unknown),
                    PhaseSpec::new(
                        "WASM decode / decoded cache",
                        1,
                        Total::Exact(u64::from(regions)),
                    ),
                    PhaseSpec::new(
                        "Resize and display tiles",
                        1,
                        Total::Exact(u64::from(regions)),
                    ),
                ],
            )
            .map_err(super::js_err)?;
        Ok(Self {
            root,
            parts,
            planned: 0,
            transferred: 0,
        })
    }
    /// Distinguish decoded margin regions from regions actually resized for display.
    #[wasm_bindgen(js_name = configureRegions)]
    pub fn configure_regions(&mut self, decoded: u32, displayed: u32) -> Result<(), JsError> {
        self.parts[2]
            .set_total(Total::Exact(u64::from(decoded)))
            .map_err(super::js_err)?;
        self.parts[3]
            .set_total(Total::Exact(u64::from(displayed)))
            .map_err(super::js_err)
    }
    #[wasm_bindgen(js_name = planRanges)]
    pub fn plan_ranges(&mut self, bytes: u32) -> Result<(), JsError> {
        self.planned = self.planned.saturating_add(u64::from(bytes));
        self.parts[1]
            .set_total(Total::Estimated(self.planned))
            .map_err(super::js_err)?;
        self.parts[1].start().map_err(super::js_err)
    }
    pub fn advance(&mut self, phase: u32, count: u32) -> Result<(), JsError> {
        let part = self
            .parts
            .get_mut(phase as usize)
            .ok_or_else(|| JsError::new("progress phase"))?;
        part.reporter().advance(u64::from(count));
        if phase == 1 {
            self.transferred = self.transferred.saturating_add(u64::from(count));
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Result<String, JsError> {
        let mut out = String::new();
        self.root
            .observer()
            .snapshot()
            .write_json(&mut out)
            .map_err(super::js_err)?;
        Ok(out)
    }
    pub fn finish(&mut self, cancelled: bool) -> Result<(), JsError> {
        self.parts[1]
            .set_total(Total::Exact(self.transferred))
            .map_err(super::js_err)?;
        let outcome = if cancelled {
            Outcome::Cancelled
        } else {
            Outcome::Succeeded
        };
        for part in &mut self.parts {
            part.finish_with(outcome).map_err(super::js_err)?;
        }
        self.root.finish_with(outcome).map_err(super::js_err)
    }
}

pub(super) struct BrowserSource {
    length: u64,
    read: Function,
    read_many: Option<Function>,
}

pub(super) type BrowserReadAt = ZencodecReadAt<BrowserSource, fn(imij::Error) -> imij::Error>;

pub(super) fn browser_source(
    length: u64,
    read: Function,
    read_many: Option<Function>,
) -> BrowserReadAt {
    ZencodecReadAt::new(
        BrowserSource {
            length,
            read,
            read_many,
        },
        core::convert::identity,
    )
}

impl BrowserSource {
    fn check_range(&self, offset: u64, size: usize) -> Result<(), imij::Error> {
        if offset
            .checked_add(size as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(imij::Error::Read);
        }
        Ok(())
    }
}

impl ReadAt for BrowserSource {
    type Error = imij::Error;

    fn len(&mut self) -> Result<u64, imij::Error> {
        Ok(self.length)
    }
    fn len_hint(&self) -> Option<u64> {
        Some(self.length)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), imij::Error> {
        self.check_range(offset, out.len())?;
        // A non-array return is a range the JS cache does not hold yet; the
        // callback has queued it.
        let bytes = self
            .read
            .call2(
                &JsValue::NULL,
                &JsValue::from_f64(offset as f64),
                &JsValue::from_f64(out.len() as f64),
            )
            .map_err(|_| imij::Error::Read)?
            .dyn_into::<Uint8Array>()
            .map_err(|_| imij::Error::Read)?;
        if bytes.length() as usize != out.len() {
            return Err(imij::Error::Read);
        }
        bytes.copy_to(out);
        Ok(())
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), imij::Error> {
        if let Some(read_many) = &self.read_many {
            let mut pairs = Vec::new();
            let count = requests.len().checked_mul(2).ok_or(imij::Error::TooLarge)?;
            pairs
                .try_reserve_exact(count)
                .map_err(|_| imij::Error::TooLarge)?;
            for (offset, out) in requests.iter() {
                self.check_range(*offset, out.len())?;
                pairs.extend_from_slice(&[*offset as f64, out.len() as f64]);
            }
            let values = read_many
                .call1(&JsValue::NULL, &Float64Array::from(pairs.as_slice()))
                .map_err(|_| imij::Error::Read)?
                .dyn_into::<Array>()
                .map_err(|_| imij::Error::Read)?;
            if values.length() as usize != requests.len() {
                return Err(imij::Error::Read);
            }
            for (i, (_, out)) in requests.iter_mut().enumerate() {
                let bytes = values
                    .get(i as u32)
                    .dyn_into::<Uint8Array>()
                    .map_err(|_| imij::Error::Read)?;
                if bytes.length() as usize != out.len() {
                    return Err(imij::Error::Read);
                }
                bytes.copy_to(out);
            }
            return Ok(());
        }
        // Gather every missing independent span in the JS driver's pending set.
        let mut error = None;
        for (offset, bytes) in requests {
            if let Err(e) = self.read_exact_at(*offset, bytes) {
                error = Some(e);
            }
        }
        error.map_or(Ok(()), Err)
    }
}

/// One retained indexed RGB8 reader. The caller supplies bounded cached ranges,
/// not a whole-file buffer; no HTTP or filesystem behavior is hidden in Rust.
#[cfg(feature = "full")]
#[wasm_bindgen]
pub struct RangeImage {
    reader: imij::region::RegionReader<BrowserReadAt>,
}

#[wasm_bindgen]
#[cfg(feature = "full")]
impl RangeImage {
    #[wasm_bindgen(constructor)]
    pub fn new(length: f64, read: Function) -> Result<RangeImage, JsError> {
        if !length.is_finite()
            || length < 1.0
            || length.fract() != 0.0
            || length > 9_007_199_254_740_991.0
        {
            return Err(JsError::new("invalid source length"));
        }
        let limits = {
            let mut limits = imij::Limits::DEFAULT;
            limits.max_pixels = 16_777_216;
            limits.max_bytes = 128 << 20;
            limits.verify_checksum = true;
            limits
        };
        let reader = imij::region::RegionReader::open_with_limits(
            browser_source(length as u64, read, None),
            &limits,
        )
        .map_err(super::js_err)?;
        let info = reader.info();
        if info.channels != imij::Channels::Rgb || info.sample != imij::SampleType::U8 {
            return Err(JsError::new("range viewer requires RGB8"));
        }
        Ok(Self { reader })
    }

    pub fn info(&self) -> js_sys::Object {
        let out = js_sys::Object::new();
        super::set_info(&out, self.reader.info());
        super::set(&out, "indexed", self.reader.has_unit_table());
        super::set(
            &out,
            "unitOrdinals",
            self.reader
                .header()
                .features
                .contains(imij::Feature::UnitOrdinals),
        );
        out
    }

    /// Every byte range `decodeRegion` of this rectangle reads, as flat
    /// `[start, end, start, end, ...]` file offsets, sorted and merged. On an
    /// indexed image planning reads nothing; the caller fetches these ranges,
    /// serves them through its callback and decodes in one call.
    #[wasm_bindgen(js_name = regionRanges)]
    pub fn region_ranges(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<f64>, JsError> {
        let rect = region_rect(x, y, width, height)?;
        let ranges = self.reader.region_ranges(rect).map_err(super::js_err)?;
        Ok(ranges
            .iter()
            .flat_map(|r| [r.start as f64, r.end as f64])
            .collect())
    }

    /// Decode the same RGB8 region as opaque RGBA8 code values for presentation.
    /// No CICP/ICC color transform or resampling is applied.
    #[wasm_bindgen(js_name = decodeRegionRgba)]
    pub fn decode_region_rgba(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Uint8ClampedArray, JsError> {
        let pixels = self.region_pixels(x, y, width, height)?;
        let mut info = self.reader.info().clone();
        info.width = width;
        info.height = height;
        let rgba = super::to_rgba8(&info, &pixels).map_err(super::js_err)?;
        Ok(Uint8ClampedArray::from(rgba.as_slice()))
    }

    #[wasm_bindgen(js_name = decodeRegion)]
    pub fn decode_region(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Uint8Array, JsError> {
        let pixels = self.region_pixels(x, y, width, height)?;
        Ok(Uint8Array::from(pixels.as_slice()))
    }
}

#[cfg(feature = "full")]
impl RangeImage {
    fn region_pixels(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, JsError> {
        let rect = region_rect(x, y, width, height)?;
        let stride = width as usize * 3;
        let mut pixels =
            super::buffers::filled(stride * height as usize, 0).map_err(super::js_err)?;
        self.reader
            .decode_region(rect, &mut pixels, stride)
            .map_err(super::js_err)?;
        Ok(pixels)
    }
}

/// The bounded rectangle every region method accepts.
pub(super) fn region_rect(x: u32, y: u32, width: u32, height: u32) -> Result<Rect, JsError> {
    if width == 0 || height == 0 || width > 16_384 || height > 1024 {
        return Err(JsError::new(
            "range region requires width 1..16384 and height 1..1024",
        ));
    }
    Ok(Rect::new(x, y, width, height))
}
