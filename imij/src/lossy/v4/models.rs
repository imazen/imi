//! Typed immutable definitions. IDs are sorted keys, never array lengths.

use alloc::{sync::Arc, vec::Vec};

use super::{AcModel, LossySetup};
use crate::decode_resources::DecodeSession;
use crate::parse::limits::Admission;
use crate::v4::NumericModel;
use crate::{ByteReader, Error};

#[derive(Clone)]
pub(crate) enum Model {
    Ac(Arc<AcModel>),
    Numeric(Arc<NumericModel>),
}

#[derive(Default)]
struct Plane {
    high_water: Option<u64>,
    payload: Vec<(u64, Model)>,
}

struct Transient {
    role: u8,
    id: u64,
    target: u64,
    model: Model,
}

/// Forward V4 model namespace. The owning driver supplies complete definition
/// bodies in physical order and visits every main unit. Directory binding and
/// group geometry are separate validation obligations.
pub struct LossyModels<'s, C> {
    setup: Arc<LossySetup>,
    planes: [Plane; 4],
    pending: Option<Transient>,
    begun: bool,
    awaiting_main: bool,
    session: &'s DecodeSession<C>,
}

#[cfg(feature = "std")]
pub(crate) struct ModelsState {
    setup: Arc<LossySetup>,
    planes: [Plane; 4],
    pending: Option<Transient>,
    begun: bool,
    awaiting_main: bool,
}
#[cfg(feature = "std")]
impl ModelsState {
    pub(crate) fn bind<C: Fn() -> bool + Sync>(
        self,
        session: &DecodeSession<C>,
    ) -> LossyModels<'_, C> {
        LossyModels {
            setup: self.setup,
            planes: self.planes,
            pending: self.pending,
            begun: self.begun,
            awaiting_main: self.awaiting_main,
            session,
        }
    }
}

impl<'s, C: Fn() -> bool + Sync> LossyModels<'s, C> {
    #[cfg(feature = "std")]
    pub(crate) fn into_state(self) -> ModelsState {
        ModelsState {
            setup: self.setup,
            planes: self.planes,
            pending: self.pending,
            begun: self.begun,
            awaiting_main: self.awaiting_main,
        }
    }
    pub fn new(setup: Arc<LossySetup>, session: &'s DecodeSession<C>) -> Self {
        Self {
            setup,
            planes: core::array::from_fn(|_| Plane::default()),
            pending: None,
            begun: false,
            awaiting_main: false,
            session,
        }
    }

    pub fn setup(&self) -> &LossySetup {
        &self.setup
    }

    pub(super) fn shared_setup(&self) -> Arc<LossySetup> {
        Arc::clone(&self.setup)
    }

    /// Parse record 63's body. The object layer owns framing/input/integrity
    /// admission. Expanded models and registry capacity are charged here.
    pub fn define(&mut self, body: &[u8]) -> Result<(), Error> {
        self.session.adapter_work(body.len() as u128 + 1)?;
        let definition = Definition::read(&self.setup, body)?;
        let Definition {
            role, id, target, ..
        } = definition;
        let transient = target.is_some();
        let plane = &mut self.planes[role as usize];
        if plane.high_water.is_some_and(|old| id <= old) {
            return Err(Error::Corrupt("V4 model ID reuse/order"));
        }
        if id == 0 && (self.begun || transient) || id != 0 && plane.high_water.is_none() {
            return Err(Error::Corrupt("V4 root model placement/lifetime"));
        }
        if transient && self.pending.is_some() {
            return Err(Error::Corrupt("two pending V4 next-unit models"));
        }
        if !transient {
            self.session
                .v4_models()
                .hint(core::mem::size_of::<(u64, Model)>())?;
            plane
                .payload
                .try_reserve_exact(1)
                .map_err(|_| Error::TooLarge)?;
        }
        let model = definition.expand(&self.setup, self.session)?;
        plane.high_water = Some(id);
        if let Some(target) = target {
            self.pending = Some(Transient {
                role,
                id,
                target,
                model,
            });
        } else {
            plane.payload.push((id, model));
        }
        self.awaiting_main = true;
        Ok(())
    }

    fn roots(&self) -> Result<(), Error> {
        for role in 0..4 {
            if (role == 1 || self.setup.numeric[role].is_some())
                && self.planes[role].high_water.is_none()
            {
                return Err(Error::Corrupt("V4 roots missing before main unit"));
            }
        }
        Ok(())
    }

