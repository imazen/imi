//! Surrounding-block adaptive RDO, exact 8-state row-strip partition-map DP,
//! empirical pilot-pass table learning, and zero-decode-overhead perceptual
//! trellis-lambda AQ (open-loop + closed-loop unfiltered-seam residual
//! feedback) for the lossy encoder.
//!
//! All mechanisms operate strictly within the existing V4 bitstream format:
//! zero wire changes, zero decoder changes, fallible allocations returning
//! `Error::TooLarge`, deterministic scalar f64/f32 arithmetic across CPU
//! targets, and bit-identical output across encoder thread counts.

mod closed_loop;
mod fallible;
mod map_dp;
mod open_loop;

#[cfg(all(test, feature = "encode"))]
mod tests;

pub(super) use closed_loop::{
    CLOSED_LOOP_EXACT_MAX_BLOCKS, ClosedLoopLambdaModel, ClosedLoopLambdaStream,
    closed_loop_stripe_errors, update_lambda_map_closed_loop,
};
pub(super) use fallible::{try_push, try_vec, try_vec_cap};
pub(super) use map_dp::{
    FlagStats, LearnedFlags, MapBitStats, MapBitTable, solve_row_strip_dp, sym_class,
};
pub(super) use open_loop::{
    OpenLoopBandCursor, OpenLoopLambdaModel, OpenLoopLambdaStream, build_open_loop_lambda_map,
};
