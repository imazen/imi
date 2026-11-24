//! Bounded-memory archives of byte-identical standalone images and tiny blobs.
//!
//! See the crate README and FORMAT.md for the append and recovery contract.
//! Numeric keys increase within each append batch. Latest generation wins.
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
mod dictionary;
mod metadata;
mod policy;
pub mod pyramid;
mod reader;
mod sequence;
mod upgrade;
mod wire;
mod writer;
pub use imij::{Digest, HashAlgorithm};
pub use metadata::{EntryMetadata, Value};
pub use policy::{AccessDecision, EvaluationPurpose, EvaluationRun, HeldOutAccess, ScopeAccess};
pub use pyramid::{
    CATALOG_KEY, CATALOG_MAGIC, PyramidCatalog, PyramidLevel, TileOrder, morton_decode_2d,
    morton_encode_2d,
};
pub use reader::{
    CicpListing, DimensionListing, EntrySource, FilenameListing, Limits, Listing, Reader,
};
pub use sequence::{SequenceIndex, SequenceIndexRef, SequenceLocation};
pub use upgrade::{UpgradeOptions, upgrade};
pub use wire::{Entry, Snapshot, Summary};
pub use writer::{IndexPlacement, Writer};

/// Merge committed segments into a new archive, removing superseded keys.
/// Copies encoded bytes and checks every retained entry; never decodes or re-encodes an image.
/// Preserves a common source digest algorithm. Mixed-algorithm sources select
/// the algorithm required by retained sequence-index bindings, or the default
/// when no binding requires one. Conflicting binding algorithms are refused
/// before writing; [`upgrade_container`] preserves their separate generations.
/// This preflights index metadata; blob bytes are read only once.
pub fn compact<S: imij::planes::ReadAt, W: std::io::Write>(
    reader: &mut Reader<S>,
    sink: W,
) -> std::io::Result<(W, Snapshot)> {
    // Homogeneous archives keep their digest identity, including legacy XXH3-64.
    // A mixed archive can still compact if all sequence bindings require one
    // common algorithm. Preflight metadata before writing any destination bytes.
    let mut algorithm = None;
    let mut mixed = false;
    let mut listing = reader.listing()?;
    while let Some(entry) = listing.next(reader)? {
        let next = entry.checksum().algorithm();
        mixed |= algorithm.is_some_and(|prior| prior != next);
        algorithm.get_or_insert(next);
    }
    if mixed {
        let mut required = None;
        let mut listing = reader.listing()?;
        while let Some(entry) = listing.next(reader)? {
            if let Some(reference) = SequenceIndexRef::from_metadata(entry.metadata())? {
                for next in [
                    entry.checksum().algorithm(),
                    reference.checksum().algorithm(),
                ] {
                    if required.is_some_and(|prior| prior != next) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Unsupported,
                            "sequence bindings need different digest algorithms; use upgrade_container to retain generations",
                        ));
                    }
                    required = Some(next);
                }
            }
        }
        algorithm = required;
    }
    let mut writer =
        Writer::new(sink).with_hash_algorithm(algorithm.unwrap_or(HashAlgorithm::DEFAULT))?;
    let mut listing = reader.listing()?;
    while let Some(entry) = listing.next(reader)? {
        writer.insert_stream_with_metadata(
            entry.key(),
            entry.head_size(),
            entry.summary(),
            entry.metadata(),
            |out| reader.copy_to(&entry, &mut { out }),
        )?;
    }
    writer.finish()
}

/// Rewrite every committed generation using the current archive container format.
///
/// Accepts the container versions supported by [`Reader`] (currently v1 and v2).
/// Rebuilds index pages and footers while retaining keys, summaries, metadata,
/// append generations and each generation's digest algorithm. All blobs,
/// including superseded entries, are copied and their original digests checked.
/// Preserving digest identities keeps embedded sequence-index bindings valid.
///
/// This is a **container-only** upgrade: contained IMI files, nested archives,
/// sequence-index blobs and other opaque data retain their exact bytes and format
/// versions. It neither decodes pixels nor changes hash algorithms. Use
/// [`compact`] separately when removing superseded keys is intended.
///
/// The source must remain immutable. The sink must be a separate empty stream at
/// byte zero and only needs [`std::io::Write`]. Blob copying uses a 64 KiB buffer;
/// index memory is bounded by page size/tree depth plus one snapshot per source
/// generation, admitted by [`Limits::max_segments`]. No entire blob or archive
/// is materialized. A buffering source/sink can add its own memory use.
///
/// On error, the sink may contain partial output or earlier completed
/// generations; it is **not** a successful upgrade. Stage to a separate file,
/// check this result, then apply your own fsync/atomic-publication policy.
/// Successful completion flushes the sink but does not fsync it.
///
/// ```no_run
/// use imij::planes::SeekReader;
/// use imij_set::{Reader, upgrade_container};
/// use std::{fs::File, io::BufWriter};
///
/// let source = File::open("source.imijset")?;
/// let mut reader = Reader::open(SeekReader::new(source))?;
/// // create_new protects existing files, including the source, from truncation.
/// let staging = File::create_new("upgraded.imijset.partial")?;
/// let (mut output, snapshot) = upgrade_container(&mut reader, BufWriter::new(staging))?;
/// output.get_mut().sync_all()?;
/// assert_eq!(snapshot.end(), output.get_ref().metadata()?.len());
/// # Ok::<(), std::io::Error>(())
/// ```
pub fn upgrade_container<S: imij::planes::ReadAt, W: std::io::Write>(
    reader: &mut Reader<S>,
    sink: W,
) -> std::io::Result<(W, Snapshot)> {
    reader.upgrade_container_into(sink)
}
