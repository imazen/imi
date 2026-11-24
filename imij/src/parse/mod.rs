//! The parse layer: every reader of the format above the entropy kernels.
//!
//! [`header`] reads the signature and header record (SPEC 1.0) and the
//! profile declaration (SPEC 0); [`chunks`] holds the chunk rules (SPEC
//! 1.1) every reader applies to the chunks it meets; [`limits`] the
//! admission checkpoints and the output limits checked right after the
//! header; [`payload`] the input a `DATA` head is read from (a slice or a
//! stream) and the checks the header makes of the payload; [`records`]
//! where each unit's records sit and the rules for reading them; [`nested`]
//! how a decoder parses the complete files it holds (frames and pages).
//! Decode routes, region and plane readers, frame decoders and the writers
//! that edit finished files read the format through this module only
//! (docs/FORMAT_GRAMMAR_PLAN.md section 7).

pub(crate) mod chunks;
pub(crate) mod header;
pub(crate) mod limits;
pub(crate) mod nested;
pub(crate) mod payload;
pub(crate) mod positional;
pub(crate) mod records;
