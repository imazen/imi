//! Partition-map encoding, compact restart-state length checks, V3 container
//! assembly, and seeded-vs-explicit file selection.

use alloc::vec;
use alloc::vec::Vec;
use how_far::Pulse;

use super::nested::EncodedDc;
use super::trellis_stage::AcCoding;
#[cfg(all(feature = "lab", test))]
use crate::ByteReader;
#[cfg(all(feature = "lab", test))]
use crate::lossy::model_candidates;
#[cfg(all(feature = "lab", feature = "std"))]
use crate::lossy::prof;
use crate::lossy::{
    Coded, H_AQ, H_BIAS, H_DC32, H_EXACT, H_EXPLICIT, H_KEEP, H_LAYOUTS, H_TRANSFORM, InputKind,
    LossyChannelLayout, LossyColor, LossyConfig, LossyEntropy, MATRIX_EXPLICIT, MATRIX_LAST_PRESET,
    MapKind, QParams, StripeCoefs, coding_defaults, compact, dc_coverage, envelope, layout,
    local_ans, model_refs, seed_bank, small, superblock,
};
use crate::rans::NL;
use crate::{Error, ImageInfo, Metadata, put_varint};

/// Encoded per-unit partition maps (superblock merges, joint 4x4+merge maps,
/// or scoped coding defaults).
pub(super) struct PartitionMaps {
    pub any_merge: bool,
    pub map_kind: MapKind,
    pub default_maps: Option<(Vec<u8>, Vec<Vec<u8>>)>,
    pub sb_maps: Option<Vec<Vec<u8>>>,
}

/// Encodes each unit's partition map (head record 21, 23, or coding defaults).
pub(super) fn build_partition_maps(
    stripes: &[StripeCoefs],
    cfg: &LossyConfig,
    layouts: [LossyChannelLayout; 3],
    sb_on: bool,
    has_exact: bool,
    skip_on: bool,
) -> Result<PartitionMaps, Error> {
    let any_split = stripes.iter().any(|sc| sc.splits.iter().any(|&v| v != 0));
    let any_merge = sb_on && stripes.iter().any(|sc| sc.modes.iter().any(|&v| v != 0));
    let map_kind = if any_split {
        MapKind::Joint
    } else if any_merge {
        MapKind::Merges
    } else {
        MapKind::None
    };
    let default_maps = if cfg.coding_defaults {
        if layouts != [LossyChannelLayout::default(); 3] {
            return Err(Error::InvalidInput("coding defaults with channel layouts"));
        }
        Some(coding_defaults::encode(stripes)?)
    } else {
        None
    };
    let sb_maps: Option<Vec<Vec<u8>>> = if let Some((_, maps)) = &default_maps {
        Some(maps.clone())
    } else {
        match map_kind {
            MapKind::None => None,
            MapKind::Merges => Some(
                stripes
                    .iter()
                    .map(|sc| superblock::map::encode(&sc.modes, sc.nby, sc.bw))
                    .collect(),
            ),
            MapKind::Joint => Some(
                stripes
                    .iter()
                    .map(|sc| {
                        // Bypassed parents (exact blocks: wire code 1; kept:
                        // code 0) carry no flags.
                        let bypass = |b: usize| {
                            let c = sc.aq.get(b).copied();
                            (has_exact && c == Some(1)) || (skip_on && c == Some(0))
                        };
                        let n = sc.nby * sc.bw;
                        let modes = if sc.modes.is_empty() {
                            vec![0u8; n]
                        } else {
                            sc.modes.clone()
                        };
                        // Flags only where the final coefficients have AC (a
                        // plane the trellis emptied decodes as the 8x8 block).
                        let ac = superblock::ac_bits(&sc.q, n);
                        let splits: Vec<u8> = (0..n)
                            .map(|b| sc.splits.get(b).copied().unwrap_or(0) & ac[b])
                            .collect();
                        superblock::map::encode_joint(
                            &modes,
                            &splits,
                            &ac,
                            (sc.nby, sc.bw),
                            &bypass,
                        )
                    })
                    .collect(),
            ),
        }
    };
    Ok(PartitionMaps {
        any_merge,
        map_kind,
        default_maps,
        sb_maps,
    })
}

