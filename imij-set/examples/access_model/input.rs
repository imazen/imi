//! Physical files and bounded nested archive views; no replacement wire parser.
use imij::{
    Error,
    planes::{ReadAt, SeekReader},
};
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};
use xxhash_rust::xxh3::Xxh3;

#[derive(Clone)]
struct Part {
    path: PathBuf,
    length: u64,
    hash: u64,
}
#[derive(Clone)]
pub struct Input {
    parts: Vec<Part>,
    length: u64,
    pub part_size: u64,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn hash_file(path: &Path) -> io::Result<(u64, u64)> {
    let mut file = File::open(path)?;
    let mut hash = Xxh3::new();
    let mut length = 0;
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        length += n as u64;
    }
    Ok((length, hash.digest()))
}
impl Input {
    pub fn open(path: &str) -> io::Result<Self> {
        if !path.ends_with(".parts.tsv") {
            let (length, hash) = hash_file(Path::new(path))?;
            return Ok(Self {
                parts: vec![Part {
                    path: path.into(),
                    length,
                    hash,
                }],
                length,
                part_size: 0,
            });
        }
        let manifest = std::fs::read_to_string(path)?;
        let mut lines = manifest.lines();
        let head = lines
            .next()
            .ok_or_else(|| invalid("empty part manifest"))?
            .split('\t')
            .collect::<Vec<_>>();
        if head.len() != 3 || head[0] != "imi-access-parts-v1" {
            return Err(invalid("part manifest header"));
        }
        let part_size = head[1].parse::<u64>().map_err(|_| invalid("part size"))?;
        let length = head[2]
            .parse::<u64>()
            .map_err(|_| invalid("logical size"))?;
        if part_size == 0 || length == 0 || length.div_ceil(part_size) > 65536 {
            return Err(invalid("part count/size"));
        }
        let parent = Path::new(path).parent().unwrap_or(Path::new("."));
        let mut parts = Vec::new();
        for line in lines {
            let f = line.split('\t').collect::<Vec<_>>();
            if f.len() != 3 {
                return Err(invalid("part manifest row"));
            }
            let length = f[1].parse().map_err(|_| invalid("part length"))?;
            let hash = u64::from_str_radix(f[2], 16).map_err(|_| invalid("part digest"))?;
            parts.push(Part {
                path: parent.join(f[0]),
                length,
                hash,
            });
        }
        if parts.len() as u64 != length.div_ceil(part_size) {
            return Err(invalid("part count"));
        }
        for (i, part) in parts.iter().enumerate() {
            if part.length != (length - i as u64 * part_size).min(part_size) {
                return Err(invalid("part geometry"));
            }
        }
        let input = Self {
            parts,
            length,
            part_size,
        };
        input.verify()?;
        Ok(input)
    }
    /// Integrity I/O is outside the simulated transport clock, before and after replay.
    pub fn verify(&self) -> io::Result<()> {
        for part in &self.parts {
            if hash_file(&part.path)? != (part.length, part.hash) {
                return Err(invalid("part/source identity changed"));
            }
        }
        Ok(())
    }
}
impl ReadAt for Input {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.length)
    }
    fn read_exact_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(out.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::Read);
        }
        let size = if self.part_size == 0 {
            self.length.max(1)
        } else {
            self.part_size
        };
        let mut offset = offset;
        let mut output = out;
        while !output.is_empty() {
            let part = &self.parts[(offset / size) as usize];
            let local = offset % size;
            let n = (part.length - local).min(output.len() as u64) as usize;
            let file = File::open(&part.path).map_err(|_| Error::Read)?;
            SeekReader::new(file).read_exact_at(local, &mut output[..n])?;
            offset += n as u64;
            output = &mut output[n..];
        }
        Ok(())
    }
}