    pub(super) fn take(&mut self, role: u8, unit: u64, id: u64) -> Result<Model, Error> {
        self.session.adapter_work(1)?;
        self.roots()?;
        if role >= 4 || role != 1 && self.setup.numeric[role as usize].is_none() {
            return Err(Error::Corrupt("V4 main plane role"));
        }
        if let Some(pending) = &self.pending {
            if (pending.role, pending.target, pending.id) != (role, unit, id) {
                return Err(Error::Corrupt("V4 next-unit model target/use"));
            }
            self.begun = true;
            self.awaiting_main = false;
            return Ok(self.pending.take().unwrap().model);
        }
        let plane = &self.planes[role as usize];
        let i = plane
            .payload
            .binary_search_by_key(&id, |&(key, _)| key)
            .map_err(|_| Error::Corrupt("undefined or expired V4 model"))?;
        self.begun = true;
        self.awaiting_main = false;
        Ok(plane.payload[i].1.clone())
    }

    pub fn finish(&self) -> Result<(), Error> {
        self.roots()?;
        if self.pending.is_some() || self.awaiting_main {
            return Err(Error::Corrupt(
                "V4 definition without a following main unit",
            ));
        }
        Ok(())
    }
}

pub(crate) struct Definition<'a> {
    pub role: u8,
    pub id: u64,
    pub target: Option<u64>,
    bank: u64,
    bytes: &'a [u8],
}
impl<'a> Definition<'a> {
    pub(crate) fn read(setup: &LossySetup, body: &'a [u8]) -> Result<Self, Error> {
        let mut r = ByteReader::new(body);
        crate::trace::trace_enter_addr("payload", "V4:4", "immutable model definition", r.mark());
        let role = u8::try_from(crate::trace::tf!("model role", r, r.varint()?))
            .ok()
            .filter(|&v| v < 4)
            .ok_or(Error::Unsupported("V4 model role"))?;
        if role != 1 && setup.numeric[role as usize].is_none() {
            return Err(Error::Corrupt("V4 model for absent plane"));
        }
        let id = crate::trace::tf!("model ID", r, r.varint()?);
        let transient = match crate::trace::tf!("model lifetime", r, r.varint()?) {
            0 => false,
            1 => true,
            _ => return Err(Error::Unsupported("V4 model lifetime")),
        };
        let target = if transient {
            Some(crate::trace::tf!("next unit target", r, r.varint()?))
        } else {
            None
        };
        let bank = crate::trace::tf!("seed bank ID", r, r.varint()?);
        if bank != 0 && setup.banks.binary_search(&bank).is_err() {
            return Err(Error::Corrupt("undeclared V4 model bank"));
        }
        if bank == 1 && !setup.declared.contains(crate::Feature::SeedBank1) {
            return Err(Error::Corrupt("V4 bank 1 without its capability"));
        }
        if bank != 0 && role == 3 {
            return Err(Error::Unsupported("V4 exact model seed role"));
        }
        let length = crate::trace::tf!("model bytes", r, r.var_usize()?);
        let definition = r.bytes(length)?;
        if r.remaining() != 0 {
            return Err(Error::Corrupt("V4 model trailing bytes"));
        }
        crate::trace::exit_addr(r.mark());
        Ok(Self {
            role,
            id,
            target,
            bank,
            bytes: definition,
        })
    }
    #[cfg(feature = "lab")]
    pub(super) fn inspection_bank(&self) -> u64 {
        self.bank
    }
    #[cfg(feature = "lab")]
    pub(super) fn inspection_bytes(&self) -> &[u8] {
        self.bytes
    }

    pub(crate) fn expand<C: Fn() -> bool + Sync>(
        &self,
        setup: &LossySetup,
        session: &DecodeSession<C>,
    ) -> Result<Model, Error> {
        Ok(if self.role == 1 {
            Model::Ac(Arc::new(AcModel::read(
                setup.coder,
                self.bank,
                self.bytes,
                session,
            )?))
        } else {
            Model::Numeric(Arc::new(NumericModel::read(
                setup.numeric[self.role as usize]
                    .as_ref()
                    .ok_or(Error::Corrupt("V4 model for absent plane"))?
                    .clone(),
                self.bytes,
                self.bank,
                setup.declared,
                session,
            )?))
        })
    }
}

pub(crate) trait ModelResolver {
    #[cfg(all(feature = "lab", feature = "std"))]
    fn trace_settings(
        &self,
        setup: &crate::v4::LossySetup,
        id: u64,
        settings: &[u8],
    ) -> Result<(), Error>;

    fn define(&mut self, body: &[u8]) -> Result<(), Error>;
    fn take(&mut self, role: u8, unit: u64, id: u64) -> Result<Model, Error>;
}
impl<C: Fn() -> bool + Sync> ModelResolver for LossyModels<'_, C> {
    #[cfg(all(feature = "lab", feature = "std"))]
    fn trace_settings(
        &self,
        setup: &crate::v4::LossySetup,
        id: u64,
        settings: &[u8],
    ) -> Result<(), Error> {
        setup.trace_settings(id, settings, self.session)
    }

    fn define(&mut self, body: &[u8]) -> Result<(), Error> {
        LossyModels::define(self, body)
    }
    fn take(&mut self, role: u8, unit: u64, id: u64) -> Result<Model, Error> {
        LossyModels::take(self, role, unit, id)
    }
}