/// Computes total framed bytes of AC records and whether compact rANS restart
/// states reduce total size.
pub(super) fn ac_records_len(
    cfg: &LossyConfig,
    stripes: &[StripeCoefs],
    pmaps: &PartitionMaps,
    adaptive_mask: u8,
    entropy: LossyEntropy,
    stripe_bytes: &[Vec<u8>],
) -> Result<(u128, bool), Error> {
    let record_len =
        |n: usize| n as u128 + (64 - (n as u64).leading_zeros()).max(1).div_ceil(7) as u128;
    let (mut raw, mut packed) = (0u128, 0u128);
    #[cfg(all(feature = "lab", test))]
    let catalog = cfg
        .catalog_lab
        .is_some_and(model_candidates::Settings::uses_envelope);
    #[cfg(not(all(feature = "lab", test)))]
    let catalog = false;
    let compactable = cfg.compact_streams
        && entropy == LossyEntropy::RunLevel
        && !cfg.local_ans_contexts
        && !cfg.model_references
        && !catalog;
    for (u, (bytes, sc)) in stripe_bytes.iter().zip(stripes).enumerate() {
        let map_len = if let Some(maps) = &pmaps.sb_maps {
            crate::varint_len(maps[u].len() as u64) + maps[u].len()
        } else if adaptive_mask == 0 || sc.modes.is_empty() {
            // No mode map is written (with superblocks on and nothing
            // merged, `modes` holds the selector's all-zero roles).
            0
        } else if sc.modes.iter().all(|m| *m == sc.modes[0]) {
            2
        } else {
            1 + (sc.modes.len() * 3).div_ceil(8)
        };
        raw += record_len(map_len + bytes.len());
        if compactable {
            packed += record_len(map_len + compact::packed_len(bytes, sc.bw.min(NL))?);
        }
    }
    Ok(if compactable && packed < raw {
        (packed, true)
    } else {
        (raw, false)
    })
}

/// Context shared across seeded and explicit file assembly candidates.
pub(super) struct AssemblyCtx<'a> {
    pub info: &'a ImageInfo,
    pub meta: &'a Metadata,
    pub cfg: &'a LossyConfig,
    pub sb: u32,
    pub gcols: usize,
    pub ng: usize,
    pub color: LossyColor,
    pub kind: InputKind,
    pub scale: [f32; 3],
    pub qp: QParams,
    pub layouts: [LossyChannelLayout; 3],
    pub adaptive_mask: u8,
    pub aq_on: bool,
    pub skip_on: bool,
    pub fits16: bool,
    pub dc_rows: usize,
    pub coverage_head: Option<&'a [u8]>,
    pub coverage: Option<&'a dc_coverage::Coverage>,
    pub exact: Option<&'a Coded>,
    pub stripes: &'a [StripeCoefs],
    pub pmaps: &'a PartitionMaps,
    pub alt_stripes: Option<&'a [StripeCoefs]>,
    pub alt_pmaps: Option<&'a PartitionMaps>,
    pub nested_features: crate::parse::header::Features,
}

impl<'a> AssemblyCtx<'a> {
    fn coding_maps(&self, uses_alt: bool) -> (&'a [StripeCoefs], &'a PartitionMaps) {
        if uses_alt {
            (
                self.alt_stripes.unwrap_or(self.stripes),
                self.alt_pmaps.unwrap_or(self.pmaps),
            )
        } else {
            (self.stripes, self.pmaps)
        }
    }
}

type Part<'p> = (&'p [u8], &'p [Vec<u8>]);

