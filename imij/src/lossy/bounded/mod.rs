//! Two-pass lossy encoding with working storage bounded by one work band of
//! about 8 block rows, full width, plus the current DC stripe's DC image
//! (issue #14). The caller supplies the source rows of each band twice,
//! top to bottom: pass 1 measures what the head must state before the first
//! unit record (DC range, the tier's side statistics, entropy histograms);
//! pass 2 recomputes each band's coefficients and codes its records.
//!
//! The quantized coefficients are computed exactly as the whole-image
//! encoder's pass 1 computes them, so a file decodes to the same pixels as
//! [`super::encode_lossy`] with the same configuration. The coding choices
//! the whole-image encoder makes by comparing complete candidates are made
//! explicitly here and are lossless: explicit AC tables (no seed bank),
//! no compact restart states, the tier rule for the entropy mode at every
//! image size, and a nested DC image coded in the bounded lossless group
//! grammar with tables measured in pass 1, each group searching its own
//! predictors and decorrelation within the effort's masks.
//!
//! Supported: Gray, GrayAlpha, RGB and RGBA of u8, u16, u32, f16 or f32
//! samples (16-bit as XYB with a known transfer, else YCoCg; float as XYB;
//! declared significant bits and u32 through the whole-image encoder's
//! precision conversion, each band range-checked as it is read), channel
//! layouts (row-local), small transforms and superblock merges (chosen by
//! the selector under pass 4's prices), a decoder memory budget (planned
//! from the geometry), no AQ, no inter reference.
//! Efforts 9 and 10 add one source pass per AC
//! trellis rate-model pass (XYB images above 64 Ki pixels; smaller ones are
//! refused, as the whole-image encoder compares candidate codings there).
//! Exact blocks add two: pass 0 measures their prices over the whole image
//! (`exact::estimate_bits`' run-level tables and `exact::select_mask`'s
//! residual costs), and after the trellis passes a pass codes the nested
//! exact image's stripes, which the caller stores and pass 2 places in the
//! records of the units holding their first blocks. Every pass chooses the
//! same blocks, each lane's EOB context carried across bands. Superblock
//! merges (the Compact tier) add one: pass 4 measures the selector's prices
//! from the units' token counts before any merge, and passes 1 and 2 run the
//! selection stripe by stripe (`superblock::select::Selector`). Anything
//! else is refused with `Error::Unsupported`; nothing requested is dropped.
//!
//! The public entry point is `stream_encode::encode_lossy_bounded` (`std`);
//! the in-memory form below is the tests' oracle path.
#![cfg_attr(not(feature = "std"), allow(dead_code))]

use super::*;
use crate::stream_encode::{GroupEncodeConfig, NativeGroupEncodeSession as GroupEncodeSession};

#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) mod one_pass;

#[cfg(all(test, feature = "std", feature = "lab"))]
mod coding_context_experiment;

mod passes;
mod plan;
#[cfg(feature = "std")]
mod session;
#[cfg(feature = "std")]
mod stream;

#[cfg(any(test, feature = "std"))]
use passes::*;
use plan::*;
#[cfg(all(test, feature = "std"))]
use session::*;
#[cfg(feature = "std")]
use stream::*;

#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) use passes::component_one_pass_records;
#[cfg(all(test, feature = "encode"))]
pub(crate) use passes::encode_lossy_bounded;
#[cfg(feature = "std")]
pub(crate) use plan::bounded_working_bytes;
#[cfg(feature = "std")]
pub(crate) use session::{Session, SessionPlan, Step};
#[cfg(feature = "std")]
pub(crate) use stream::{Stream, StreamIndex, StreamIo, encode_lossy_bounded_to};

#[cfg(test)]
mod tests;
