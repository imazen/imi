//! Declarative image/group/tile deltas; no entropy or transform arithmetic.
use super::*;

pub(super) const HEAD: u64 = crate::registry::HEAD_CODING_DEFAULTS;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Delta {
    pub partition: Option<u8>,
    /// Three plane bits per parent, in 2x2 raster order.
    pub small: Option<u16>,
    pub scale: Option<[u8; 4]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct State {
    pub partition: u8,
    pub small: u16,
    pub scale: [u8; 4],
}

impl State {
    fn apply(mut self, d: Delta) -> Self {
        if let Some(v) = d.partition {
            self.partition = v;
        }
        if let Some(v) = d.small {
            self.small = v;
        }
        if let Some(v) = d.scale {
            self.scale = v;
        }
        self
    }
}

pub(super) fn read_delta(r: &mut ByteReader<'_>) -> Result<Delta, Error> {
    let mask = r.varint()?;
    if mask & !7 != 0 {
        return Err(Error::Unsupported("coding defaults field"));
    }
    let mut d = Delta::default();
    if mask & 1 != 0 {
        let v = r.varint()?;
        if v > 7 {
            return Err(Error::Corrupt("default partition"));
        }
        d.partition = Some(v as u8);
    }
    if mask & 2 != 0 {
        let v = r.varint()?;
        if v > 4095 {
            return Err(Error::Corrupt("default small planes"));
        }
        d.small = Some(v as u16);
    }
    if mask & 4 != 0 {
        let v = r.varint()?;
        d.scale = Some(if v <= 255 {
            [v as u8; 4]
        } else if v == 256 {
            let mut values = [0; 4];
            for value in &mut values {
                let v = r.varint()?;
                if v > 255 {
                    return Err(Error::Corrupt("default parent AC scale"));
                }
                *value = v as u8;
            }
            values
        } else {
            return Err(Error::Unsupported("default AC scale form"));
        });
    }

    Ok(d)
}

pub(super) struct Defaults {
    sets: Vec<(u64, Delta)>,
    image: State,
}

impl Defaults {
    pub fn read(
        bytes: &[u8],
        admission: &impl crate::parse::limits::Admission,
    ) -> Result<Self, Error> {
        let mut r = ByteReader::new(bytes);
        if r.varint()? != 1 {
            return Err(Error::Unsupported("coding defaults version"));
        }
        let count = r.var_usize()?;
        if count > r.remaining() / 2 {
            return Err(Error::Corrupt("coding defaults count"));
        }
        admission.hint(
            count
                .checked_mul(core::mem::size_of::<(u64, Delta)>())
                .ok_or(Error::TooLarge)?,
        )?;
        admission
            .wire_extra_bytes((count as u128) * core::mem::size_of::<(u64, Delta)>() as u128)?;
        let mut sets = Vec::new();
        sets.try_reserve_exact(count).map_err(|_| Error::TooLarge)?;
        let mut last = 0;
        for _ in 0..count {
            admission.check()?;
            let id = r.varint()?;
            if id <= last {
                return Err(Error::Corrupt("coding defaults ids"));
            }
            last = id;
            let d = read_delta(&mut r)?;
            sets.push((id, d));
        }
        let mut out = Self {
            sets,
            image: State::default(),
        };
        out.image = out.select(out.image, r.varint()?)?;
        if r.remaining() != 0 {
            return Err(Error::Corrupt("coding defaults tail"));
        }
        Ok(out)
    }

    pub(super) fn sets(&self) -> &[(u64, Delta)] {
        &self.sets
    }
    pub(super) fn image(&self) -> State {
        self.image
    }

    fn select(&self, parent: State, id: u64) -> Result<State, Error> {
        if id == 0 {
            return Ok(parent);
        }
        let i = self
            .sets
            .binary_search_by_key(&id, |&(id, _)| id)
            .map_err(|_| Error::Corrupt("unknown coding set"))?;
        Ok(parent.apply(self.sets[i].1))
    }

    fn scope(&self, parent: State, r: &mut ByteReader<'_>) -> Result<State, Error> {
        match r.varint()? {
            0 => Ok(parent),
            1 => Ok(parent.apply(read_delta(r)?)),
            2 => self.select(parent, r.varint()?),
            _ => Err(Error::Unsupported("coding selection kind")),
        }
    }

    /// Prefix boundary is length-delimited, independent of tile geometry.
    pub fn split(bytes: &[u8]) -> Result<(&[u8], &[u8]), Error> {
        let mut r = ByteReader::new(bytes);
        let n = r.var_usize()?;
        let settings = r.bytes(n)?;
        Ok((settings, &bytes[r.pos..]))
    }

    pub fn expand(
        &self,
        bytes: &[u8],
        rows: usize,
        cols: usize,
        mut tile: impl FnMut(usize, State) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let (settings, _) = Self::split(bytes)?;
        let n = rows
            .div_ceil(2)
            .checked_mul(cols.div_ceil(2))
            .ok_or(Error::TooLarge)?;
        if settings.is_empty() {
            for i in 0..n {
                tile(i, self.image)?;
            }
            return Ok(());
        }
        let mut r = ByteReader::new(settings);
        let group = self.scope(self.image, &mut r)?;
        let count = r.var_usize()?;
        if count > n || count > r.remaining() / 2 {
            return Err(Error::Corrupt("coding override count"));
        }
        let mut next = 0usize;
        for j in 0..count {
            let delta = r.var_usize()?;
            let at = next
                .checked_add(delta)
                .filter(|&v| v < n)
                .ok_or(Error::Corrupt("coding override position"))?;
            for i in next..at {
                tile(i, group)?;
            }
            tile(at, self.scope(group, &mut r)?)?;
            next = at + 1;
            if j + 1 < count && next == n {
                return Err(Error::Corrupt("coding override position"));
            }
        }
        for i in next..n {
            tile(i, group)?;
        }
        if r.remaining() != 0 {
            return Err(Error::Corrupt("coding settings tail"));
        }
        Ok(())
    }

    pub fn maps(
        &self,
        p: &LossyParsed<'_>,
        bytes: &[u8],
        rows: usize,
        cols: usize,
        aq: &[f32],
        roles: &mut Vec<u8>,
        splits: &mut Vec<u8>,
        scales: &mut Vec<f32>,
    ) -> Result<(bool, bool), Error> {
        let n = rows.checked_mul(cols).ok_or(Error::TooLarge)?;
        resize_fallible(roles, n, 0)?;
        resize_fallible(splits, n, 0)?;
        resize_fallible(scales, n, 1.0)?;
        roles.fill(0);
        splits.fill(0);
        if !aq.is_empty() {
            if aq.len() < n {
                return Err(Error::Corrupt("coding scale count"));
            }
            scales.copy_from_slice(&aq[..n]);
        } else {
            scales.fill(1.0);
        }
        let mut merged = false;
        let mut small = false;
        self.expand(bytes, rows, cols, |tile, state| {
            let (y, x) = (tile / cols.div_ceil(2) * 2, tile % cols.div_ceil(2) * 2);
            let pattern = superblock::PARTITIONS[state.partition as usize];
            for (j, &role) in pattern.iter().enumerate() {
                let (yy, xx) = (y + j / 2, x + j % 2);
                let mask = ((state.small >> (j * 3)) & 7) as u8;
                if yy >= rows || xx >= cols {
                    if role != 0 || mask != 0 {
                        return Err(Error::Corrupt("coding tile past unit"));
                    }
                    continue;
                }
                let at = yy * cols + xx;
                let bypass =
                    (p.exact.is_some() && scales[at] == 0.0) || (p.skip && scales[at] == 0.0);
                if role != 0 && p.sb.is_none() {
                    return Err(Error::Corrupt("default merge without feature"));
                }
                if mask != 0 && p.small.is_none() {
                    return Err(Error::Corrupt("default small without feature"));
                }
                if role != 0 && (bypass || mask != 0) {
                    return Err(Error::Corrupt("conflicting coding tile"));
                }
                roles[at] = role;
                splits[at] = if bypass { 0 } else { mask };
                if state.scale[j] != 0 && !bypass {
                    scales[at] = f32::from(state.scale[j]) / 64.0;
                }
                merged |= role != 0;
                small |= splits[at] != 0;
            }
            Ok(())
        })?;
        if merged {
            superblock::validate(roles, scales, cols, code0_skips(p))?;
        }
        Ok((merged, small))
    }
}

/// Re-express existing partition decisions; no transform or quantizer search.
#[cfg(feature = "encode")]
pub(super) fn encode(stripes: &[StripeCoefs]) -> Result<(Vec<u8>, Vec<Vec<u8>>), Error> {
    let mut decisions = Vec::new();
    let mut palette = Vec::<(u8, u16)>::new();
    for sc in stripes {
        let mut tiles = Vec::new();
        let ac = superblock::ac_bits(&sc.q, sc.nby * sc.bw);
        for y in (0..sc.nby).step_by(2) {
            for x in (0..sc.bw).step_by(2) {
                let mut roles = [0; 4];
                let mut mask = 0;
                for (j, role) in roles.iter_mut().enumerate() {
                    let (yy, xx) = (y + j / 2, x + j % 2);
                    if yy < sc.nby && xx < sc.bw {
                        let at = yy * sc.bw + xx;
                        *role = sc.modes.get(at).copied().unwrap_or(0);
                        mask |=
                            u16::from(sc.splits.get(at).copied().unwrap_or(0) & ac[at]) << (3 * j);
                    }
                }
                let pattern =
                    superblock::symbol_of(roles).ok_or(Error::InvalidInput("coding partition"))?;
                let state = (pattern, mask);
                let id = if let Some(i) = palette.iter().position(|s| *s == state) {
                    i
                } else {
                    palette.push(state);
                    palette.len() - 1
                };
                tiles.push(id);
            }
        }
        decisions.push(tiles);
    }
    let mut counts = alloc::vec![0usize; palette.len()];
    for &id in decisions.iter().flatten() {
        counts[id] += 1;
    }
    let image = counts
        .iter()
        .enumerate()
        .max_by_key(|&(i, n)| (*n, core::cmp::Reverse(i)))
        .map_or(0, |(i, _)| i);
    let mut head = Vec::new();
    put_varint(&mut head, 1);
    put_varint(&mut head, palette.len() as u64);
    for (i, &(pattern, small)) in palette.iter().enumerate() {
        for v in [i as u64 + 1, 3, u64::from(pattern), u64::from(small)] {
            put_varint(&mut head, v);
        }
    }
    put_varint(&mut head, image as u64 + 1);
    let mut settings = Vec::new();
    for tiles in decisions {
        counts.fill(0);
        for &i in &tiles {
            counts[i] += 1;
        }
        let group = counts
            .iter()
            .enumerate()
            .max_by_key(|&(i, n)| (*n, core::cmp::Reverse(i)))
            .map_or(image, |(i, _)| i);
        let overrides = tiles.iter().filter(|&&i| i != group).count();
        let mut body = Vec::new();
        if group != image || overrides != 0 {
            if group == image {
                put_varint(&mut body, 0);
            } else {
                put_varint(&mut body, 2);
                put_varint(&mut body, group as u64 + 1);
            }
            put_varint(&mut body, overrides as u64);
            let mut next = 0;
            for (at, &id) in tiles.iter().enumerate() {
                if id != group {
                    put_varint(&mut body, (at - next) as u64);
                    put_varint(&mut body, 2);
                    put_varint(&mut body, id as u64 + 1);
                    next = at + 1;
                }
            }
        }
        settings.push(body);
    }
    Ok((head, settings))
}

/// Causal representation: current-unit deltas, with no future-image catalog.
#[cfg(feature = "encode")]
pub(super) fn encode_inline(stripe: &StripeCoefs) -> Result<Vec<u8>, Error> {
    let (head, settings) = encode(core::slice::from_ref(stripe))?;
    let defaults = Defaults::read(&head, &crate::parse::limits::Unrestricted)?;
    let mut framed = Vec::new();
    put_varint(&mut framed, settings[0].len() as u64);
    framed.extend_from_slice(&settings[0]);
    let mut tiles = Vec::new();
    defaults.expand(&framed, stripe.nby, stripe.bw, |_, state| {
        tiles.push(state);
        Ok(())
    })?;
    let group = defaults.image;
    let overrides = tiles.iter().filter(|&&state| state != group).count();
    if group == State::default() && overrides == 0 {
        return Ok(Vec::new());
    }
    fn inline(out: &mut Vec<u8>, state: State) {
        for v in [1, 3, u64::from(state.partition), u64::from(state.small)] {
            put_varint(out, v);
        }
    }
    let mut out = Vec::new();
    inline(&mut out, group);
    put_varint(&mut out, overrides as u64);
    let mut next = 0;
    for (at, &state) in tiles.iter().enumerate() {
        if state != group {
            put_varint(&mut out, (at - next) as u64);
            inline(&mut out, state);
            next = at + 1;
        }
    }
    Ok(out)
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn scoped_deltas_inherit_fields_without_leaking_between_tiles() {
        // Image: 16x16 and scale 80. Group changes only scale to 96.
        // Tile 1 changes only partition to 8x8; tile 2 inherits the group.
        let head = [1, 3, 1, 5, 1, 80, 2, 4, 96, 3, 1, 0, 1];
        let defaults = Defaults::read(&head, &crate::parse::limits::Unrestricted).unwrap();
        let bytes = [6, 2, 2, 1, 1, 2, 3];
        let mut states = Vec::new();
        defaults
            .expand(&bytes, 2, 6, |_, s| {
                states.push(s);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            states
                .iter()
                .map(|s| (s.partition, s.scale[0]))
                .collect::<Vec<_>>(),
            [(1, 96), (0, 96), (1, 96)]
        );
        for invalid in [
            &[4, 0, 1, 3, 0][..],
            &[3, 2, 4, 0][..],
            &[4, 2, 2, 0, 0][..],
        ] {
            assert!(defaults.expand(invalid, 2, 6, |_, _| Ok(())).is_err());
        }
    }

    #[test]
    fn malformed_catalogs_and_full_width_ids() {
        let mut head = vec![1, 1];
        put_varint(&mut head, u64::MAX);
        head.extend_from_slice(&[1, 0]);
        put_varint(&mut head, u64::MAX);
        assert!(Defaults::read(&head, &crate::parse::limits::Unrestricted).is_ok());
        for bytes in [
            &[1, 255, 255, 255, 255, 15][..],
            &[1, 1, 0, 0, 0][..],
            &[1, 1, 1, 4, 0x81, 2, 1][..],
            &[1, 1, 1, 1, 8, 1][..],
            &[1, 0, 1][..],
        ] {
            assert!(Defaults::read(bytes, &crate::parse::limits::Unrestricted).is_err());
        }
    }

    #[test]
    fn existing_quantized_choices_decode_identically_through_every_route() {
        for entropy in [LossyEntropy::RunLevel, LossyEntropy::SplitHuf] {
            for (w, h, columns, compact) in
                [(1, 1, 0, false), (73, 99, 32, false), (128, 131, 64, true)]
            {
                let info = ImageInfo::new(w, h, Channels::Rgb, SampleType::U8);
                let px = super::super::tests::test_image(w, h, 41);
                let mut cfg = LossyConfig::new(80.0).with_stripe_block_rows(3);
                cfg.column_groups = Some(columns);
                cfg.entropy = entropy;
                cfg.decode_speed = compact.then_some(crate::DecodeSpeed::Compact);
                let old =
                    encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
                cfg.coding_defaults = true;
                let new =
                    encode_lossy(&px, w as usize * 3, &info, &Metadata::default(), &cfg).unwrap();
                let expected = crate::decode(&old).unwrap().1;
                assert_eq!(crate::decode(&new).unwrap().1, expected);
                #[cfg(feature = "std")]
                {
                    let mut rows = Vec::new();
                    crate::decode_reader(new.as_slice(), &crate::Limits::NONE, |_, _, row| {
                        rows.extend_from_slice(row);
                        Ok(())
                    })
                    .unwrap();
                    assert_eq!(rows, expected);
                    let rect = crate::planes::Rect::new(
                        w / 3,
                        h / 3,
                        (w - w / 3).min(19),
                        (h - h / 3).min(23),
                    );
                    let rb = rect.width() as usize * 3;
                    let mut roi = vec![0; rb * rect.height() as usize];
                    crate::region::decode_region(&new, rect, &mut roi, rb).unwrap();
                    for y in 0..rect.height() as usize {
                        assert_eq!(
                            &roi[y * rb..][..rb],
                            &expected
                                [((rect.y() as usize + y) * w as usize + rect.x() as usize) * 3..]
                                [..rb]
                        );
                    }
                }
                #[cfg(feature = "std")]
                for threads in [1, 2, 4, 8] {
                    let stride = w as usize * 3 + 7;
                    let mut out = vec![0xA5; stride * h as usize];
                    crate::decode_into_threads(&new, &mut out, stride, threads).unwrap();
                    for y in 0..h as usize {
                        assert_eq!(
                            &out[y * stride..][..w as usize * 3],
                            &expected[y * w as usize * 3..][..w as usize * 3]
                        );
                        assert!(
                            out[y * stride + w as usize * 3..(y + 1) * stride]
                                .iter()
                                .all(|&v| v == 0xA5)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn per_parent_scales_can_restore_numeric_aq_without_changing_other_fields() {
        // Image: uniform 16x16, explicit 2x AC. Group switches to 8x8;
        // a four-parent scale delta restores AQ for parent zero only.
        let mut head = Vec::new();
        for value in [1, 2, 1, 5, 1, 128, 2, 5, 0, 256, 0, 96, 64, 128, 1] {
            put_varint(&mut head, value);
        }
        let defaults = Defaults::read(&head, &crate::parse::limits::Unrestricted).unwrap();
        let mut body = Vec::new();
        for value in [2, 2, 0] {
            put_varint(&mut body, value);
        } // catalog set 2, no tile overrides
        let mut bytes = Vec::new();
        put_varint(&mut bytes, body.len() as u64);
        bytes.extend(body);
        let mut state = None;
        defaults
            .expand(&bytes, 2, 2, |_, s| {
                state = Some(s);
                Ok(())
            })
            .unwrap();
        let state = state.unwrap();
        assert_eq!(state.partition, 0);
        assert_eq!(state.scale, [0, 96, 64, 128]);
        // An inline delta changes scale only and inherits the image partition.
        let bytes = [4, 1, 4, 64, 0];
        defaults
            .expand(&bytes, 2, 2, |_, s| {
                assert_eq!((s.partition, s.scale), (1, [64; 4]));
                Ok(())
            })
            .unwrap();
    }
}