/// Assembles one complete V3 lossy file candidate from AC, DC, and optional
/// alpha parts under `seed_selection`.
#[allow(clippy::too_many_arguments)]
fn assemble_v3_file(
    ctx: &AssemblyCtx<'_>,
    entropy: LossyEntropy,
    uses_alt: bool,
    tb: &[u8],
    stripe_bytes: &[Vec<u8>],
    dc_head: &[u8],
    dc_stripes: &[Vec<u8>],
    alpha: Option<(&[u8], &[Vec<u8>])>,
    seed_selection: crate::seeds::Selection,
) -> Result<Vec<u8>, Error> {
    use crate::parse::records::{
        H_UNIT_ORDINALS, R_AC, R_ALPHA, R_DC, R_EXACT, main_framed_len, put_main_record, put_record,
    };
    let cfg = ctx.cfg;
    let (stripes, pmaps) = ctx.coding_maps(uses_alt);
    let compact_states = ac_records_len(
        cfg,
        stripes,
        pmaps,
        ctx.adaptive_mask,
        entropy,
        stripe_bytes,
    )?
    .1;
    let mut model_writer = if cfg.model_references {
        Some(model_refs::Writer::new(
            tb,
            entropy,
            if seed_selection.scope & crate::seeds::SCOPE_AC != 0 {
                seed_selection.bank
            } else {
                0
            },
        )?)
    } else {
        None
    };
    let tb = model_writer.as_ref().map_or(tb, |w| w.root);
    // Unit records (SPEC 2.7), in decode order: per lossy stripe s, the
    // DC stripe s / k when s is its first lossy stripe (k = DC stripe
    // height / sb), the alpha stripe s, lossy stripe s, then its exact
    // record (exact blocks only); with column groups, per stripe and
    // group in order. The unit table hint lists their spans.
    let k = ctx.dc_rows / ctx.sb as usize;
    let ordinals = cfg.unit_ordinals;
    let mut body = Vec::new();
    let mut spans = Vec::new();
    let ordinal_counter = core::cell::Cell::new(0u64);
    let mut rec = |id: u64, b: &[u8]| {
        let prefixed;
        let b = if cfg.model_references && id == R_DC {
            let mut bytes = vec![0, 0];
            bytes.extend_from_slice(b);
            prefixed = bytes;
            &prefixed[..]
        } else {
            b
        };
        let ord = ordinals.then_some(ordinal_counter.get());
        ordinal_counter.set(ordinal_counter.get() + 1);
        spans.push(main_framed_len(id, ord, b.len()));
        put_main_record(&mut body, id, ord, b);
    };
    for (u, st) in stripe_bytes.iter().enumerate() {
        let (si, gi) = (u / ctx.ng, u % ctx.ng);
        if let Some(c) = ctx.coverage {
            let span = c.span(si, gi);
            if span.start == si {
                rec(R_DC, &dc_stripes[span.unit]);
            }
        } else if si % k == 0 {
            rec(R_DC, &dc_stripes[si / k * ctx.ng + gi]);
        }
        if let Some((_, a)) = &alpha {
            rec(R_ALPHA, &a[u]);
        }
        let native;
        let st = if let Some(writer) = &mut model_writer {
            native = writer.ac(st, ordinal_counter.get())?;
            &native[..]
        } else {
            st
        };
        if ctx.adaptive_mask != 0 || compact_states || pmaps.sb_maps.is_some() {
            let mut record = Vec::new();
            if let Some(maps) = &pmaps.sb_maps {
                put_varint(&mut record, maps[u].len() as u64);
                record.extend_from_slice(&maps[u]);
            } else if ctx.adaptive_mask != 0 {
                layout::write_modes(&mut record, &stripes[u].modes);
            }
            if compact_states {
                record.extend_from_slice(&compact::pack(st, stripes[u].bw.min(NL))?);
            } else {
                record.extend_from_slice(st);
            }
            rec(R_AC, &record);
        } else {
            rec(R_AC, st);
        }
        if let Some((_, er)) = ctx.exact {
            rec(R_EXACT, &er[u]);
        }
    }
    // Head records in increasing id order (SPEC 2.8). The unit table
    // can be a hint (10) or a reservation (12), on either side of DC32 (11).
    let mut records = Vec::new();
    let hint_form = matches!(crate::v1::unit_table_form(), crate::v1::UnitTableForm::Hint);
    for (on, id) in [
        (ctx.exact.is_some(), H_EXACT),
        (ctx.aq_on, H_AQ),
        (ctx.skip_on, H_KEEP),
    ] {
        if on {
            put_record(&mut records, id, &[]);
        }
    }
    if hint_form {
        crate::v1::put_unit_table(&mut records, stripe_bytes.len(), &spans);
    }
    if !ctx.fits16 {
        put_record(&mut records, H_DC32, &[]);
    }
    if !hint_form {
        crate::v1::put_unit_table(&mut records, stripe_bytes.len(), &spans);
    }
    if ctx.qp.matrix == MATRIX_EXPLICIT {
        let mut rec = Vec::new();
        envelope::write_explicit(&mut rec, ctx.qp.explicit.as_ref().expect("explicit tables"));
        put_record(&mut records, H_EXPLICIT, &rec);
    }
    if let Some(b) = &ctx.qp.bias {
        let mut rec = Vec::new();
        envelope::write_bias(&mut rec, b);
        put_record(&mut records, H_BIAS, &rec);
    }
    if let Some(t) = ctx.qp.transform {
        let mut rec = Vec::new();
        envelope::write_transform(&mut rec, t);
        put_record(&mut records, H_TRANSFORM, &rec);
    }
    match if cfg.coding_defaults {
        MapKind::None
    } else {
        pmaps.map_kind
    } {
        MapKind::None => {}
        MapKind::Merges => put_record(&mut records, superblock::H_SUPERBLOCKS, &[]),
        MapKind::Joint => {
            #[cfg(feature = "lab")]
            let payload: &[u8] = if cfg.lab.small.as_ref().is_some_and(|s| s.int) {
                &[1]
            } else {
                &[]
            };
            #[cfg(not(feature = "lab"))]
            let payload: &[u8] = &[];
            put_record(&mut records, small::H_PARTITIONS, payload)
        }
    }
    if ctx.layouts != [LossyChannelLayout::default(); 3] {
        put_record(&mut records, H_LAYOUTS, &ctx.layouts.map(|l| l.byte()));
    }
    if ordinals {
        put_record(&mut records, H_UNIT_ORDINALS, &[]);
    }
    if cfg.local_ans_contexts {
        put_record(&mut records, local_ans::HEAD, &[]);
    }
    if let Some((head, _)) = &pmaps.default_maps {
        put_record(&mut records, coding_defaults::HEAD, head);
    }
    if cfg.model_references {
        put_record(&mut records, model_refs::HEAD, &[1, 0]);
    }
    if let Some(head) = ctx.coverage_head {
        put_record(&mut records, dc_coverage::HEAD, head);
    }
    #[cfg(all(feature = "lab", test))]
    let tb = if !cfg.model_references
        && cfg
            .catalog_lab
            .is_some_and(model_candidates::Settings::uses_envelope)
    {
        let mut bundle = ByteReader::new(tb);
        let n = bundle.var_usize()?;
        let tables = bundle.bytes(n)?;
        put_record(
            &mut records,
            model_candidates::HEAD,
            bundle.bytes(bundle.remaining())?,
        );
        tables
    } else {
        tb
    };
    let mut data = Vec::new();
    data.push(ctx.sb as u8);
    put_varint(&mut data, ctx.gcols as u64);
    envelope::write(
        &mut data,
        ctx.color,
        ctx.kind,
        &envelope::Params {
            scale: ctx.scale,
            qp: ctx.qp,
            entropy,
            compact: compact_states,
            seed: seed_selection,
            records: !records.is_empty(),
        },
    );
    put_varint(&mut data, tb.len() as u64);
    data.extend_from_slice(tb);
    if !records.is_empty() {
        put_varint(&mut data, records.len() as u64);
        data.extend_from_slice(&records);
    }
    // The nested heads, positional (SPEC 2.8): DC, alpha (with alpha),
    // exact blocks (with head record 5).
    put_varint(&mut data, dc_head.len() as u64);
    data.extend_from_slice(dc_head);
    if let Some((ah, _)) = &alpha {
        put_varint(&mut data, ah.len() as u64);
        data.extend_from_slice(ah);
    }
    if let Some((eh, _)) = ctx.exact {
        put_varint(&mut data, eh.len() as u64);
        data.extend_from_slice(eh);
    }
    data.extend_from_slice(&body);
    let mut out = Vec::with_capacity(data.len() + 64);
    let mut c = cfg.clone();
    c.color = ctx.color;
    // New seed IDs declare `seed_bank_ids`, exact blocks `exact_blocks`,
    // an i32 DC image `samples_32` (implied by 32-bit sample types: the
    // header writer drops it there), channel layouts `channel_layouts`,
    // with the nested heads' features, in the header's feature mask
    // (the comparisons below count its bytes).
    use crate::parse::header::Feature;
    let mut features = ctx.nested_features;
    if seed_selection.is_some() {
        features = features.with(Feature::SeedBankIds);
    }
    if ctx.exact.is_some() {
        features = features.with(Feature::ExactBlocks);
    }
    if !ctx.fits16 {
        features = features.with(Feature::Samples32);
    }
    if ctx.layouts != [LossyChannelLayout::default(); 3] {
        features = features.with(Feature::ChannelLayouts);
    }
    if (3..=MATRIX_LAST_PRESET).contains(&ctx.qp.matrix) {
        features = features.with(Feature::DequantPresets);
    }
    if ctx.qp.matrix == MATRIX_EXPLICIT {
        features = features.with(Feature::ExplicitTables);
    }
    if ctx.qp.bias.is_some() {
        features = features.with(Feature::ReconBias);
    }
    if ctx.qp.transform.is_some() {
        features = features.with(Feature::ColorTransform);
    }
    if pmaps.any_merge {
        features = features.with(Feature::Superblocks);
    }
    if cfg.model_references {
        features = features.with(Feature::ModelReferences);
    }
    if cfg.dc_coverage.is_some() {
        features = features.with(Feature::DcCoverage);
    }
    if cfg.coding_defaults {
        features = features.with(Feature::CodingDefaults);
    }
    if pmaps.map_kind == MapKind::Joint {
        features = features.with(Feature::SmallTransforms);
    }
    if ordinals {
        features = features.with(Feature::UnitOrdinals);
    }
    if cfg.local_ans_contexts {
        features = features.with(Feature::LocalAnsContexts);
    }
    crate::container::write_head_features(
        &mut out,
        ctx.info,
        ctx.meta,
        crate::Coding::Lossy,
        Some(ctx.color),
        c.decode_speed(),
        features,
    )?;
    crate::container::put_data(&mut out, &data);
    Ok(out)
}

