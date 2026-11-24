//! V4 coefficient models use the existing checked tables and entropy kernels.

use alloc::vec::Vec;

use super::{AcModelView, Geom, Group, LossyEntropy, NCH, decode_coefficient_unit, local_ans};
use crate::Error;
use crate::decode_resources::DecodeSession;
use crate::parse::limits::Admission;

pub(crate) mod convert;
#[cfg(all(feature = "encode", feature = "std"))]
pub(crate) mod encode;
mod setup;
pub use setup::LossySetup;
mod models;
pub use models::LossyModels;
pub(crate) use models::{Definition, Model, ModelResolver};
pub(crate) use work::read_unit;
mod work;
pub(crate) use work::StoredLossyUnit;
pub use work::{LossyGroupInputs, LossyUnit};
mod stream;
pub use stream::{ExactPlan, LossyEvent, LossyGroupDecoder};
mod assemble;
pub use assemble::LossyWork;
#[cfg(feature = "std")]
pub(crate) use assemble::{Assembler, AssemblerState};
pub(crate) use assemble::{NumericDependency, plane_capacity, prepare_group};
#[cfg(feature = "std")]
pub(crate) use stream::LossyState;
#[cfg(feature = "lab")]
mod inspect;
#[cfg(all(feature = "lab", feature = "encode"))]
pub(crate) use inspect::quantized as inspect_quantized;
#[cfg(feature = "lab")]
pub(crate) use inspect::{dc_image as inspect_dc_image, models as inspect_models};
mod file;
pub use file::LossyFile;

#[cfg(all(test, feature = "lab", feature = "encode"))]
mod tests;

/// An immutable V4 AC definition, independent of unit geometry and settings.
/// This is a coefficient component, not a lossy file/pixel decoder. The owning
/// driver admits its framed input, declared bank and definition lifetime.
pub struct AcModel {
    model: local_ans::DecodedModel,
    coder: LossyEntropy,
    _charge: crate::decode_resources::V4Retained,
}

impl AcModel {
    /// Parse a complete table blob. The V4 setup must have declared a nonzero
    /// bank before this operation; unknown banks and retired coders refuse.
    /// Framed definition input is charged by the outer object driver.
    pub fn read<C: Fn() -> bool + Sync>(
        coder: LossyEntropy,
        bank_id: u64,
        definition: &[u8],
        session: &DecodeSession<C>,
    ) -> Result<Self, Error> {
        let scope = session.v4_model_scope();
        if !matches!(coder, LossyEntropy::RunLevel | LossyEntropy::SplitHuf) {
            return Err(Error::Unsupported("V4 AC entropy coder"));
        }
        let bank = if bank_id == 0 {
            None
        } else {
            Some(crate::seeds::bank(bank_id).ok_or(Error::Unsupported("V4 AC model bank"))?)
        };
        session.adapter_work(definition.len() as u128 + 1)?;
        session.v4_models().ac_models(1)?;
        session
            .v4_models()
            .hint(core::mem::size_of::<Self>() + 2 * core::mem::size_of::<usize>())?;
        let model = local_ans::DecodedModel::read(definition, coder, bank)?;
        Ok(Self {
            model,
            coder,
            _charge: scope.commit()?,
        })
    }

    pub const fn coder(&self) -> LossyEntropy {
        self.coder
    }

    /// Decode only AC spectra; DC slots are zero until the owning driver fills
    /// their coverage. Output is parent-row-major, then parent/three planes/64
    /// raster coefficients. `row_stride` counts i32 coefficients, not bytes.
    /// The driver separately validates settings and unused sampled coefficients.
    /// Scratch is bounded per group and released after this call. Caller output
    /// and framed entropy input retain their own admission/queue ownership.
    pub fn decode_into<C: Fn() -> bool + Sync>(
        &self,
        width: u32,
        height: u32,
        compact: bool,
        entropy: &[u8],
        output: &mut [i32],
        row_stride: usize,
        session: &DecodeSession<C>,
    ) -> Result<(), Error> {
        if width == 0 || height == 0 {
            return Err(Error::Corrupt("V4 AC group geometry"));
        }
        if compact && self.coder != LossyEntropy::RunLevel {
            return Err(Error::Corrupt("compact restart on V4 Huffman unit"));
        }
        let cols = (width as usize).div_ceil(8);
        let rows = (height as usize).div_ceil(8);
        let row = cols.checked_mul(NCH * 64).ok_or(Error::TooLarge)?;
        let required = (rows - 1)
            .checked_mul(row_stride)
            .and_then(|n| n.checked_add(row))
            .ok_or(Error::TooLarge)?;
        if row_stride < row || output.len() < required {
            return Err(Error::InvalidInput("V4 coefficient stride/extent"));
        }
        let count = rows.checked_mul(row).ok_or(Error::TooLarge)?;
        let blocks = rows.checked_mul(cols).ok_or(Error::TooLarge)?;
        let qlen = count.checked_add(64).ok_or(Error::TooLarge)?;
        let elen = blocks
            .checked_mul(NCH)
            .and_then(|n| n.checked_add(1))
            .ok_or(Error::TooLarge)?;
        // The common plan includes all Huffman side vectors (luma/chroma,
        // table IDs and flattened widths), not only the caller's width array.
        let scratch = crate::decoder_memory::lossy_stripe_bytes(u128::from(width), rows as u128)?
            .checked_add(16 * 1024)
            .ok_or(Error::TooLarge)?;
        let _working = session.v4_working(scratch)?;
        session.adapter_work(count as u128 + entropy.len() as u128 + 1)?;
        let mut q = Vec::new();
        q.try_reserve_exact(qlen).map_err(|_| Error::TooLarge)?;
        let mut eobs = Vec::new();
        eobs.try_reserve_exact(elen).map_err(|_| Error::TooLarge)?;
        let g = Geom::new(
            width,
            height,
            u32::try_from(rows).map_err(|_| Error::TooLarge)?,
        );
        let (q, _) = self.decode_quantized(&g, compact, entropy, q, eobs)?;
        for y in 0..rows {
            session.adapter_work(1)?;
            output[y * row_stride..y * row_stride + row]
                .copy_from_slice(&q[y * row..(y + 1) * row]);
        }
        Ok(())
    }

