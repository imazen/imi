//! Browser bindings for the production imij-set reader.
use crate::ranges::{BrowserReadAt, browser_source};
use crate::{js_err, set, set_info};
use imij_set::{Entry, Reader, SequenceIndexRef, Value};
use js_sys::{Array, Function, Object, Uint8Array};
use wasm_bindgen::prelude::*;

const MAX_ENTRIES: u32 = 128;
const MAX_PAYLOAD: usize = 32 << 20;

// This covers retained archives whose earlier sequence index is migration-only.
// The complete encoded blob has already passed its archive digest check.
fn decode_buffered_item(
    file: &[u8],
    ordinal: u64,
    limits: &imij::Limits,
) -> Result<(imij::ImageInfo, Vec<u8>, Option<Vec<u8>>), imij::Error> {
    let mut decoder = imij::FrameDecoder::with_limits(file, limits)?;
    let mut index = usize::try_from(ordinal).map_err(|_| imij::Error::TooLarge)?;
    if decoder.kind() == imij::FrameKind::Animation && index != 0 {
        return Err(imij::Error::Unsupported("buffered indexed animation item"));
    }
    let mut chain = Vec::new();
    loop {
        if chain.len() == 16 {
            return Err(imij::Error::TooLarge);
        }
        decoder.seek_frame(index)?;
        chain.push(index);
        let Some(reference) = decoder.reference_index() else {
            break;
        };
        index = reference;
    }
    let mut pixels = Vec::new();
    let mut info = decoder.info().clone();
    for index in chain.into_iter().rev() {
        decoder.seek_frame(index)?;
        info = decoder
            .frame_info()
            .ok_or(imij::Error::Corrupt("missing indexed item"))?;
        let (stride, size) = crate::buffers::layout(&info, 0)?;
        if size > MAX_PAYLOAD {
            return Err(imij::Error::TooLarge);
        }
        if decoder.reference_index().is_none() {
            pixels = crate::buffers::filled(size, 0)?;
        } else if pixels.len() != size {
            return Err(imij::Error::Corrupt("indexed reference layout"));
        }
        decoder.decode_into(&mut pixels, stride, 1)?;
        if decoder.pixel_checksum_status() == imij::PixelChecksumStatus::Unsupported {
            return Err(imij::Error::Unsupported("indexed item pixel checksum"));
        }
    }
    let icc = decoder.frame_metadata().and_then(|meta| meta.icc.clone());
    Ok((info, pixels, icc))
}

fn describe(entry: &Entry) -> Object {
    let item = Object::new();
    let meta = entry.metadata();
    let summary = entry.summary();
    set(&item, "key", entry.key().to_string());
    set(&item, "offset", entry.offset() as f64);
    set(&item, "bytes", entry.len() as f64);
    set(&item, "name", meta.filename.as_deref().unwrap_or(""));
    set(&item, "width", summary.width);
    set(&item, "height", summary.height);
    set(&item, "sample", u32::from(summary.sample));
    set(&item, "channels", u32::from(summary.channels));
    set(&item, "coding", u32::from(summary.coding));
    set(&item, "digest", format!("{:?}", entry.checksum()));
    set(
        &item,
        "digestId",
        u32::from(entry.checksum().algorithm().wire()),
    );
    set(&item, "digestBytes", format!("{:x}", entry.checksum()));
    if let Some(count) = summary.image_count {
        set(&item, "imageCount", count.to_string());
    }
    if let Some(cicp) = summary.cicp {
        let color = Object::new();
        set(&color, "primaries", u32::from(cicp.color_primaries));
        set(&color, "transfer", u32::from(cicp.transfer_characteristics));
        set(&color, "matrix", u32::from(cicp.matrix_coefficients));
        set(&color, "fullRange", cicp.full_range);
        set(&item, "cicp", color);
    }
    let attributes = Array::new();
    for (name, value) in &meta.attributes {
        let attr = Object::new();
        set(&attr, "name", name.as_str());
        match value {
            Value::Text(value) => set(&attr, "value", value.as_str()),
            Value::Unsigned(value) => set(&attr, "value", value.to_string()),
            Value::Integer(value) => set(&attr, "value", value.to_string()),
            Value::Float(value) => set(&attr, "value", *value),
            Value::Bool(value) => set(&attr, "value", *value),
            Value::Bytes(value) => set(&attr, "value", format!("{} bytes", value.len())),
            _ => continue,
        }
        attributes.push(&attr);
    }
    set(&item, "attributes", attributes);
    item
}

#[wasm_bindgen]
pub struct RangeArchive {
    reader: Reader<BrowserReadAt>,
}

