//! Caller-drained V4 numeric files without an image-sized index.
use super::{NumericGroupEncoder, ObjectIntegrity, SpatialConfig, TileLimits, TileOrder};
use crate::float_guarantees::{self, EMPTY};
use crate::stream_encode::{GroupEncodeConfig, GroupRect};
use crate::{Error, FloatGuarantees, ImageInfo, ImageRef, ImiEncoderConfig, Metadata};
use alloc::vec::Vec;

/// Caller-drained, fixed-group V4 lossless/Quick output. Drain the header and
/// every completed group before supplying the next rectangle. Tile markers are
/// published with their adjacent group, preserving the single-drain contract.
/// No directory or file checksum is accumulated; explicit indexed/checksummed
/// writers use caller-owned scratch. Sample bits and strided rows are retained.
pub struct GroupEncodeSession {
    encoder: NumericGroupEncoder,
    output: Vec<u8>,
    consumed: usize,
    tiles: u64,
    failed: bool,
    guarantees: [FloatGuarantees; 4],
}
impl GroupEncodeSession {
    pub fn new(info: ImageInfo, meta: &Metadata, cfg: &ImiEncoderConfig) -> Result<Self, Error> {
        Self::with_groups(info, meta, GroupEncodeConfig::required(cfg)?)
    }
    /// Declare float guarantees in the opening header and verify every
    /// supplied group before publishing its bytes. A rejected group can be
    /// retried. As with an ordinary session, only a finished stream is a file.
    pub fn new_with_float_guarantees(
        info: ImageInfo,
        meta: &Metadata,
        cfg: &ImiEncoderConfig,
        guarantees: [FloatGuarantees; 4],
    ) -> Result<Self, Error> {
        if guarantees != EMPTY && !cfg.is_exact() {
            return Err(Error::Unsupported(
                "float guarantees require exact encoding",
            ));
        }
        Self::with_groups_and_guarantees(info, meta, GroupEncodeConfig::required(cfg)?, guarantees)
    }

    pub(crate) fn with_groups(
        info: ImageInfo,
        meta: &Metadata,
        groups: GroupEncodeConfig,
    ) -> Result<Self, Error> {
        Self::with_groups_and_guarantees(info, meta, groups, EMPTY)
    }

    fn with_groups_and_guarantees(
        info: ImageInfo,
        meta: &Metadata,
        mut groups: GroupEncodeConfig,
        guarantees: [FloatGuarantees; 4],
    ) -> Result<Self, Error> {
        float_guarantees::check_declaration(&info, guarantees)?;
        crate::stream_encode::NativeGroupEncodeSession::check_working(&info, meta, groups)?;
        let frame = (groups.max_working_bytes / 8).max(1024);
        groups.max_working_bytes = groups
            .max_working_bytes
            .checked_sub(frame)
            .ok_or(Error::TooLarge)?;
        let mut cfg = ImiEncoderConfig::lossless().with_row_groups(crate::RowGroups::new(
            groups.columns,
            groups.rows,
            groups.max_working_bytes,
        ));
        if groups.quick_pixels {
            cfg = cfg.with_quick(groups.quick_bit_smarts);
        }
        let spatial = SpatialConfig {
            tile_columns: info.width.div_ceil(groups.columns),
            tile_rows: 1,
            order: TileOrder::Raster,
            integrity: ObjectIntegrity::Objects,
            limits: TileLimits {
                max_object_bytes: frame as u64,
                max_tile_bytes: u64::MAX,
            },
        };
        let (mut encoder, setup) = NumericGroupEncoder::new(info.clone(), &cfg, spatial)?;
        let mut output = Vec::new();
        crate::container::write_group_head_features(
            &mut output,
            &info,
            meta,
            crate::Coding::Lossless,
            None,
            crate::DecodeSpeed::default(),
            encoder.features(),
            4,
        )?;
        let mut output = float_guarantees::write_header(output, &info, guarantees)?;
        super::emit::extend(&mut output, b"DATA\0", usize::MAX)?;
        super::emit::extend(&mut output, &setup, usize::MAX)?;
        encoder.begin_tile()?;
        super::emit::extend(&mut output, encoder.output(), usize::MAX)?;
        encoder.consume_output(encoder.output().len())?;
        Ok(Self {
            encoder,
            output,
            consumed: 0,
            tiles: 0,
            failed: false,
            guarantees,
        })
    }
    pub fn output(&self) -> &[u8] {
        &self.output[self.consumed..]
    }
    pub fn consume_output(&mut self, count: usize) -> Result<(), Error> {
        if count > self.output().len() {
            return Err(Error::InvalidInput("output drain exceeds pending bytes"));
        }
        self.consumed += count;
        if self.consumed == self.output.len() {
            self.output.clear();
            self.consumed = 0;
        }
        Ok(())
    }
    pub fn next_group(&self) -> Option<GroupRect> {
        if self.failed || !self.output().is_empty() {
            None
        } else {
            self.encoder.next_group()
        }
    }
    fn drain(&mut self) -> Result<(), Error> {
        super::emit::extend(&mut self.output, self.encoder.output(), usize::MAX)?;
        self.encoder.consume_output(self.encoder.output().len())
    }
    pub fn push_group(&mut self, pixels: &[u8], stride: usize) -> Result<(), Error> {
        let rect = self.next_group().ok_or(Error::InvalidInput(
            "group needs output drainage or is finished",
        ))?;
        if self.guarantees != EMPTY {
            let mut local = self.encoder.image_info().clone();
            local.width = rect.width;
            local.height = rect.height;
            float_guarantees::verify_source(
                ImageRef::new(pixels, stride, &local),
                self.guarantees,
            )?;
        }
        self.encoder.push_group(pixels, stride)?;
        let result = (|| {
            self.drain()?;
            if self.encoder.next_group().is_none() {
                self.encoder.end_tile()?;
                self.drain()?;
                self.tiles += 1;
                if self.tiles < self.encoder.layout().tile_count() {
                    self.encoder.begin_tile()?;
                    self.drain()?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn is_finished(&self) -> bool {
        !self.failed && self.tiles == self.encoder.layout().tile_count() && self.output().is_empty()
    }
    pub fn finish(self) -> Result<(), Error> {
        if !self.is_finished() {
            return Err(Error::InvalidInput("incomplete group image"));
        }
        self.encoder.finish()
    }
}