    // The owner admits/reserves the coefficient and side buffers before this
    // shared call. Preserve original EOB/group counts for merged reconstruction.
    fn decode_quantized(
        &self,
        g: &Geom,
        compact: bool,
        entropy: &[u8],
        q: Vec<i32>,
        eobs: Vec<u8>,
    ) -> Result<(Vec<i32>, Vec<u8>), Error> {
        let unit = local_ans::Unit {
            shared: Some(&self.model),
            inline: None,
            body: entropy,
            compact,
        };
        let root = AcModelView {
            tables: &self.model.tables,
            rans: self.model.rans.as_ref(),
            huf: self.model.huf.as_ref(),
        };
        let (q, eobs, consumed, available) =
            decode_coefficient_unit(g, 0, &unit, root, Group::ALL, true, q, eobs)?;
        if consumed > available {
            return Err(Error::Corrupt("V4 coefficient lane overrun"));
        }
        Ok((q, eobs))
    }
}

#[cfg(all(test, feature = "lab", feature = "encode"))]
mod ac_tests {
    use super::*;
    use crate::decode_resources::DecodePolicy;
    use crate::{Channels, ImageInfo, ImiEncoderConfig, LossyColor, Metadata, SampleType};
    use alloc::vec;

    fn source(
        width: u32,
        coder: LossyEntropy,
        bank: u64,
        compact: bool,
    ) -> (ImageInfo, Vec<i32>, Vec<u8>) {
        let info = ImageInfo::new(width, 29, Channels::Rgb, SampleType::U8);
        let blocks = (info.width as usize).div_ceil(8) * (info.height as usize).div_ceil(8);
        let mut q = vec![0; blocks * NCH * 64];
        for b in 0..blocks {
            for c in 0..NCH {
                for k in 0..64 {
                    q[(b * NCH + c) * 64 + k] = if k == 0 {
                        ((b * 13 + c * 7) % 41) as i32 - 20
                    } else if k == 63 && b.is_multiple_of(7) {
                        (1 << 20) - 1
                    } else if (b * 31 + c * 7 + k).is_multiple_of(11) {
                        ((k * 19 + b * c) % 27) as i32 - 13
                    } else {
                        0
                    };
                }
            }
        }
        let cfg = ImiEncoderConfig::lossy(50.0)
            .with_column_groups(match width {
                41 => Some(16),
                137 => Some(128),
                _ => None,
            })
            .tune_lossy(|t| {
                t.color = LossyColor::YCoCg;
                t.entropy = coder;
                t.aq_gamma = 0.0;
                t.stripe_block_rows = 2;
                t.compact_streams = compact;
                t.exact_blocks = Some(0.0);
                t.small_transforms = false;
                t.adaptive_partitions = false;
                t.lab.seed_bank = bank;
                t.lab.seed_exact = bank != 0;
            });
        // This oracle deliberately extracts retained native tables, then builds
        // independent V4 definitions from them.
        let file = crate::lab::encode_lossy_quantized_lowered(
            &info,
            &Metadata::default(),
            &cfg.lossy_config(),
            &q,
        )
        .unwrap();
        (info, q, file)
    }

    fn explicit(p: &super::super::LossyParsed<'_>) -> Vec<u8> {
        definition(p, None)
    }

    fn definition(
        p: &super::super::LossyParsed<'_>,
        bank: Option<&'static crate::seeds::Bank>,
    ) -> Vec<u8> {
        let mut bits = crate::bits::BitWriter::new();
        if let Some((huf, _, _)) = &p.huf {
            let mut tables = huf.clone();
            tables.seeds = bank.map(|bank| (bank, vec![None; tables.lens.len()]));
            tables.write(&mut bits);
        } else {
            let mut tables = p.tables.clone();
            tables.seeds = bank.map(|bank| (bank, vec![None; tables.freq.len()]));
            tables.write(&mut bits);
        }
        bits.finish()
    }

