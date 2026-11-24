//! Bounded nested archive views through production index validation.
use imij::{Error, planes::ReadAt};
use imij_set::Reader;
use std::{collections::BTreeMap, io};
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
#[derive(Clone)]
pub struct Window<S> {
    source: S,
    start: u64,
    length: u64,
}
impl<S: ReadAt + Clone> Window<S> {
    pub fn new(mut source: S) -> Result<Self, Error> {
        let length = source.len()?;
        Ok(Self {
            source,
            start: 0,
            length,
        })
    }
    fn child(&self, start: u64, length: u64) -> Result<Self, Error> {
        if start
            .checked_add(length)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::Read);
        }
        Ok(Self {
            source: self.source.clone(),
            start: self.start.checked_add(start).ok_or(Error::Read)?,
            length,
        })
    }
}
impl<S: ReadAt> ReadAt for Window<S> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.length)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.read_many_at(&mut [(offset, out)])
    }
    fn read_many_at(&mut self, requests: &mut [(u64, &mut [u8])]) -> Result<(), Error> {
        for (offset, out) in requests.iter() {
            if offset
                .checked_add(out.len() as u64)
                .is_none_or(|end| end > self.length)
                || self.start.checked_add(*offset).is_none()
            {
                return Err(Error::Read);
            }
        }
        // Keep caller offsets unchanged, including on errors.
        let mut translated = requests
            .iter_mut()
            .map(|(o, b)| (self.start + *o, &mut **b))
            .collect::<Vec<_>>();
        self.source.read_many_at(&mut translated)
    }
}

pub struct ArchiveTree<S> {
    root: Window<S>,
    sources: BTreeMap<Vec<u64>, Window<S>>,
    readers: BTreeMap<Vec<u64>, Reader<Window<S>>>,
}
impl<S: ReadAt + Clone> ArchiveTree<S> {
    pub fn new(source: S) -> Result<Self, Error> {
        Ok(Self {
            root: Window::new(source)?,
            sources: BTreeMap::new(),
            readers: BTreeMap::new(),
        })
    }
    pub fn source(&mut self, path: &[u64]) -> io::Result<Window<S>> {
        if path.len() > 8 {
            return Err(invalid("archive nesting exceeds replay limit 8"));
        }
        if path.is_empty() {
            return Ok(self.root.clone());
        }
        if let Some(source) = self.sources.get(path) {
            return Ok(source.clone());
        }
        let parent = &path[..path.len() - 1];
        let source = self.source(parent)?;
        let reader = self.reader(parent)?;
        let entry = reader
            .get(path[path.len() - 1])?
            .ok_or_else(|| invalid("missing nested archive key"))?;
        // Let the production adapter validate snapshot bounds before retaining an owned view.
        let _checked = reader.entry_source(&entry)?;
        let child = source
            .child(entry.offset(), entry.len())
            .map_err(io::Error::other)?;
        self.sources.insert(path.to_vec(), child.clone());
        Ok(child)
    }
    pub fn reader(&mut self, path: &[u64]) -> io::Result<&mut Reader<Window<S>>> {
        if !self.readers.contains_key(path) {
            let source = self.source(path)?;
            self.readers.insert(path.to_vec(), Reader::open(source)?);
        }
        Ok(self.readers.get_mut(path).unwrap())
    }
}