fn varint_len(n: usize) -> u128 {
    (64 - (n as u64).leading_zeros()).max(1).div_ceil(7) as u128
}

fn nested_len((head, recs): Part<'_>) -> u128 {
    varint_len(head.len())
        + head.len() as u128
        + recs
            .iter()
            .map(|r| varint_len(r.len()) + r.len() as u128)
            .sum::<u128>()
}

/// The smaller of a nested head's seeded coding `main` and its explicit
/// one, and whether it is the seeded one.
fn choose_part<'p>(main: Part<'p>, explicit: &'p Option<Coded>) -> (Part<'p>, bool) {
    match explicit {
        Some(e) if nested_len(main) < nested_len((&e.0, &e.1)) => (main, true),
        Some(e) => ((&e.0[..], &e.1[..]), false),
        None => (main, false),
    }
}

/// Chooses the smaller of the seeded and explicit complete V3 files (ties go
/// to explicit, matching `finish_lossy_frame` without seeds byte-for-byte).
#[allow(clippy::too_many_arguments)]
pub(super) fn select_seeded_or_explicit(
    ctx: &AssemblyCtx<'_>,
    codings: &[AcCoding],
    pick: usize,
    pick_explicit: usize,
    dc: &EncodedDc,
    alpha: Option<&(ImageInfo, Vec<u8>, Vec<Vec<u8>>, Option<Coded>)>,
    pulse: &dyn Pulse,
) -> Result<Vec<u8>, Error> {
    let ac_len = |c: &AcCoding, (tb, recs): Part<'_>| -> Result<u128, Error> {
        let (stripes, pmaps) = ctx.coding_maps(c.uses_alt);
        Ok(varint_len(tb.len())
            + tb.len() as u128
            + ac_records_len(ctx.cfg, stripes, pmaps, ctx.adaptive_mask, c.entropy, recs)?.0)
    };
    let ac = &codings[pick];
    let (ac_part, ac_seeded) = match &ac.explicit {
        Some(e) if ac_len(ac, (&ac.tb, &ac.records))? < ac_len(ac, (&e.0, &e.1))? => {
            ((&ac.tb[..], &ac.records[..]), true)
        }
        _ => (ac.explicit(), false),
    };
    let (dc_part, dc_seeded) = choose_part((&dc.head, &dc.stripes), &dc.explicit);
    let al = alpha.map(|(_, h, r, e)| choose_part((h, r), e));
    let scope = ((ac_seeded as u8) * crate::seeds::SCOPE_AC)
        | ((dc_seeded as u8) * crate::seeds::SCOPE_DC)
        | ((al.is_some_and(|a| a.1) as u8) * crate::seeds::SCOPE_ALPHA);
    let explicit_file = |c: &AcCoding| {
        let (dh, dr) = dc
            .explicit
            .as_ref()
            .map_or((&dc.head[..], &dc.stripes[..]), |e| (&e.0[..], &e.1[..]));
        let al = alpha.map(|(_, h, r, e)| {
            e.as_ref()
                .map_or((&h[..], &r[..]), |e| (&e.0[..], &e.1[..]))
        });
        let (t, r) = c.explicit();
        assemble_v3_file(
            ctx,
            c.entropy,
            c.uses_alt,
            t,
            r,
            dh,
            dr,
            al,
            crate::seeds::Selection::NONE,
        )
    };
    if scope == 0 && pick == pick_explicit {
        #[cfg(all(feature = "lab", feature = "std"))]
        prof::ac_tables(
            codings[pick_explicit].entropy as u8,
            codings[pick_explicit].explicit().0.len() as u64,
        );
        let out = explicit_file(&codings[pick_explicit])?;
        pulse.check().map_err(|_| Error::Cancelled)?;
        return Ok(out);
    }
    let bank_id = seed_bank(ctx.cfg).map_or(0, |b| b.id);
    let seed_selection = if scope == 0 {
        crate::seeds::Selection::NONE
    } else {
        crate::seeds::Selection::new(bank_id, scope)
    };
    let file = assemble_v3_file(
        ctx,
        ac.entropy,
        ac.uses_alt,
        ac_part.0,
        ac_part.1,
        dc_part.0,
        dc_part.1,
        al.map(|a| a.0),
        seed_selection,
    )?;
    let without = explicit_file(&codings[pick_explicit])?;
    #[cfg(all(feature = "lab", feature = "std"))]
    {
        let (mode, tb) = if file.len() < without.len() {
            (ac.entropy, ac_part.0)
        } else {
            (
                codings[pick_explicit].entropy,
                codings[pick_explicit].explicit().0,
            )
        };
        prof::ac_tables(mode as u8, tb.len() as u64);
    }
    pulse.check().map_err(|_| Error::Cancelled)?;
    Ok(if file.len() < without.len() {
        file
    } else {
        without
    })
}