    #[test]
    fn v4_ac_definitions_decode_supplied_spectra_with_both_coders_banks_and_restarts() {
        for width in [41, 137, 265] {
            for coder in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                for bank in [0, 1] {
                    for compact in [false, true] {
                        let (info, expected, file) = source(width, coder, bank, compact);
                        let parsed = super::super::parse_lossy(&file).unwrap();
                        let f = crate::v1::parse_file(
                            &file,
                            &crate::parse::limits::Unrestricted,
                            crate::checksum::Verify::Now(1),
                        )
                        .unwrap();
                        let mut head = crate::ByteReader::new(f.data);
                        head.u8().unwrap();
                        head.varint().unwrap();
                        let params = super::super::envelope::read(
                            &mut head,
                            LossyColor::YCoCg,
                            super::super::InputKind::U8,
                            f.declared,
                            true,
                        )
                        .unwrap();
                        let length = head.var_usize().unwrap();
                        let original = head.bytes(length).unwrap();
                        let actual_bank = params
                            .seed
                            .scoped(crate::seeds::SCOPE_AC)
                            .map_or(0, |b| b.id);
                        let plain = explicit(&parsed);
                        let bank_explicit =
                            definition(&parsed, Some(crate::seeds::bank(1).unwrap()));
                        for (definition, bank_id) in [
                            (original, actual_bank),
                            (plain.as_slice(), 0),
                            (bank_explicit.as_slice(), 1),
                        ] {
                            let session = DecodeSession::new(DecodePolicy::default());
                            let model =
                                AcModel::read(coder, bank_id, definition, &session).unwrap();
                            assert_eq!(model.coder(), coder);
                            let image_bw = (info.width as usize).div_ceil(8);
                            for (u, encoded) in parsed.stripes.iter().enumerate() {
                                let gi = u % parsed.ngroups();
                                let s = u / parsed.ngroups();
                                let (x, geometry) = parsed.group(gi);
                                let (a, b) = geometry.rows(s);
                                let (_, bytes) = parsed.record(&geometry, s, encoded).unwrap();
                                let unit = local_ans::unit(&parsed, bytes).unwrap();
                                let height = (b * 8).min(info.height as usize) - a * 8;
                                let row = geometry.bw * NCH * 64;
                                let stride = row + 17;
                                let mut output = vec![i32::MIN; stride * (b - a)];
                                let worker = DecodeSession::new(DecodePolicy::default());
                                model
                                    .decode_into(
                                        geometry.w as u32,
                                        height as u32,
                                        unit.compact,
                                        unit.body,
                                        &mut output,
                                        stride,
                                        &worker,
                                    )
                                    .unwrap();
                                for r in 0..b - a {
                                    for bx in 0..geometry.bw {
                                        for c in 0..NCH {
                                            for k in 0..64 {
                                                let src =
                                                    ((((a + r) * image_bw + x / 8 + bx) * NCH + c)
                                                        * 64)
                                                        + k;
                                                let dst = r * stride + (bx * NCH + c) * 64 + k;
                                                assert_eq!(
                                                    output[dst],
                                                    if k == 0 { 0 } else { expected[src] },
                                                    "{coder:?} bank={bank_id} compact={} unit={u} ({r},{bx},{c},{k})",
                                                    unit.compact
                                                );
                                            }
                                        }
                                    }
                                    assert!(
                                        output[r * stride + row..(r + 1) * stride]
                                            .iter()
                                            .all(|&v| v == i32::MIN)
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn v4_ac_limits_bad_definitions_and_strides_refuse_without_touching_output() {
        let (_, _, file) = source(41, LossyEntropy::RunLevel, 0, false);
        let parsed = super::super::parse_lossy(&file).unwrap();
        let definition = explicit(&parsed);
        let session = DecodeSession::new(DecodePolicy::default());
        assert!(AcModel::read(LossyEntropy::RunLevel, u64::MAX, &definition, &session).is_err());
        for count in [0, 1, definition.len() / 2] {
            let session = DecodeSession::new(DecodePolicy::default());
            assert!(
                AcModel::read(LossyEntropy::RunLevel, 0, &definition[..count], &session).is_err()
            );
        }
        let session = DecodeSession::new(DecodePolicy::default().with_max_working_bytes(0));
        assert!(AcModel::read(LossyEntropy::RunLevel, 0, &definition, &session).is_err());
        let session = DecodeSession::new(DecodePolicy::default());
        let model = AcModel::read(LossyEntropy::RunLevel, 0, &definition, &session).unwrap();
        let mut output = [i32::MIN; NCH * 64];
        assert!(
            model
                .decode_into(8, 8, false, &[], &mut output, NCH * 64 - 1, &session)
                .is_err()
        );
        assert!(output.iter().all(|&v| v == i32::MIN));
        let session = DecodeSession::with_cancel(DecodePolicy::default(), || true);
        assert!(
            model
                .decode_into(8, 8, false, &[], &mut output, NCH * 64, &session)
                .is_err()
        );
    }
}
