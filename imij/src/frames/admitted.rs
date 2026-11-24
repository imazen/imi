//! Adapter hooks share native rendering and cumulative admission.
use super::*;
use crate::parse::limits::Admission;

impl FrameDecoder<'_> {
    /// Last consumers and common-interface timing; no image payload is decoded.
    pub(crate) fn animation_plan(
        &self,
        admission: &impl Admission,
    ) -> Result<(Vec<Option<usize>>, Vec<FrameDuration>), Error> {
        let count = self.frame_count();
        let bytes = count
            .checked_mul(
                core::mem::size_of::<Option<usize>>() + core::mem::size_of::<FrameDuration>(),
            )
            .ok_or(Error::TooLarge)?;
        admission.hint(bytes)?;
        let mut last = Vec::new();
        last.try_reserve_exact(count).map_err(|_| Error::TooLarge)?;
        last.resize(count, None);
        let mut durations = Vec::new();
        durations
            .try_reserve_exact(count)
            .map_err(|_| Error::TooLarge)?;
        durations.push(self.first_duration);
        for (index, span) in self.frames.iter().enumerate() {
            admission.check()?;
            let control = frame_control(&self.data[span.clone()], self.version)?;
            durations.push(control.duration);
            // Legacy frames need only the current canvas. A unified reference
            // may outlive that canvas, so retain it until its final consumer.
            if self.unified
                && let Some((reference, _)) = control.reference
            {
                let reference = usize::try_from(reference).map_err(|_| Error::TooLarge)?;
                if reference > index {
                    return Err(Error::Corrupt("reference is not an earlier image"));
                }
                last[reference] = Some(index + 1);
            }
        }
        Ok((last, durations))
    }

    pub(crate) fn decode_into_admitted(
        &mut self,
        out: &mut [u8],
        stride: usize,
        admission: &impl Admission,
        check: &(impl Fn() -> Result<(), Error> + Sync),
    ) -> Result<(), Error> {
        self.pixel_status = self.pixelsum.view(&self.data).status();
        let (header, _) = self
            .cur
            .as_ref()
            .ok_or(Error::InvalidInput("no current frame"))?;
        let ordinal = header.index;
        let info = if self.kind == FrameKind::Animation {
            self.canvas.clone()
        } else {
            self.frame_info()
                .ok_or(Error::InvalidInput("no current frame"))?
        };
        let table = self.pixelsum.view(&self.data);
        if matches!(table, crate::pixelsum::Table::Known(..)) {
            admission.hint(core::mem::size_of::<crate::pixelsum::Raster>())?;
            admission.pixel_checksum(&info)?;
        }
        if self.unified
            && let Some((index, hash)) = self.reference
        {
            admission.hint(core::mem::size_of::<crate::pixelsum::Raster>())?;
            admission.pixel_checksum(&info)?;
            if crate::pixelsum::digest_checked(
                hash.algorithm(),
                &info,
                index as u64,
                out,
                stride,
                &mut || check(),
            )? != hash
            {
                return Err(Error::Corrupt("reference raster checksum mismatch"));
            }
        }
        self.decode_current_with(out, stride, |file, out, stride, inter, inherited| {
            crate::v1::decode_parsed_frame_admitted(
                file, out, stride, inter, inherited, admission, check,
            )
        })?;
        table.verify_checked(ordinal, &info, out, stride, check)?;
        check()?;
        if matches!(table, crate::pixelsum::Table::Known(..)) {
            self.pixel_status = crate::pixelsum::PixelChecksumStatus::Verified;
        }
        Ok(())
    }
}
