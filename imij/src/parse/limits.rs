//! Admission (SPEC-independent resource policy): the checkpoints every
//! parse passes (header, chunks, models, geometry) and the policies that
//! refuse a file at them. The output limits ([`Limits`]) are one policy,
//! checked right after the header on every route, so a file over them is
//! `TooLarge` before its chunks, checksum or payload are read, whatever else
//! is wrong with it; a [`crate::decode_resources::DecodeSession`] adds its
//! cumulative budgets.

use crate::{Error, ImageInfo, Limits, LossyChannelLayout, LossyColor};

/// Refuses an image over `limits` (pixels, decoded bytes).
pub(crate) fn check_limits(info: &ImageInfo, limits: &Limits) -> Result<(), Error> {
    let px = info.width as u64 * info.height as u64;
    let total = info.checked_buffer_len().ok_or(Error::TooLarge)?;
    if px > limits.max_pixels || total as u64 > limits.max_bytes {
        return Err(Error::TooLarge);
    }
    Ok(())
}

/// Coarse parser checkpoints; the unrestricted instantiation compiles away.
pub(crate) trait Admission {
    /// A V4 object/row step owns its request while suspended. Concrete request
    /// policies retain their cumulative counters and live model account.
    fn v4_resume(&self) -> crate::decode_resources::V4Resume {
        use crate::decode_resources::{DecodePolicy, DecodeSession};
        DecodeSession::new(
            DecodePolicy::default()
                .with_output_limits(Limits::NONE)
                .with_max_input_bytes(u64::MAX)
                .with_max_header_bytes(u64::MAX)
                .with_max_metadata_bytes(u64::MAX)
                .with_max_chunks(u64::MAX)
                .with_max_units(u64::MAX)
                .with_max_work_units(u64::MAX)
                .with_max_working_bytes(u64::MAX),
        )
        .v4_resume()
    }
    fn v4_commit(&self, _state: crate::decode_resources::V4Resume) -> Result<(), Error> {
        Ok(())
    }
    fn v4_cancel(&self) -> impl Fn() -> bool + Sync
    where
        Self: Sized,
    {
        || false
    }