#[wasm_bindgen]
impl RangeArchive {
    #[wasm_bindgen(constructor)]
    pub fn new(length: f64, read: Function, read_many: Function) -> Result<RangeArchive, JsError> {
        if !length.is_finite()
            || length < 1.0
            || length.fract() != 0.0
            || length > 9_007_199_254_740_991.0
        {
            return Err(JsError::new("invalid archive length"));
        }
        let reader =
            Reader::open(browser_source(length as u64, read, Some(read_many))).map_err(js_err)?;
        Ok(Self { reader })
    }

    pub fn snapshot(&self) -> Object {
        let snapshot = self.reader.snapshot();
        let result = Object::new();
        set(&result, "generation", snapshot.generation().to_string());
        set(&result, "bytes", snapshot.end() as f64);
        result
    }

    pub fn list(&mut self, start: u64, count: u32) -> Result<Array, JsError> {
        if count == 0 || count > MAX_ENTRIES {
            return Err(JsError::new("invalid archive page size"));
        }
        let mut listing = self.reader.listing_from(start).map_err(js_err)?;
        let items = Array::new();
        for _ in 0..count {
            let Some(entry) = listing.next(&mut self.reader).map_err(js_err)? else {
                break;
            };
            items.push(&describe(&entry));
        }
        Ok(items)
    }

    pub fn entries(&mut self, keys: Vec<u64>) -> Result<Array, JsError> {
        if keys.len() > MAX_ENTRIES as usize {
            return Err(JsError::new("archive key batch is too large"));
        }
        let items = Array::new();
        for entry in self.reader.get_many(&keys).map_err(js_err)? {
            match entry {
                Some(entry) => items.push(&describe(&entry)),
                None => items.push(&JsValue::NULL),
            };
        }
        Ok(items)
    }

    #[wasm_bindgen(js_name = readEntry)]
    pub fn read_entry(&mut self, key: u64, max_bytes: usize) -> Result<Vec<u8>, JsError> {
        let entry = self.entry(key)?;
        self.reader
            .read_entry_bounded(&entry, max_bytes.min(MAX_PAYLOAD))
            .map_err(js_err)
    }

    #[wasm_bindgen(js_name = decodeItem)]
    pub fn decode_item(&mut self, key: u64, ordinal: u64) -> Result<Object, JsError> {
        let entry = self.entry(key)?;
        let mut limits = imij::Limits::DEFAULT;
        limits.max_bytes = MAX_PAYLOAD as u64;
        let index = match SequenceIndexRef::from_metadata(entry.metadata()) {
            Ok(index) => index,
            Err(_)
                if matches!(
                    entry.metadata().attributes.get("imij.sequence-index"),
                    Some(Value::Bytes(bytes)) if bytes.len() == 25 && bytes[0] == 1
                ) =>
            {
                None
            }
            Err(error) => return Err(js_err(error)),
        };
        let (info, pixels, icc) = if index.is_some() {
            let (info, pixels) = self
                .reader
                .decode_indexed_item_verified(&entry, ordinal, 16, MAX_PAYLOAD, &limits, 1)
                .map_err(js_err)?;
            (info, pixels, None)
        } else {
            let file = self
                .reader
                .read_entry_bounded(&entry, MAX_PAYLOAD)
                .map_err(js_err)?;
            decode_buffered_item(&file, ordinal, &limits).map_err(js_err)?
        };
        let result = Object::new();
        set_info(&result, &info);
        set(&result, "stride", info.row_bytes() as u32);
        let color = Array::of4(
            &JsValue::from(info.cicp.color_primaries),
            &JsValue::from(info.cicp.transfer_characteristics),
            &JsValue::from(info.cicp.matrix_coefficients),
            &JsValue::from(info.cicp.full_range),
        );
        set(&result, "cicp", color);
        set(&result, "alphaMode", format!("{:?}", info.alpha_mode));
        let bits = Array::new();
        for channel in 0..info.channels.count() {
            bits.push(&JsValue::from(info.channel_bits(channel).unwrap()));
        }
        set(&result, "significantBits", bits);
        set(&result, "byteOrder", "little-endian");
        set(&result, "pixels", Uint8Array::from(pixels.as_slice()));
        if let Some(icc) = icc {
            set(&result, "icc", Uint8Array::from(icc.as_slice()));
        }
        Ok(result)
    }
}

impl RangeArchive {
    fn entry(&mut self, key: u64) -> Result<Entry, JsError> {
        self.reader
            .get(key)
            .map_err(js_err)?
            .ok_or_else(|| JsError::new("archive entry not found"))
    }
}
