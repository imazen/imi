//! Lossy decoder: bitstream parsing, ROI decoding, stripe coefficient decoding,
//! and color reconstruction.

pub(super) mod parse;
pub(super) mod recon;

#[cfg(any(feature = "v3", feature = "lab"))]
pub(crate) use parse::RegionGeometry;
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) use parse::parse_lossy;
pub(crate) use parse::{
    LossyParsed, MapKind, decoder_recon_bytes, decoder_table_bytes, parse_lossy_as,
    parse_lossy_checked, parse_lossy_head_async, parse_lossy_head_checked, payload_len_checked,
};
#[cfg(feature = "v3")]
pub(crate) use parse::{decode_lossy_region, region_unit_bytes};
#[cfg(feature = "lab")]
pub(crate) use recon::IDCT_FMA;
#[cfg(all(target_arch = "x86_64", feature = "avx512"))]
pub(crate) use recon::aq_dequant_in;
#[cfg(all(target_arch = "x86_64", any(test, feature = "lab")))]
pub(crate) use recon::force_gather;
#[cfg(all(target_arch = "x86_64", feature = "lab"))]
pub(crate) use recon::kernel_choice_str;
#[cfg(all(test, feature = "encode", feature = "lab", feature = "std"))]
pub(crate) use recon::scoped_recon_chunk_override;
#[cfg(all(feature = "std", any(all(test, feature = "encode"), feature = "lab")))]
pub(crate) use recon::set_shared_dc;
pub(crate) use recon::{
    Scratch, aq_dequant, decode_lossy_frame, decode_lossy_parsed, decode_lossy_parsed_checked,
    decode_lossy_parsed_observed, decoder_transfer_table_bytes, prof,
};
#[cfg(all(feature = "std", feature = "lab"))]
pub(crate) use recon::{decode_lossy_frame_observed, set_keep_scratch};
#[cfg(any(all(test, feature = "encode"), feature = "lab"))]
pub(crate) use recon::{set_dc_split, set_pairs, set_recon_chunk};
#[allow(unused_imports)]
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub(crate) use recon::{split_group, split_unpack_bc};
