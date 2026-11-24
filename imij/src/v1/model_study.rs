//! Native numeric model trials. Reuse the ordinary analysis, row predictors,
//! mapping, tokenization, model builder, serializer and unit decoder.
//! Test executable only; no production grammar or public interface.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Training {
    Global,
    Bucket(usize),
    Inline(usize),
}

pub(super) struct UnitCounts {
    pub slots: Vec<Vec<Vec<u64>>>,
    pub palette: Option<Vec<Vec<u64>>>,
}

pub(crate) struct Definition {
    pub training: Training,
    pub bank: u64,
    pub head: Vec<u8>,
    pub records: Vec<Vec<u8>>,
}
impl Definition {
    pub(super) fn new(
        training: Training,
        bank: u64,
        (head, records): (Vec<u8>, Vec<Vec<u8>>),
    ) -> Self {
        Self {
            training,
            bank,
            head,
            records,
        }
    }
    pub(crate) fn parsed(&self, info: &ImageInfo) -> Result<Parsed<'static>, Error> {
        let bank = if self.bank == 0 {
            None
        } else {
            Some(crate::seeds::bank(self.bank).ok_or(Error::Unsupported("numeric model bank"))?)
        };
        let mut r = ByteReader::new(&self.head);
        let p = parse_head_nested(
            info.clone(),
            &mut r,
            &crate::parse::limits::Unrestricted,
            bank,
            crate::parse::header::Features::ALL,
        )?;
        if r.remaining() != 0 {
            return Err(Error::Corrupt("numeric model head tail"));
        }
        Ok(p)
    }
    pub(crate) fn unit(&self, info: &ImageInfo, u: usize) -> Result<Vec<u8>, Error> {
        let p = self.parsed(info)?;
        let (_, gw, _) = p.group(u % p.ngroups());
        let mut row = vec![0; gw * p.bpp()];
        let mut output = Vec::new();
        let om = OutMap::new(&p.info, &p.layout);
        unit_rows(
            &p,
            &om,
            u,
            self.records
                .get(u)
                .ok_or(Error::Corrupt("numeric model unit"))?,
            &mut row,
            |row| {
                output.extend_from_slice(row);
                Ok(())
            },
            &|| Ok(()),
        )?;
        Ok(output)
    }
}

pub(crate) struct NativeStudy {
    pub buckets: usize,
    pub inline: bool,
    pub definitions: Vec<Definition>,
}

type SeedChoices = Vec<(usize, u8, f64, crate::seeds::Expanded)>;
impl NativeStudy {
    pub(super) fn collect(
        &mut self,
        counts: &[UnitCounts],
        global: &[Model],
        slots: usize,
        bank: Option<&'static crate::seeds::Bank>,
        fit: impl Fn(usize, &[Vec<u64>]) -> (Model, SeedChoices),
        emit: impl Fn(
            &[Model],
            Option<&'static crate::seeds::Bank>,
            Option<usize>,
        ) -> Result<(Vec<u8>, Vec<Vec<u8>>), Error>,
    ) -> Result<(), Error> {
        self.definitions.push(Definition::new(
            Training::Global,
            0,
            emit(global, None, None)?,
        ));
        let mut specs: Vec<_> = (0..self.buckets).map(Training::Bucket).collect();
        if self.inline {
            specs.extend((0..counts.len()).map(Training::Inline));
        }
        for training in specs {
            let fitted: Vec<_> = global
                .iter()
                .enumerate()
                .map(|(slot, m)| {
                    let hist = histogram(
                        counts,
                        slot,
                        slots,
                        m.tc.alphabet() as usize,
                        training,
                        self.buckets,
                    );
                    fit(slot, &hist)
                })
                .collect();
            let plain: Vec<_> = fitted.iter().map(|p| p.0.clone()).collect();
            let unit = match training {
                Training::Inline(u) => Some(u),
                _ => None,
            };
            self.definitions
                .push(Definition::new(training, 0, emit(&plain, None, unit)?));
            if let Some(bank) = bank {
                let seeded: Vec<_> = fitted
                    .into_iter()
                    .map(|(mut m, candidates)| {
                        for (c, i, _, e) in candidates {
                            m.freq[c] = e.freq;
                            m.weights[c] = e.weights;
                            m.prec[c] = e.prec;
                            m.seeds[c] = Some(i);
                        }
                        m
                    })
                    .collect();
                self.definitions.push(Definition::new(
                    training,
                    bank.id,
                    emit(&seeded, Some(bank), unit)?,
                ));
            }
        }
        Ok(())
    }
}

