#![forbid(unsafe_code)]

use imij::planes::SeekReader;
use imij_set::{EntryMetadata, HashAlgorithm, Reader, Summary, Writer};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP: &str = "\
Inspect, verify, and package IMIJ-set archives.

Usage:
  imij-set list ARCHIVE
  imij-set verify ARCHIVE
  imij-set pack [--hash ALGORITHM] [--] NEW_ARCHIVE INPUT...

list writes a TSV inventory of the latest entries and their complete digests.
verify checks the archive index and every latest entry's encoded bytes.
Governed scopes require an application with an audit grant.
pack keeps input bytes unchanged, uses basenames as entry names, and refuses
to overwrite an existing archive. Default hash: xxh3-128.

Algorithms: xxh3-64, xxh3-128, blake3-256, blake3-512, blake3-1024,
            sha256, sha512
";

const ALGORITHMS: [(&str, HashAlgorithm); 7] = [
    ("xxh3-64", HashAlgorithm::Xxh3_64),
    ("xxh3-128", HashAlgorithm::Xxh3_128),
    ("blake3-256", HashAlgorithm::Blake3_256),
    ("blake3-512", HashAlgorithm::Blake3_512),
    ("blake3-1024", HashAlgorithm::Blake3_1024),
    ("sha256", HashAlgorithm::Sha256),
    ("sha512", HashAlgorithm::Sha512),
];

enum Command {
    Help,
    Version,
    List(PathBuf),
    Verify(PathBuf),
    Pack {
        algorithm: HashAlgorithm,
        output: PathBuf,
        inputs: Vec<PathBuf>,
    },
}

impl Command {
    fn parse(args: &[OsString]) -> Result<Self, String> {
        let Some((command, args)) = args.split_first() else {
            return Ok(Self::Help);
        };
        if args == ["--help"] || args == ["-h"] {
            return Ok(Self::Help);
        }
        match (command.to_str(), args) {
            (Some("-h" | "--help"), []) => Ok(Self::Help),
            (Some("-V" | "--version"), []) => Ok(Self::Version),
            (Some("list"), [path]) => Ok(Self::List(path.into())),
            (Some("verify"), [path]) => Ok(Self::Verify(path.into())),
            (Some("list" | "verify"), _) => Err("supply exactly one archive path".into()),
            (Some("pack"), _) => {
                let (algorithm, args) = if args.first().is_some_and(|arg| arg == "--hash") {
                    let Some((name, paths)) = args[1..].split_first() else {
                        return Err("--hash requires an algorithm name".into());
                    };
                    let algorithm = ALGORITHMS
                        .iter()
                        .find(|(label, _)| name == *label)
                        .map(|(_, algorithm)| *algorithm)
                        .ok_or_else(|| format!("unknown checksum algorithm {:?}", name))?;
                    (algorithm, paths)
                } else {
                    (HashAlgorithm::DEFAULT, args)
                };
                let args = match args.split_first() {
                    Some((flag, paths)) if flag == "--" => paths,
                    Some((flag, _)) if flag.as_encoded_bytes().starts_with(b"-") => {
                        return Err(format!("unknown pack option {:?}", flag));
                    }
                    _ => args,
                };
                let Some((output, inputs)) =
                    args.split_first().filter(|(_, inputs)| !inputs.is_empty())
                else {
                    return Err("pack requires a new archive path and at least one input".into());
                };
                Ok(Self::Pack {
                    algorithm,
                    output: output.into(),
                    inputs: inputs.iter().map(PathBuf::from).collect(),
                })
            }
            _ => Err(format!("unknown command {:?}", command)),
        }
    }
}

fn path_error(action: &str, path: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("{action} {}: {error}", path.display()),
    )
}

fn pack(algorithm: HashAlgorithm, output: &Path, paths: Vec<PathBuf>) -> io::Result<()> {
    let mut names = BTreeSet::new();
    let mut inputs = Vec::with_capacity(paths.len());
    for path in paths {
        let info = path
            .metadata()
            .map_err(|error| path_error("inspect", &path, error))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.chars().any(char::is_control) && name.len() <= 4096);
        let name = name.filter(|name| names.insert((*name).to_owned()));
        let Some(name) = name.filter(|_| info.is_file()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} needs a regular file with a distinct UTF-8 basename",
                    path.display()
                ),
            ));
        };
        inputs.push((name.to_owned(), path));
    }
    let file = File::create_new(output).map_err(|error| path_error("create", output, error))?;
    let mut writer = Writer::new(BufWriter::new(file)).with_hash_algorithm(algorithm)?;
    for (key, (name, path)) in inputs.into_iter().enumerate() {
        let mut input = File::open(&path).map_err(|error| path_error("open", &path, error))?;
        let info = input
            .metadata()
            .map_err(|error| path_error("inspect", &path, error))?;
        if !info.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is no longer a regular file", path.display()),
            ));
        }
        let mut metadata = EntryMetadata::default();
        metadata.filename = Some(name);
        writer
            .insert_with_metadata(
                key as u64,
                info.len(),
                0,
                Summary::default(),
                &metadata,
                &mut input,
            )
            .map_err(|error| path_error("pack", &path, error))?;
    }
    writer
        .finish()
        .map_err(|error| path_error("commit", output, error))?;
    Ok(())
}

fn inspect(path: &Path, verify: bool, out: &mut impl Write) -> io::Result<()> {
    let file = File::open(path).map_err(|error| path_error("open", path, error))?;
    let mut reader =
        Reader::open(SeekReader::new(file)).map_err(|error| path_error("read", path, error))?;
    let mut listing = reader.listing()?;
    let mut count = 0u64;
    let mut bytes = 0u64;
    if !verify {
        writeln!(out, "key\tbytes\talgorithm\tdigest\tname")?;
    }
    while let Some(entry) = listing.next(&mut reader)? {
        if verify {
            reader.copy_to(&entry, &mut io::sink()).map_err(|error| {
                io::Error::new(error.kind(), format!("entry {}: {error}", entry.key()))
            })?;
            count += 1;
            bytes += entry.len();
        } else {
            let digest = entry.checksum();
            let algorithm = ALGORITHMS
                .iter()
                .find(|(_, algorithm)| *algorithm == digest.algorithm())
                .map_or("unknown", |(name, _)| *name);
            let name = entry.metadata().filename.as_deref().unwrap_or("");
            writeln!(
                out,
                "{}\t{}\t{}\t{:x}\t{:?}",
                entry.key(),
                entry.len(),
                algorithm,
                digest,
                name
            )?;
        }
    }
    if verify {
        writeln!(out, "Verified {count} entries ({bytes} payload bytes).")?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let command = match Command::parse(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("imij-set: {error}\nSee imij-set --help.");
            return ExitCode::from(2);
        }
    };
    let mut out = io::stdout().lock();
    let result = match command {
        Command::Help => write!(out, "{HELP}"),
        Command::Version => writeln!(out, "imij-set {}", env!("CARGO_PKG_VERSION")),
        Command::List(path) => inspect(&path, false, &mut out),
        Command::Verify(path) => inspect(&path, true, &mut out),
        Command::Pack {
            algorithm,
            output,
            inputs,
        } => pack(algorithm, &output, inputs),
    };
    match result.and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("imij-set: {error}");
            ExitCode::FAILURE
        }
    }
}
