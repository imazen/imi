//! Typed immutable definitions. Sharing never carries entropy/predictor state.
use super::*;
use alloc::{boxed::Box, sync::Arc};

pub(super) const HEAD: u64 = crate::registry::HEAD_MODEL_REFERENCES;

pub(super) enum Definition {
    Ac(Arc<local_ans::DecodedModel>),
    Dc(Arc<crate::v1::Parsed<'static>>),
}

pub(super) struct Models {
    catalog: Vec<(u64, Definition)>,
    reusable: Vec<(u64, Definition)>,
    sources: Vec<Vec<u8>>,
    by_prefix: alloc::collections::BTreeMap<Vec<u8>, u64>,
    mode: LossyEntropy,
    declared: crate::parse::header::Features,
}

pub(super) struct WireDefinition<'a> {
    pub role: u64,
    pub coder: u64,
    pub bank: u64,
    pub scope: u64,
    pub bytes: &'a [u8],
}

impl<'a> WireDefinition<'a> {
    pub fn read(r: &mut ByteReader<'a>) -> Result<Self, Error> {
        let role = r.varint()?;
        let coder = r.varint()?;
        let bank = r.varint()?;
        let scope = r.varint()?;
        if role > 1 {
            return Err(Error::Unsupported("model definition role"));
        }
        if (role == 0 && coder != 0) || (role == 1 && coder != 0 && coder != 3) {
            return Err(Error::Unsupported("model definition coder"));
        }
        if (bank == 0 && scope != 0) || (bank != 0 && scope != if role == 0 { 2 } else { 1 }) {
            return Err(Error::Corrupt("model definition scope"));
        }
        if bank != 0 && crate::seeds::bank(bank).is_none() {
            return Err(Error::Unsupported("model definition bank"));
        }
        let n = r.var_usize()?;
        let bytes = r.bytes(n)?;
        if bytes.is_empty() {
            return Err(Error::Corrupt("empty model definition"));
        }
        Ok(Self {
            role,
            coder,
            bank,
            scope,
            bytes,
        })
    }

    #[cfg(feature = "encode")]
    pub fn write(&self, out: &mut Vec<u8>) {
        for v in [
            self.role,
            self.coder,
            self.bank,
            self.scope,
            self.bytes.len() as u64,
        ] {
            put_varint(out, v);
        }
        out.extend_from_slice(self.bytes);
    }
}

pub(super) enum Selector<'a> {
    Root,
    Catalog(u64),
    Inline(WireDefinition<'a>),
    Define(WireDefinition<'a>),
    Earlier(u64),
}

pub(super) fn prefix<'a>(
    r: &mut ByteReader<'a>,
    role: u64,
    mode: LossyEntropy,
) -> Result<(Selector<'a>, bool), Error> {
    let selection = match r.varint()? {
        0 => Selector::Root,
        1 => Selector::Catalog(r.varint()?),
        2 => Selector::Inline(WireDefinition::read(r)?),
        3 => Selector::Define(WireDefinition::read(r)?),
        4 => Selector::Earlier(r.varint()?),
        _ => return Err(Error::Unsupported("model selector")),
    };
    if let Selector::Inline(d) | Selector::Define(d) = &selection
        && (d.role != role || (role == 1 && d.coder != mode as u64))
    {
        return Err(Error::Corrupt("model definition use"));
    }
    let compact = if role == 0 || mode == LossyEntropy::RunLevel {
        match r.varint()? {
            0 => false,
            1 if role == 1 => true,
            _ => return Err(Error::Corrupt("model restart form")),
        }
    } else {
        false
    };
    Ok((selection, compact))
}

impl Models {
    pub fn read(
        bytes: &[u8],
        root: &crate::v1::Parsed<'_>,
        mode: LossyEntropy,
        declared: crate::parse::header::Features,
        units: usize,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Self, Error> {
        let mut r = ByteReader::new(bytes);
        if r.varint()? != 1 {
            return Err(Error::Unsupported("model references version"));
        }
        let n = r.var_usize()?;
        if n > r.remaining() / 7 {
            return Err(Error::Corrupt("model definition count"));
        }
        admission.hint(
            n.checked_mul(core::mem::size_of::<(u64, Definition)>())
                .ok_or(Error::TooLarge)?,
        )?;
        admission
            .wire_extra_bytes((n as u128) * core::mem::size_of::<(u64, Definition)>() as u128)?;
        admission.local_ans(units as u128)?;
        let mut out = Self {
            catalog: Vec::new(),
            reusable: Vec::new(),
            sources: Vec::new(),
            by_prefix: alloc::collections::BTreeMap::new(),
            mode,
            declared,
        };
        out.catalog
            .try_reserve_exact(n)
            .map_err(|_| Error::TooLarge)?;
        let mut last = 0;
        for _ in 0..n {
            admission.check()?;
            let id = r.varint()?;
            if id <= last {
                return Err(Error::Corrupt("model definition ids"));
            }
            last = id;
            let wire = WireDefinition::read(&mut r)?;
            let definition = out.expand(&wire, root, admission)?;
            out.catalog.push((id, definition));
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("model catalog tail"));
        }
        Ok(out)
    }

