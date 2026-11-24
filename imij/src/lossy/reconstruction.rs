//! Version-independent borrowed reconstruction parameters. Parsing, model
//! selection and record scheduling stay with their owning wire drivers.

use super::*;

#[derive(Clone, Copy)]
pub(super) struct Reconstruction<'a> {
    pub info: &'a ImageInfo,
    pub color: LossyColor,
    pub xyb_power: bool,
    pub inv_scale: [f32; 3],
    pub dq: &'a [[f32; 64]; 3],
    pub bias: &'a Option<alloc::boxed::Box<[[[f32; 64]; 2]; 3]>>,
    pub transform: &'a Option<alloc::boxed::Box<[[f32; 4]; 3]>>,
    pub layouts: [LossyChannelLayout; 3],
    pub layout_steps: &'a [[[f32; 64]; 2]; 3],
    pub xfer16: &'a Option<(Vec<f32>, f32)>,
    pub code0_skips: bool,
    pub clear_q: bool,
    pub mode: LossyEntropy,
}

impl LossyParsed<'_> {
    pub(super) fn reconstruction(&self) -> Reconstruction<'_> {
        Reconstruction {
            info: &self.info,
            color: self.color,
            xyb_power: false,
            inv_scale: self.inv_scale,
            dq: &self.dq,
            bias: &self.bias,
            transform: &self.transform,
            layouts: self.layouts,
            layout_steps: &self.layout_steps,
            xfer16: &self.xfer16,
            code0_skips: code0_skips(self),
            clear_q: true,
            mode: self.tables.mode,
        }
    }
}
