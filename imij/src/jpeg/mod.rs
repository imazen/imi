//! Native signed 8x8 component coefficients and exact integer quantizers.
//!
//! Historical v2 experiment, available only with `native-jpeg` (off by default).
//! V3 and encoded-domain upgrades reject this syntax in every build.
//! Experimental opt-in storage: current measured reconstruction is slower than
//! sequential JPEG with restart markers. These explicit coefficient APIs are
//! not selected by the ordinary image encoder. V2 files remain decodable only
//! with this feature; experimental writers emit v2.
//!
//! These APIs accept decoded coefficients; they do not parse JPEG markers or
//! recreate JPEG bytes. Sampling grids, component identities, table slots and
//! source siting stay available for intermediate use. Gray, 4:4:4 and centered
//! 4:2:0 have one reconstruction: IMI AAN, component clamp, whole-plane triangle
//! and existing output rounding. The study's observed worst RGB error 3 against
//! libjpeg is not a universal bound. No correction layer is applied.

mod archive;
pub use archive::NativeArchive;
mod components;
pub(crate) mod wire;
pub use components::{
    ChromaSiting, CoefficientImage, ComponentColor, ComponentInfo, QuantizedComponent,
};
#[cfg(feature = "encode")]
pub use wire::BandEncoder;
pub use wire::{NativeHeader, decode_coefficients};

#[cfg(all(test, feature = "encode"))]
mod tests;