    fn expand(
        &self,
        d: &WireDefinition<'_>,
        root: &crate::v1::Parsed<'_>,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Definition, Error> {
        admission.wire_extra_bytes(if d.role == 1 {
            decoder_table_bytes()
        } else {
            crate::decoder_memory::lossless_tables(4, 32, true)
        })?;
        let bank = if d.scope == 0 {
            None
        } else {
            crate::seeds::bank(d.bank)
        };
        if d.role == 1 {
            if d.coder != (self.mode as u64) {
                return Err(Error::Corrupt("AC definition coder mismatch"));
            }
            admission.ac_models(1)?;
            Ok(Definition::Ac(Arc::new(local_ans::DecodedModel::read(
                d.bytes, self.mode, bank,
            )?)))
        } else {
            let mut r = ByteReader::new(d.bytes);
            let mut parsed = crate::v1::parse_head_nested(
                root.info.clone(),
                &mut r,
                admission,
                bank,
                self.declared,
            )?;
            if r.remaining() != 0 {
                return Err(Error::Corrupt("DC definition head tail"));
            }
            if parsed.quick
                || parsed.hs != root.hs
                || parsed.gcols != root.gcols
                || parsed.lanes != root.lanes
            {
                return Err(Error::Corrupt("DC definition geometry"));
            }
            parsed.unit_rectangles = root.unit_rectangles.clone();
            Ok(Definition::Dc(Arc::new(parsed.without_records())))
        }
    }

    #[cfg(feature = "v3")]
    pub fn has(&self, ordinal: u64) -> bool {
        self.reusable
            .binary_search_by_key(&ordinal, |&(id, _)| id)
            .is_ok()
    }

    #[cfg(feature = "v3")]
    pub fn storage_bytes(&self) -> u128 {
        self.catalog
            .iter()
            .chain(&self.reusable)
            .map(|(_, d)| match d {
                Definition::Ac(_) => decoder_table_bytes(),
                Definition::Dc(_) => crate::decoder_memory::lossless_tables(4, 32, true),
            })
            .sum::<u128>()
            + self
                .sources
                .iter()
                .map(|s| (s.len() * 2 + 1024) as u128)
                .sum::<u128>()
    }

    #[cfg(feature = "v3")]
    pub fn dependency(&self, ordinal: u64, role: u64, bytes: &[u8]) -> Result<Option<u64>, Error> {
        let mut r = ByteReader::new(bytes);
        let (selector, _) = prefix(&mut r, role, self.mode)?;
        if let Selector::Earlier(at) = selector {
            if at >= ordinal {
                return Err(Error::Corrupt("forward model reference"));
            }
            return Ok((!self.has(at)).then_some(at));
        }
        Ok(None)
    }

    fn get(&self, selection: &Selector<'_>) -> Result<Option<&Definition>, Error> {
        let (entries, id) = match selection {
            Selector::Root => return Ok(None),
            Selector::Catalog(id) => (&self.catalog, *id),
            Selector::Earlier(id) => (&self.reusable, *id),
            _ => return Err(Error::Corrupt("inline selection lookup")),
        };
        let at = entries
            .binary_search_by_key(&id, |&(id, _)| id)
            .map_err(|_| Error::Corrupt("missing model definition"))?;
        Ok(Some(&entries[at].1))
    }