pub(crate) fn prepare(
    pixels: &[u8],
    stride: usize,
    info: &ImageInfo,
    cfg: &EncodeConfig,
    bank: Option<&'static crate::seeds::Bank>,
    buckets: usize,
    inline: bool,
) -> Result<NativeStudy, Error> {
    let mut study = NativeStudy {
        buckets: buckets.clamp(1, 16),
        inline,
        definitions: Vec::new(),
    };
    encode_parts_ref_with_progress(
        pixels,
        stride,
        info,
        cfg,
        None,
        bank.map(|b| (b, true)),
        false,
        &how_far::NoPulse,
        None,
        Some(&mut study),
    )?;
    Ok(study)
}

pub(super) fn histogram(
    counts: &[UnitCounts],
    slot: usize,
    slots: usize,
    alphabet: usize,
    training: Training,
    buckets: usize,
) -> Vec<Vec<u64>> {
    let mut total = vec![vec![0u64; alphabet]; CTX_CLASSES];
    let hist = |u: usize| {
        if slot < slots {
            counts[u].slots.get(slot)
        } else {
            counts[u].palette.as_ref()
        }
    };
    for u in 0..counts.len() {
        if let Some(h) = hist(u) {
            for (out, row) in total.iter_mut().zip(h) {
                for (out, &n) in out.iter_mut().zip(row) {
                    *out += n;
                }
            }
        }
    }
    if matches!(training, Training::Global) {
        return total;
    }
    for n in total.iter_mut().flatten() {
        *n = if matches!(training, Training::Inline(_)) {
            0
        } else {
            u64::from(*n != 0)
        };
    }
    for u in 0..counts.len() {
        let selected = match training {
            Training::Global => true,
            Training::Bucket(k) => u * buckets / counts.len() == k,
            Training::Inline(at) => u == at,
        };
        if selected && let Some(h) = hist(u) {
            for (out, row) in total.iter_mut().zip(h) {
                for (out, &n) in out.iter_mut().zip(row) {
                    *out += n;
                }
            }
        }
    }
    total
}

#[test]
fn native_numeric_model_trials_preserve_units_and_global_bytes() {
    for sample in [SampleType::U16, SampleType::I32] {
        for palette in [false, true] {
            let info = ImageInfo::new(65, 17, crate::Channels::Rgba, sample);
            let stride = info.row_bytes() + 16;
            let mut pixels = vec![0xa7; stride * info.height as usize];
            for y in 0..info.height as usize {
                for x in 0..info.width as usize * 4 {
                    let v = if palette {
                        [0x0111u32, 0x7fff, 0xf00f][(x / 4 / 7 + y / 4) % 3]
                            ^ ((x % 4) as u32 * 0x1234)
                    } else {
                        (x * 977 + y * 113 + x * y * 13) as u32
                    };
                    let at = y * stride + x * sample.bytes();
                    if sample == SampleType::U16 {
                        pixels[at..at + 2].copy_from_slice(&(v as u16).to_ne_bytes());
                    } else {
                        pixels[at..at + 4].copy_from_slice(&(v ^ 0x8000_0000).to_ne_bytes());
                    }
                }
            }
            let cfg = EncodeConfig {
                effort: 0.0,
                stripe_rows: Some(4),
                lanes: Some(8),
                column_groups: Some(0),
                float_shift: false,
                ..Default::default()
            };
            let plain = encode_parts(&pixels, stride, &info, &cfg).unwrap();
            if palette && sample == SampleType::U16 {
                let d = Definition::new(Training::Global, 0, (plain.0.clone(), plain.1.clone()));
                assert!(
                    d.parsed(&info).unwrap().pal.is_some(),
                    "positive palette witness"
                );
            }
            for bank in [None, crate::seeds::bank(1)] {
                let study = prepare(&pixels, stride, &info, &cfg, bank, 2, true).unwrap();
                assert_eq!(
                    (&study.definitions[0].head, &study.definitions[0].records),
                    (&plain.0, &plain.1)
                );
                for d in &study.definitions {
                    for u in 0..d.records.len() {
                        if let Training::Inline(at) = d.training
                            && at != u
                        {
                            assert!(d.records[u].is_empty());
                            continue;
                        }
                        let output = d.unit(&info, u).unwrap();
                        let y = u * 4;
                        let rows = 4.min(info.height as usize - y);
                        let expected: Vec<u8> = (y..y + rows)
                            .flat_map(|y| {
                                pixels[y * stride..y * stride + info.row_bytes()]
                                    .iter()
                                    .copied()
                            })
                            .collect();
                        assert_eq!(
                            output, expected,
                            "{sample:?} {:?} bank={} unit={u}",
                            d.training, d.bank
                        );
                    }
                }
            }
        }
    }
}