    /// The parse precedes decoding, so samples this build cannot decode
    /// (signed integers and 32-/64-bit samples without `extended`) are refused at the head. [`Structure`]
    /// parses only find where a payload's parts end.
    #[inline]
    fn decodes(&self) -> bool {
        true
    }
    #[inline]
    fn check(&self) -> Result<(), Error> {
        Ok(())
    }
    #[inline]
    fn header(&self, _info: &ImageInfo, _bytes: usize) -> Result<(), Error> {
        Ok(())
    }
    fn coding_profile(&self, _header: &crate::Header) -> Result<(), Error> {
        Ok(())
    }
    #[inline]
    fn chunk_start(&self) -> Result<(), Error> {
        Ok(())
    }
    #[inline]
    fn chunk(&self, _tag: [u8; 4], _payload: &[u8]) -> Result<(), Error> {
        Ok(())
    }
    #[inline]
    fn model(&self) -> Result<(), Error> {
        Ok(())
    }
    fn quick(
        &self,
        _info: &ImageInfo,
        _rows: usize,
        _group: usize,
        _bits: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
    #[inline]
    fn lossless(
        &self,
        _info: &ImageInfo,
        _layout: &crate::coding_layout::Layout,
        _lanes: usize,
        _rows: usize,
        _group: usize,
        _palette: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
    /// Template palette tables (head record 35, SPEC 2.14) and their row
    /// states, after [`Admission::lossless`] charged the payload's ordinary
    /// states (`palette`: with palette expansion).
    #[inline]
    fn template(
        &self,
        _info: &ImageInfo,
        _layout: &crate::coding_layout::Layout,
        _lanes: usize,
        _rows: usize,
        _group: usize,
        _palette: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
    #[inline]
    fn lossy(
        &self,
        _info: &ImageInfo,
        _rows: u8,
        _group: usize,
        _color: LossyColor,
        _layouts: &[LossyChannelLayout; 3],
    ) -> Result<(), Error> {
        Ok(())
    }
    #[inline]
    fn local_ans(&self, _units: u128) -> Result<(), Error> {
        Ok(())
    }
    fn ac_models(&self, _models: u128) -> Result<(), Error> {
        Ok(())
    }
    /// Additional new-wire metadata/tables, for source-specific admission.
    fn wire_extra_bytes(&self, _bytes: u128) -> Result<(), Error> {
        Ok(())
    }
    #[inline]
    fn dc(&self, _parsed: &crate::v1::Parsed<'_>) -> Result<(), Error> {
        Ok(())
    }
    /// Supported decoded bitmap checksum bytes, admitted before output work.
    #[inline]
    fn pixel_checksum(&self, _info: &ImageInfo) -> Result<(), Error> {
        Ok(())
    }
    #[cfg(feature = "native-jpeg")]
    fn native(&self, _header: &crate::jpeg::NativeHeader, _rows: u16) -> Result<(), Error> {
        Ok(())
    }
    #[cfg(feature = "std")]
    #[cfg(feature = "native-jpeg")]
    fn native_buffer(&self, _bytes: usize) -> Result<(), Error> {
        Ok(())
    }
    /// The nested exact-block image of a lossy file (its head).
    #[inline]
    fn exact(&self, _parsed: &crate::v1::Parsed<'_>) -> Result<(), Error> {
        Ok(())
    }
    /// A unit table hint record held until the head names its count (the
    /// lossy head's, `bytes` long).
    #[inline]
    fn hint(&self, _bytes: usize) -> Result<(), Error> {
        Ok(())
    }
    /// A value table of `entries` entries (map mode 9, SPEC 2.1), before it
    /// is read; its expansion table is part of [`Admission::lossless`].
    #[inline]
    fn value_table(&self, _entries: usize) -> Result<(), Error> {
        Ok(())
    }
}

pub(crate) struct Unrestricted;
impl Admission for Unrestricted {}

/// Structure-only parses (`container::locate` finding the end of a unit
/// table): unrestricted, and valid for files this build cannot decode, so
/// writing and remuxing do not depend on decoder features.
pub(crate) struct Structure;
impl Admission for Structure {
    #[inline]
    fn decodes(&self) -> bool {
        false
    }
}

/// `A` with output limits: a header over `limits` is `TooLarge`.
pub(crate) struct Limited<'l, A>(pub &'l Limits, pub A);

impl<A: Admission> Admission for Limited<'_, A> {
    fn v4_resume(&self) -> crate::decode_resources::V4Resume {
        self.1.v4_resume()
    }
    fn v4_commit(&self, state: crate::decode_resources::V4Resume) -> Result<(), Error> {
        self.1.v4_commit(state)
    }
    fn v4_cancel(&self) -> impl Fn() -> bool + Sync {
        self.1.v4_cancel()
    }

    #[inline]
    fn decodes(&self) -> bool {
        self.1.decodes()
    }
    #[inline]
    fn check(&self) -> Result<(), Error> {
        self.1.check()
    }
    #[inline]
    fn header(&self, info: &ImageInfo, bytes: usize) -> Result<(), Error> {
        check_limits(info, self.0)?;
        self.1.header(info, bytes)
    }
    #[inline]
    fn chunk_start(&self) -> Result<(), Error> {
        self.1.chunk_start()
    }
    #[inline]
    fn chunk(&self, tag: [u8; 4], payload: &[u8]) -> Result<(), Error> {
        self.1.chunk(tag, payload)
    }
    #[inline]
    fn model(&self) -> Result<(), Error> {
        self.1.model()
    }
    #[inline]
    fn quick(&self, info: &ImageInfo, rows: usize, group: usize, bits: bool) -> Result<(), Error> {
        self.1.quick(info, rows, group, bits)
    }
    fn lossless(
        &self,
        info: &ImageInfo,
        layout: &crate::coding_layout::Layout,
        lanes: usize,
        rows: usize,
        group: usize,
        palette: bool,
    ) -> Result<(), Error> {
        self.1.lossless(info, layout, lanes, rows, group, palette)
    }
    fn template(
        &self,
        info: &ImageInfo,
        layout: &crate::coding_layout::Layout,
        lanes: usize,
        rows: usize,
        group: usize,
        palette: bool,
    ) -> Result<(), Error> {
        self.1.template(info, layout, lanes, rows, group, palette)
    }
    #[inline]
    fn lossy(
        &self,
        info: &ImageInfo,
        rows: u8,
        group: usize,
        color: LossyColor,
        layouts: &[LossyChannelLayout; 3],
    ) -> Result<(), Error> {
        self.1.lossy(info, rows, group, color, layouts)
    }
    #[inline]
    fn local_ans(&self, units: u128) -> Result<(), Error> {
        self.1.local_ans(units)
    }
    fn ac_models(&self, models: u128) -> Result<(), Error> {
        self.1.ac_models(models)
    }
    fn wire_extra_bytes(&self, bytes: u128) -> Result<(), Error> {
        self.1.wire_extra_bytes(bytes)
    }
    #[inline]
    fn dc(&self, parsed: &crate::v1::Parsed<'_>) -> Result<(), Error> {
        self.1.dc(parsed)
    }
    #[inline]
    fn exact(&self, parsed: &crate::v1::Parsed<'_>) -> Result<(), Error> {
        self.1.exact(parsed)
    }
    #[inline]
    #[cfg(feature = "native-jpeg")]
    fn native(&self, header: &crate::jpeg::NativeHeader, rows: u16) -> Result<(), Error> {
        self.1.native(header, rows)
    }
    #[cfg(feature = "std")]
    #[cfg(feature = "native-jpeg")]
    fn native_buffer(&self, bytes: usize) -> Result<(), Error> {
        self.1.native_buffer(bytes)
    }
    fn hint(&self, bytes: usize) -> Result<(), Error> {
        self.1.hint(bytes)
    }
    #[inline]
    fn value_table(&self, entries: usize) -> Result<(), Error> {
        self.1.value_table(entries)
    }
    #[inline]
    fn pixel_checksum(&self, info: &ImageInfo) -> Result<(), Error> {
        self.1.pixel_checksum(info)
    }
}

/// Repeat structural carrier checks without charging an outer plan twice.
#[cfg(feature = "native-jpeg")]
pub(crate) struct Checked<'a>(pub &'a dyn Admission);
#[cfg(feature = "native-jpeg")]
impl Admission for Checked<'_> {
    fn check(&self) -> Result<(), Error> {
        self.0.check()
    }
    fn decodes(&self) -> bool {
        true
    }
}
#[cfg(feature = "native-jpeg")]
pub(crate) struct Callback<'a, F>(pub &'a F);
#[cfg(feature = "native-jpeg")]
impl<F: Fn() -> Result<(), Error>> Admission for Callback<'_, F> {
    fn check(&self) -> Result<(), Error> {
        (self.0)()
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;
    use crate::{Channels, SampleType};
    use core::cell::Cell;

    #[test]
    fn check_limits_bounds_pixels_and_bytes_inclusively() {
        let info = ImageInfo::new(10, 4, Channels::Rgb, SampleType::U16); // 40 px, 240 bytes
        let at = |max_pixels, max_bytes| {
            check_limits(
                &info,
                &Limits {
                    max_pixels,
                    max_bytes,
                    verify_checksum: true,
                },
            )
        };
        assert_eq!(at(40, 240), Ok(()));
        assert_eq!(at(39, 240), Err(Error::TooLarge));
        assert_eq!(at(40, 239), Err(Error::TooLarge));
        // A buffer length this host cannot hold is over any limit.
        let huge = ImageInfo::new(u32::MAX, u32::MAX, Channels::Rgba, SampleType::U16);
        assert_eq!(check_limits(&huge, &Limits::NONE), Err(Error::TooLarge));
    }

    /// Counts the checkpoints it sees.
    #[derive(Default)]
    struct Count {
        calls: Cell<u32>,
    }
    impl Count {
        fn hit(&self) -> Result<(), Error> {
            self.calls.set(self.calls.get() + 1);
            Ok(())
        }
    }
    impl Admission for Count {
        fn decodes(&self) -> bool {
            false
        }
        fn check(&self) -> Result<(), Error> {
            self.hit()
        }
        fn header(&self, _: &ImageInfo, _: usize) -> Result<(), Error> {
            self.hit()
        }
        fn chunk_start(&self) -> Result<(), Error> {
            self.hit()
        }
        fn chunk(&self, _: [u8; 4], _: &[u8]) -> Result<(), Error> {
            self.hit()
        }
        fn model(&self) -> Result<(), Error> {
            self.hit()
        }
        fn dc(&self, _: &crate::v1::Parsed<'_>) -> Result<(), Error> {
            self.hit()
        }
    }

    #[test]
    fn limited_adds_the_header_limit_and_forwards_the_rest() {
        let small = Limits {
            max_pixels: 10,
            max_bytes: 1 << 20,
            verify_checksum: true,
        };
        let fits = ImageInfo::new(2, 5, Channels::Gray, SampleType::U8);
        let over = ImageInfo::new(3, 5, Channels::Gray, SampleType::U8);
        let l = Limited(&small, Count::default());
        assert!(!l.decodes(), "decodes() comes from the inner policy");
        l.check().unwrap();
        l.header(&fits, 9).unwrap();
        l.chunk_start().unwrap();
        l.chunk(*b"iCCP", b"x").unwrap();
        l.model().unwrap();
        assert_eq!(l.1.calls.get(), 5);
        // Over the limits: refused before the inner policy sees the header.
        assert_eq!(l.header(&over, 9), Err(Error::TooLarge));
        assert_eq!(l.1.calls.get(), 5);
        assert!(Unrestricted.decodes());
        assert!(!Structure.decodes());
        assert_eq!(Unrestricted.header(&over, 9), Ok(()));
    }
}