    /// Called in positional order, or for an explicitly fetched first-use prefix.
    pub fn admit(
        &mut self,
        ordinal: u64,
        role: u64,
        bytes: &[u8],
        root: &crate::v1::Parsed<'_>,
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<(), Error> {
        let mut r = ByteReader::new(bytes);
        let (selector, _) = prefix(&mut r, role, self.mode)?;
        match selector {
            Selector::Earlier(at) if at >= ordinal => {
                Err(Error::Corrupt("forward model reference"))
            }
            Selector::Define(d) => {
                if let Ok(i) = self.reusable.binary_search_by_key(&ordinal, |&(id, _)| id) {
                    return if self.sources[i] == bytes[..r.pos] {
                        Ok(())
                    } else {
                        Err(Error::Corrupt("model definition changed"))
                    };
                }
                admission.hint(
                    r.pos
                        .checked_mul(2)
                        .and_then(|n| n.checked_add(1024))
                        .ok_or(Error::TooLarge)?,
                )?;
                admission.wire_extra_bytes((r.pos as u128) * 2 + 1024)?;
                let definition = self.expand(&d, root, admission)?;
                self.reusable.try_reserve(1).map_err(|_| Error::TooLarge)?;
                let at = self.reusable.partition_point(|&(id, _)| id < ordinal);
                let mut source = Vec::new();
                source
                    .try_reserve_exact(r.pos)
                    .map_err(|_| Error::TooLarge)?;
                source.extend_from_slice(&bytes[..r.pos]);
                let mut key = Vec::new();
                key.try_reserve_exact(r.pos).map_err(|_| Error::TooLarge)?;
                key.extend_from_slice(&source);
                self.sources.try_reserve(1).map_err(|_| Error::TooLarge)?;
                self.sources.insert(at, source);
                self.reusable.insert(at, (ordinal, definition));
                self.by_prefix.insert(key, ordinal);
                Ok(())
            }
            Selector::Inline(d) => {
                self.expand(&d, root, admission)?;
                Ok(())
            }
            _ => {
                if let Some(d) = self.get(&selector)?
                    && !matches!((role, d), (0, Definition::Dc(_)) | (1, Definition::Ac(_)))
                {
                    return Err(Error::Corrupt("model reference role"));
                }
                Ok(())
            }
        }
    }

    pub fn ac<'a>(&'a self, bytes: &'a [u8]) -> Result<local_ans::Unit<'a>, Error> {
        let mut r = ByteReader::new(bytes);
        let (selector, compact) = prefix(&mut r, 1, self.mode)?;
        let mut shared = None;
        let mut inline = None;
        match &selector {
            Selector::Define(_) => {
                let ordinal = *self
                    .by_prefix
                    .get(&bytes[..r.pos])
                    .ok_or(Error::Corrupt("unadmitted first-use model"))?;
                match self.get(&Selector::Earlier(ordinal))? {
                    Some(Definition::Ac(model)) => shared = Some(model.as_ref()),
                    _ => return Err(Error::Corrupt("AC first-use role")),
                }
            }
            Selector::Inline(d) => {
                inline = Some(Box::new(local_ans::DecodedModel::read(
                    d.bytes,
                    self.mode,
                    if d.bank == 0 {
                        None
                    } else {
                        crate::seeds::bank(d.bank)
                    },
                )?));
            }
            _ => {
                if let Some(d) = self.get(&selector)? {
                    match d {
                        Definition::Ac(model) => shared = Some(model.as_ref()),
                        _ => return Err(Error::Corrupt("AC reference to DC model")),
                    }
                }
            }
        }
        Ok(local_ans::Unit {
            shared,
            inline,
            compact,
            body: &bytes[r.pos..],
        })
    }

    pub fn dc<'a>(
        &self,
        bytes: &'a [u8],
        root: &crate::v1::Parsed<'_>,
    ) -> Result<(Option<Arc<crate::v1::Parsed<'static>>>, &'a [u8]), Error> {
        let mut r = ByteReader::new(bytes);
        let (selector, _) = prefix(&mut r, 0, self.mode)?;
        let model = match &selector {
            Selector::Define(_) => {
                let ordinal = *self
                    .by_prefix
                    .get(&bytes[..r.pos])
                    .ok_or(Error::Corrupt("unadmitted first-use model"))?;
                match self.get(&Selector::Earlier(ordinal))? {
                    Some(Definition::Dc(p)) => Some(Arc::clone(p)),
                    _ => return Err(Error::Corrupt("DC first-use role")),
                }
            }
            Selector::Inline(d) => {
                match self.expand(d, root, &crate::parse::limits::Unrestricted)? {
                    Definition::Dc(p) => Some(p),
                    _ => unreachable!(),
                }
            }
            _ => match self.get(&selector)? {
                None => None,
                Some(Definition::Dc(p)) => Some(Arc::clone(p)),
                _ => return Err(Error::Corrupt("DC reference to AC model")),
            },
        };
        Ok((model, &bytes[r.pos..]))
    }
}

/// Translate the candidate learner's private bundle at final native assembly.
/// The learner's policies remain encoder choices; they are not decoder syntax.
#[cfg(feature = "encode")]
pub(super) struct Writer<'a> {
    pub root: &'a [u8],
    definitions: Vec<&'a [u8]>,
    introduced: Vec<Option<u64>>,
    mode: LossyEntropy,
    bank: u64,
}

