//! Browser range access to a native image directory.
use crate::ranges::{BrowserReadAt, browser_source, region_rect};
use imij::planes::{ImageViewRole, PlaneSetLimits, PlaneSetReader, ViewRegistration};
use imij::v4::CodingProfile;
use js_sys::{Array, Function, Object, Uint8Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct RangePyramid {
    reader: PlaneSetReader<BrowserReadAt>,
}

#[wasm_bindgen]
impl RangePyramid {
    #[wasm_bindgen(constructor)]
    pub fn new(
        length: f64,
        read: Function,
        read_many: Option<Function>,
        workers: Option<u32>,
        verify: Option<bool>,
    ) -> Result<RangePyramid, JsError> {
        Self::open(length, read, read_many, workers, verify, None)
    }

    /// Open a catalog-certified view source under a V4 coding profile.
    /// Unread tiles remain unverified until selected for decoding.
    #[wasm_bindgen(js_name = openProfile)]
    pub fn open_profile(
        profile: &str,
        length: f64,
        read: Function,
        read_many: Option<Function>,
        workers: Option<u32>,
        verify: Option<bool>,
    ) -> Result<RangePyramid, JsError> {
        let profile = profile.parse().map_err(super::js_err)?;
        Self::open(length, read, read_many, workers, verify, Some(profile))
    }

    fn open(
        length: f64,
        read: Function,
        read_many: Option<Function>,
        workers: Option<u32>,
        verify: Option<bool>,
        profile: Option<CodingProfile>,
    ) -> Result<RangePyramid, JsError> {
        if !length.is_finite()
            || length < 1.0
            || length.fract() != 0.0
            || length > 9_007_199_254_740_991.0
        {
            return Err(JsError::new("invalid source length"));
        }
        let verify = verify.unwrap_or(true);
        let mut limits = PlaneSetLimits::default()
            .with_file_bytes(1 << 46)
            .with_plane_pixels(1 << 40)
            .with_work_bytes(16 << 20, 64 << 20)
            .with_decode_workers(workers.unwrap_or(1) as usize)
            .with_checksum_verification(verify)
            .with_scoped_pixel_verification(verify);
        if let Some(profile) = profile {
            limits = limits.with_v4_profile(profile);
        }
        let reader = PlaneSetReader::open(browser_source(length as u64, read, read_many), limits)
            .map_err(super::js_err)?;
        if reader.image_directory().is_none() {
            return Err(JsError::new("missing native image directory"));
        }
        if reader.checksum_coverage().checking {
            return Err(JsError::new(
                "native browser pyramid requires independent record checks",
            ));
        }
        Ok(Self { reader })
    }

    pub fn views(&mut self) -> Result<Array, JsError> {
        let views = self.reader.image_directory().unwrap().views();
        let entries: Vec<_> = views
            .iter()
            .map(|v| (v.id(), v.role(), v.name().to_owned(), v.registration()))
            .collect();
        let original = self.reader.image_directory().unwrap().original();
        let original = self.reader.view_info(original).map_err(super::js_err)?;
        let result = Array::new();
        for (id, role, name, registration) in entries {
            let info = self.reader.view_info(id).map_err(super::js_err)?;
            let view = Object::new();
            super::set_info(&view, &info);
            super::set(&view, "id", id.to_string());
            super::set(&view, "name", name);
            super::set(
                &view,
                "role",
                match role {
                    ImageViewRole::Original => "original",
                    ImageViewRole::Reduced => "reduced",
                    ImageViewRole::Preview => "preview",
                    _ => return Err(JsError::new("unknown native view role")),
                },
            );
            let edge_aligned = role == ImageViewRole::Original
                || ViewRegistration::resized(
                    original.width,
                    original.height,
                    info.width,
                    info.height,
                )
                .ok()
                .is_some_and(|expected| registration == Some(expected));
            super::set(&view, "edgeAligned", edge_aligned);
            result.push(&view);
        }
        Ok(result)
    }

    #[wasm_bindgen(js_name = regionPages)]
    pub fn region_pages(
        &self,
        id: u64,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<f64>, JsError> {
        let rect = region_rect(x, y, width, height)?;
        let pages = self
            .reader
            .view_region_pages(id, rect)
            .map_err(super::js_err)?;
        Ok(pages
            .into_iter()
            .flat_map(|r| [r.start as f64, r.end as f64])
            .collect())
    }

    #[wasm_bindgen(js_name = regionRanges)]
    pub fn region_ranges(
        &mut self,
        id: u64,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<f64>, JsError> {
        let rect = region_rect(x, y, width, height)?;
        let ranges = self
            .reader
            .view_region_ranges(id, rect)
            .map_err(super::js_err)?;
        Ok(ranges
            .into_iter()
            .flat_map(|r| [r.start as f64, r.end as f64])
            .collect())
    }

    #[wasm_bindgen(js_name = decodeRegion)]
    pub fn decode_region(
        &mut self,
        id: u64,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Uint8Array, JsError> {
        let rect = region_rect(x, y, width, height)?;
        let info = self.reader.view_info(id).map_err(super::js_err)?;
        if info.channels != imij::Channels::Rgb || info.sample != imij::SampleType::U8 {
            return Err(JsError::new("native range viewer requires RGB8"));
        }
        let stride = width as usize * 3;
        let mut pixels =
            super::buffers::filled(stride * height as usize, 0).map_err(super::js_err)?;
        self.reader
            .decode_view_region(id, rect, &mut pixels, stride)
            .map_err(super::js_err)?;
        Ok(Uint8Array::from(pixels.as_slice()))
    }
}