/// Current-unit ANS learning with a root fallback. No future source rows or
/// retained model cache; both complete representations are priced on wire.
#[cfg(feature = "encode")]
pub(super) fn encode_causal_ans(
    lanes: &[Vec<Sym>; NL],
    active: usize,
    cfg: &LossyConfig,
    effort: crate::effort::Effort,
    global: &[u8],
) -> Result<Vec<u8>, Error> {
    let local = local_ans::encode(lanes, active, cfg, effort, global)?;
    let (tables, compact, body) = local_ans::split(&local)?;
    let mut candidate = Vec::new();
    if let Some(bytes) = tables {
        put_varint(&mut candidate, 2);
        WireDefinition {
            role: 1,
            coder: 0,
            bank: 0,
            scope: 0,
            bytes,
        }
        .write(&mut candidate);
    } else {
        put_varint(&mut candidate, 0);
    }
    put_varint(&mut candidate, u64::from(compact));
    candidate.extend_from_slice(body);
    let mut fallback = vec![0, 0];
    fallback.extend_from_slice(global);
    Ok(if candidate.len() < fallback.len() {
        candidate
    } else {
        fallback
    })
}

#[cfg(feature = "encode")]
impl<'a> Writer<'a> {
    pub fn new(bundle: &'a [u8], mode: LossyEntropy, bank: u64) -> Result<Self, Error> {
        let mut r = ByteReader::new(bundle);
        let n = r.var_usize()?;
        let root = r.bytes(n)?;
        if r.varint()? != 1 {
            return Err(Error::Corrupt("encoder model bundle"));
        }
        let n = r.var_usize()?;
        let mut definitions = Vec::new();
        definitions
            .try_reserve_exact(n)
            .map_err(|_| Error::TooLarge)?;
        for _ in 0..n {
            if r.varint()? != 1 {
                return Err(Error::Corrupt("encoder model role"));
            }
            let n = r.var_usize()?;
            definitions.push(r.bytes(n)?);
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("encoder model bundle tail"));
        }
        Ok(Self {
            root,
            definitions,
            introduced: alloc::vec![None;n],
            mode,
            bank,
        })
    }

    pub fn ac(&mut self, bytes: &[u8], ordinal: u64) -> Result<Vec<u8>, Error> {
        let mut r = ByteReader::new(bytes);
        let control = r.varint()?;
        let value = usize::try_from(control >> 2).map_err(|_| Error::TooLarge)?;
        let mut out = Vec::new();
        if control & 2 != 0 {
            put_varint(&mut out, 2);
            self.definition(r.bytes(value)?).write(&mut out);
        } else if value == 0 {
            put_varint(&mut out, 0);
        } else {
            let at = value - 1;
            let def = *self
                .definitions
                .get(at)
                .ok_or(Error::Corrupt("encoder model index"))?;
            if let Some(at) = self.introduced[at] {
                put_varint(&mut out, 4);
                put_varint(&mut out, at);
            } else {
                put_varint(&mut out, 3);
                self.definition(def).write(&mut out);
                self.introduced[at] = Some(ordinal);
            }
        }
        if self.mode == LossyEntropy::RunLevel {
            put_varint(&mut out, control & 1);
        }
        out.extend_from_slice(&bytes[r.pos..]);
        Ok(out)
    }

    fn definition<'b>(&self, bytes: &'b [u8]) -> WireDefinition<'b> {
        WireDefinition {
            role: 1,
            coder: self.mode as u64,
            bank: self.bank,
            scope: if self.bank == 0 { 0 } else { 1 },
            bytes,
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn source() -> (ImageInfo, Vec<u8>, Vec<u8>) {
        let info = ImageInfo::new(129, 129, Channels::Rgb, SampleType::U8);
        let pixels = super::super::tests::test_image(129, 129, 19);
        let cfg = LossyConfig::new(80.0).with_stripe_block_rows(2);
        let file = encode_lossy(&pixels, 129 * 3, &info, &Metadata::default(), &cfg).unwrap();
        (info, pixels, file)
    }

    #[test]
    fn typed_catalogs_and_backward_definitions_are_admitted_before_expansion() {
        let (_, _, file) = source();
        let p = parse_lossy(&file).unwrap();
        let root = p.dc.as_ref().unwrap();
        let mut tables = p.tables.clone();
        tables.seeds = None;
        let mut bits = BitWriter::new();
        tables.write(&mut bits);
        let bytes = bits.finish();
        let d = WireDefinition {
            role: 1,
            coder: 0,
            bank: 0,
            scope: 0,
            bytes: &bytes,
        };
        let mut head = vec![1, 1];
        put_varint(&mut head, u64::MAX);
        d.write(&mut head);
        let mut models = Models::read(
            &head,
            root,
            LossyEntropy::RunLevel,
            crate::parse::header::Features::ALL,
            9,
            &crate::parse::limits::Unrestricted,
        )
        .unwrap();
        let mut catalog = vec![1];
        put_varint(&mut catalog, u64::MAX);
        catalog.push(0);
        assert!(models.ac(&catalog).unwrap().model().is_some());
        let mut first = vec![3];
        d.write(&mut first);
        first.push(0);
        models
            .admit(7, 1, &first, root, &crate::parse::limits::Unrestricted)
            .unwrap();
        assert!(models.ac(&first).unwrap().model().is_some());
        models
            .admit(8, 1, &[4, 7, 0], root, &crate::parse::limits::Unrestricted)
            .unwrap();
        assert!(models.ac(&[4, 7, 0]).unwrap().model().is_some());
        for (ordinal, role, prefix) in [
            (7, 1, &[4, 7, 0][..]),
            (9, 1, &[4, 8, 0][..]),
            (9, 0, &[4, 7, 0][..]),
            (9, 1, &[4, 6, 0][..]),
        ] {
            assert!(
                models
                    .admit(
                        ordinal,
                        role,
                        prefix,
                        root,
                        &crate::parse::limits::Unrestricted
                    )
                    .is_err()
            );
        }
        struct Deny;
        impl crate::parse::limits::Admission for Deny {
            fn ac_models(&self, _: u128) -> Result<(), Error> {
                Err(Error::TooLarge)
            }
        }
        assert!(matches!(
            Models::read(
                &head,
                root,
                LossyEntropy::RunLevel,
                crate::parse::header::Features::ALL,
                9,
                &Deny
            ),
            Err(Error::TooLarge)
        ));
    }

    #[test]
    fn dc_definitions_use_the_real_numeric_head_and_exact_output_mapping() {
        let (_, _, file) = source();
        let p = parse_lossy(&file).unwrap();
        let root = p.dc.as_ref().unwrap();
        let (info, pixels, _) = dc_image_lab(&file).unwrap();
        let cfg = crate::EncodeConfig {
            effort: 0.0,
            stripe_rows: Some(root.hs as u32),
            lanes: Some(root.lanes as u32),
            column_groups: Some(root.gcols as u32),
            float_shift: false,
            ..Default::default()
        };
        let (head, units, _) =
            crate::v1::encode_parts(&pixels, info.row_bytes(), &info, &cfg).unwrap();
        let d = WireDefinition {
            role: 0,
            coder: 0,
            bank: 0,
            scope: 0,
            bytes: &head,
        };
        let mut catalog = vec![1, 1, 1];
        d.write(&mut catalog);
        let mut models = Models::read(
            &catalog,
            root,
            LossyEntropy::RunLevel,
            crate::parse::header::Features::ALL,
            9,
            &crate::parse::limits::Unrestricted,
        )
        .unwrap();
        for selection in [1, 2, 3] {
            let mut prefix = vec![selection];
            if selection == 1 {
                prefix.push(1);
            } else {
                d.write(&mut prefix);
            }
            prefix.push(0);
            models
                .admit(
                    selection as u64,
                    0,
                    &prefix,
                    root,
                    &crate::parse::limits::Unrestricted,
                )
                .unwrap();
            let mut bytes = prefix;
            bytes.extend_from_slice(&units[0]);
            let (alternate, body) = models.dc(&bytes, root).unwrap();
            let alternate = alternate.unwrap();
            let om = crate::v1::OutMap::new(&alternate.info, &alternate.layout);
            let mut rows = crate::v1::UnitRows::new_with(&alternate, 0, body).unwrap();
            let count = root.hs.min(info.height as usize);
            let mut output = vec![0; info.row_bytes() * count];
            for row in output.chunks_mut(info.row_bytes()) {
                rows.next_row(&alternate, &om, row).unwrap();
            }
            assert_eq!(output, &pixels[..info.row_bytes() * count]);
        }
        assert!(models.dc(&[4, 3, 0], root).unwrap().0.is_some());
        assert!(models.ac(&[1, 1, 0]).is_err());
    }

    #[test]
    fn native_model_choices_preserve_pixels_rows_regions_and_thread_counts() {
        let (info, pixels, baseline) = source();
        let expected = crate::decode(&baseline).unwrap().1;
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for defaults in [false, true] {
                let mut cfg = LossyConfig::new(80.0).with_stripe_block_rows(2);
                cfg.entropy = entropy;
                cfg.column_groups = Some(64);
                cfg.model_references = true;
                cfg.coding_defaults = defaults;
                cfg.unit_ordinals = defaults;
                cfg.dc_coverage = Some(vec![vec![1, 3, 2, 3]]);
                cfg.catalog_lab = Some(model_candidates::Settings {
                    mode: model_candidates::Mode::Mixed,
                    buckets: 2,
                    fast_training: true,
                    witness: true,
                });
                let file =
                    encode_lossy(&pixels, 129 * 3, &info, &Metadata::default(), &cfg).unwrap();
                #[cfg(feature = "lab")]
                assert_eq!(
                    quantized_lab_any(&file).unwrap(),
                    quantized_lab_any(&baseline).unwrap()
                );
                #[cfg(feature = "lab")]
                assert_eq!(
                    dc_image_lab(&file).unwrap().1,
                    dc_image_lab(&baseline).unwrap().1
                );
                let parsed = parse_lossy(&file).unwrap();
                assert!(
                    !parsed.model_refs.as_ref().unwrap().reusable.is_empty(),
                    "positive first-use witness"
                );
                for threads in [1, 2, 4, 8] {
                    let mut out = vec![0; expected.len()];
                    #[cfg(feature = "std")]
                    crate::decode_into_threads(&file, &mut out, 129 * 3, threads).unwrap();
                    #[cfg(not(feature = "std"))]
                    {
                        let _ = threads;
                        crate::decode_into(&file, &mut out, 129 * 3).unwrap();
                    }
                    assert_eq!(out, expected);
                }
                #[cfg(feature = "std")]
                {
                    let mut out = Vec::new();
                    crate::decode_reader(file.as_slice(), &crate::Limits::NONE, |_, _, row| {
                        out.extend_from_slice(row);
                        Ok(())
                    })
                    .unwrap();
                    assert_eq!(out, expected);
                    for (x, y, w, h) in [(67, 35, 61, 70), (0, 115, 129, 14)] {
                        let mut roi = vec![0; w * h * 3];
                        crate::region::decode_region_threads(
                            &file,
                            crate::planes::Rect::new(x as u32, y as u32, w as u32, h as u32),
                            &mut roi,
                            w * 3,
                            4,
                        )
                        .unwrap();
                        for row in 0..h {
                            assert_eq!(
                                &roi[row * w * 3..][..w * 3],
                                &expected[((y + row) * 129 + x) * 3..][..w * 3]
                            );
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn indexed_reference_fetches_definition_without_earlier_ac_entropy() {
        use crate::planes::ReadAt;
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let (info, pixels, _) = source();
        let mut cfg = LossyConfig::new(80.0).with_stripe_block_rows(2);
        cfg.column_groups = Some(64);
        cfg.model_references = true;
        cfg.coding_defaults = true;
        cfg.catalog_lab = Some(model_candidates::Settings {
            mode: model_candidates::Mode::Catalog,
            buckets: 2,
            fast_training: true,
            witness: true,
        });
        let file = encode_lossy(&pixels, 129 * 3, &info, &Metadata::default(), &cfg).unwrap();
        let expected = crate::decode(&file).unwrap().1;
        let p = parse_lossy(&file).unwrap();
        let map = p.unit_map();
        let ng = p.ngroups();
        let mut witness = None;
        for (u, &rec) in p.stripes.iter().enumerate().rev() {
            let span = p.dc_span(u / ng, u % ng);
            let ordinal =
                map.first(u) + usize::from(span.start == u / ng) + usize::from(p.alpha.is_some());
            let bytes = p.model_prefix(u, 1, rec).unwrap();
            let mut r = ByteReader::new(bytes);
            if let (Selector::Earlier(target), _) = prefix(&mut r, 1, p.tables.mode).unwrap() {
                witness = Some((u, target, ordinal));
                break;
            }
        }
        let (u, target, _) = witness.expect("positive backward-reference witness");
        let defining = (0..p.stripes.len())
            .find(|&du| {
                let span = p.dc_span(du / ng, du % ng);
                (map.first(du)
                    + usize::from(span.start == du / ng)
                    + usize::from(p.alpha.is_some())) as u64
                    == target
            })
            .unwrap();
        let rec = p.stripes[defining];
        let bytes = p.model_prefix(defining, 1, rec).unwrap();
        let mut r = ByteReader::new(bytes);
        assert!(matches!(
            prefix(&mut r, 1, p.tables.mode).unwrap().0,
            Selector::Define(_)
        ));
        let forbidden = (
            bytes.as_ptr() as usize - file.as_ptr() as usize + r.pos,
            rec.as_ptr() as usize - file.as_ptr() as usize + rec.len(),
        );
        assert!(forbidden.0 < forbidden.1);
        struct Source<'a> {
            file: &'a [u8],
            active: Arc<AtomicBool>,
            forbidden: (usize, usize),
        }
        impl ReadAt for Source<'_> {
            fn len(&mut self) -> Result<u64, Error> {
                Ok(self.file.len() as u64)
            }
            fn read_exact_at(&mut self, at: u64, out: &mut [u8]) -> Result<(), Error> {
                let a = usize::try_from(at).map_err(|_| Error::TooLarge)?;
                let b = a.checked_add(out.len()).ok_or(Error::TooLarge)?;
                if self.active.load(Ordering::Relaxed)
                    && a < self.forbidden.1
                    && b > self.forbidden.0
                {
                    return Err(Error::Corrupt("unrelated entropy requested"));
                }
                out.copy_from_slice(self.file.get(a..b).ok_or(Error::Corrupt("source range"))?);
                Ok(())
            }
        }
        let active = Arc::new(AtomicBool::new(false));
        let src = Source {
            file: &file,
            active: Arc::clone(&active),
            forbidden,
        };
        let mut reader = crate::region::RegionReader::open(src).unwrap();
        active.store(true, Ordering::Relaxed);
        let x = p.group(u % ng).0;
        let y = p.g.rows(u / ng).0 * 8;
        let mut out = [0; 3];
        reader
            .decode_region(
                crate::planes::Rect::new(x as u32, y as u32, 1, 1),
                &mut out,
                3,
            )
            .unwrap();
        assert_eq!(out, &expected[(y * 129 + x) * 3..][..3]);
    }

    #[test]
    fn bounded_writers_use_current_unit_settings_and_models() {
        let (info, pixels, _) = source();
        let rb = info.row_bytes();
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            let mut cfg = LossyConfig::new(80.0).with_stripe_block_rows(2);
            cfg.entropy = entropy;
            cfg.column_groups = Some(64);
            let encode = |cfg: &LossyConfig| {
                bounded::encode_lossy_bounded(&info, &Metadata::default(), cfg, &mut |_, y, out| {
                    out.copy_from_slice(&pixels[y * rb..][..out.len()]);
                    Ok(())
                })
                .unwrap()
            };
            let baseline = encode(&cfg);
            let expected = crate::decode(&baseline).unwrap().1;
            cfg.coding_defaults = true;
            cfg.model_references = true;
            let file = encode(&cfg);
            assert_eq!(crate::decode(&file).unwrap().1, expected);
            let p = parse_lossy(&file).unwrap();
            assert!(p.coding_defaults.is_some());
            assert!(p.model_refs.is_some());
        }
    }

    #[cfg(feature = "lab")]
    #[test]
    fn declared_depth_alpha_and_strides_preserve_exact_coefficients_and_pixels() {
        for bits in [10, 12, 16] {
            let info = ImageInfo::new(65, 65, Channels::Rgba, SampleType::U16)
                .with_significant_bits(&[bits; 4])
                .unwrap();
            let stride = info.row_bytes() + 16;
            let mut pixels = vec![0x71; stride * 65];
            let mask = (1u32 << bits) - 1;
            for y in 0..65 {
                for x in 0..65 * 4 {
                    let value = ((x * 977 + y * 173 + x * y * 13) as u32 & mask) as u16;
                    pixels[y * stride + x * 2..][..2].copy_from_slice(&value.to_ne_bytes());
                }
            }
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                let mut cfg = LossyConfig {
                    quality: 40.0,
                    effort: 0.0,
                    color: LossyColor::YCoCg,
                    entropy,
                    stripe_block_rows: 2,
                    column_groups: Some(64),
                    ..Default::default()
                };
                let baseline =
                    encode_lossy(&pixels, stride, &info, &Metadata::default(), &cfg).unwrap();
                let expected = crate::decode(&baseline).unwrap().1;
                cfg.model_references = true;
                cfg.coding_defaults = true;
                cfg.dc_coverage = Some(vec![vec![1, 3, 1]]);
                let file =
                    encode_lossy(&pixels, stride, &info, &Metadata::default(), &cfg).unwrap();
                assert_eq!(
                    quantized_lab_any(&file).unwrap(),
                    quantized_lab_any(&baseline).unwrap()
                );
                #[cfg(feature = "lab")]
                assert_eq!(
                    dc_image_lab(&file).unwrap().1,
                    dc_image_lab(&baseline).unwrap().1
                );
                for threads in [1, 2, 4, 8] {
                    let mut out = vec![0x35; stride * 65];
                    #[cfg(feature = "std")]
                    crate::decode_into_threads(&file, &mut out, stride, threads).unwrap();
                    #[cfg(not(feature = "std"))]
                    {
                        let _ = threads;
                        crate::decode_into(&file, &mut out, stride).unwrap();
                    }
                    for y in 0..65 {
                        assert_eq!(
                            &out[y * stride..][..info.row_bytes()],
                            &expected[y * info.row_bytes()..][..info.row_bytes()]
                        );
                        assert!(
                            out[y * stride + info.row_bytes()..(y + 1) * stride]
                                .iter()
                                .all(|&v| v == 0x35)
                        );
                        for x in 0..65 {
                            assert_eq!(
                                &out[y * stride + x * 8 + 6..][..2],
                                &pixels[y * stride + x * 8 + 6..][..2]
                            );
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "lab")]
    #[test]
    fn positive_exact_and_forced_small_transforms_keep_their_pixels() {
        let info = ImageInfo::new(129, 65, Channels::Rgb, SampleType::U8);
        let pixels: Vec<u8> = (0..info.row_bytes() * 65)
            .map(|i| ((i * 37 + i / 23) ^ ((i / 991) * 173)) as u8)
            .collect();
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for exact in [false, true] {
                let mut cfg = LossyConfig {
                    quality: 90.0,
                    effort: 0.0,
                    entropy,
                    stripe_block_rows: 4,
                    aq_gamma: 0.0,
                    exact_blocks: exact.then_some(32.0),
                    ..Default::default()
                };
                cfg.lab.superblocks = Some(false);
                if !exact {
                    cfg.lab.small = Some(Box::new(SmallLab {
                        on: Some(true),
                        split: Some(7),
                        ..Default::default()
                    }));
                }
                let baseline =
                    encode_lossy(&pixels, info.row_bytes(), &info, &Metadata::default(), &cfg)
                        .unwrap();
                let p = parse_lossy(&baseline).unwrap();
                if exact {
                    assert!(p.exact.is_some());
                } else {
                    assert_eq!(p.map, MapKind::Joint);
                    assert!(
                        small_stats_lab(&baseline).unwrap()[2..5]
                            .iter()
                            .any(|&v| v != 0)
                    );
                }
                let expected = crate::decode(&baseline).unwrap().1;
                cfg.model_references = true;
                cfg.coding_defaults = true;
                let file =
                    encode_lossy(&pixels, info.row_bytes(), &info, &Metadata::default(), &cfg)
                        .unwrap();
                assert_eq!(
                    quantized_lab_any(&file).unwrap(),
                    quantized_lab_any(&baseline).unwrap()
                );
                #[cfg(feature = "lab")]
                assert_eq!(
                    dc_image_lab(&file).unwrap().1,
                    dc_image_lab(&baseline).unwrap().1
                );
                assert_eq!(crate::decode(&file).unwrap().1, expected);
            }
        }
    }

    #[cfg(all(feature = "lab", feature = "extended"))]
    #[test]
    fn wide_and_float_samples_keep_pixels_raw_alpha_and_i32_dc() {
        for sample in [SampleType::U32, SampleType::F16, SampleType::F32] {
            let info = ImageInfo::new(33, 35, Channels::Rgba, sample);
            let rb = info.row_bytes();
            let mut pixels = Vec::new();
            for y in 0..35 {
                for x in 0..33 {
                    for c in 0..4 {
                        let selector = (x + y + c) % 4;
                        let value = if c == 3 {
                            (selector % 2) as f32
                        } else {
                            [0.0f32, 1.0, 64.0, 4096.0][selector]
                        };
                        match sample {
                            SampleType::U32 => pixels.extend_from_slice(
                                &((x * 739 + y * 31 + c * 991) as u32)
                                    .wrapping_mul(1_017_031)
                                    .to_ne_bytes(),
                            ),
                            SampleType::F16 => pixels.extend_from_slice(
                                &crate::mapping::f32_to_f16_bits_exact(value.to_bits())
                                    .unwrap()
                                    .to_ne_bytes(),
                            ),
                            SampleType::F32 => pixels.extend_from_slice(&value.to_ne_bytes()),
                            _ => unreachable!(),
                        }
                    }
                }
            }
            for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
                let mut cfg = LossyConfig {
                    quality: 116.0,
                    effort: 0.0,
                    entropy,
                    stripe_block_rows: 2,
                    exact_blocks: None,
                    ..Default::default()
                };
                let baseline =
                    encode_lossy(&pixels, rb, &info, &Metadata::default(), &cfg).unwrap();
                let expected = crate::decode(&baseline).unwrap().1;
                if sample == SampleType::F32 {
                    assert_eq!(
                        parse_lossy(&baseline)
                            .unwrap()
                            .dc
                            .as_ref()
                            .unwrap()
                            .info
                            .sample,
                        SampleType::I32
                    );
                }
                cfg.coding_defaults = true;
                cfg.model_references = true;
                cfg.dc_coverage = Some(vec![vec![1, 2]]);
                let file = encode_lossy(&pixels, rb, &info, &Metadata::default(), &cfg).unwrap();
                assert_eq!(
                    quantized_lab_any(&file).unwrap(),
                    quantized_lab_any(&baseline).unwrap()
                );
                #[cfg(feature = "lab")]
                assert_eq!(
                    dc_image_lab(&file).unwrap().1,
                    dc_image_lab(&baseline).unwrap().1
                );
                for threads in [1, 2, 4, 8] {
                    let mut out = vec![0; expected.len()];
                    #[cfg(feature = "std")]
                    crate::decode_into_threads(&file, &mut out, rb, threads).unwrap();
                    #[cfg(not(feature = "std"))]
                    {
                        let _ = threads;
                        crate::decode_into(&file, &mut out, rb).unwrap();
                    }
                    assert_eq!(out, expected);
                    let bps = sample.bytes();
                    for p in 0..33 * 35 {
                        assert_eq!(
                            &out[(p * 4 + 3) * bps..][..bps],
                            &pixels[(p * 4 + 3) * bps..][..bps]
                        );
                    }
                }
            }
        }
    }
}
