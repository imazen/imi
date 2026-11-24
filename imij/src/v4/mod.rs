//! Version 4 wire components.
//!
//! Spatial tiles select contiguous storage extents; their coding groups retain
//! independent reconstruction tasks and geometry-derived identities. Ordinary header
//! and whole-image slice entry points select this grammar from version 4.
//! Default writers and the remaining consumers are tracked in the coverage doc.

pub(crate) mod admitted;
pub(crate) mod coverage;
pub(crate) mod decode;
pub(crate) mod emit;
pub(crate) mod file;
pub(crate) mod migrate;
mod numeric_migrate;
pub(crate) mod plan;
pub use migrate::upgrade_image;
mod checks;
mod pixel_checks;
pub(crate) use pixel_checks::RetainedPixelCheckState;
pub use pixel_checks::{CheckCoverage, PixelCheckIdentity, PixelChecks};
#[cfg(all(feature = "std", feature = "encode"))]
mod file_write;
mod header;
mod index;
mod layout;
mod numeric;
mod numeric_stream;
pub use checks::{CheckDescriptor, CheckLimits, CheckList, CheckResult, CheckScope, Checks};
pub(crate) use checks::{OwnedChecks, TilePixelCheck, tile_pixel_rect};
mod object;
pub(crate) mod read_at;
mod settings;
pub(crate) use settings::PreparedUnit;
mod storage;
mod tile;
#[cfg(feature = "std")]
pub(crate) use tile::TileState;

pub use layout::{Group, Groups, SpatialTile, TileLayout, TileOrder};
pub use object::{Object, ObjectIntegrity, ObjectReader, SetupContext, append_object};

pub use tile::{ObjectExtent, TileEvent, TileExtent, TileLimits, TileReader, TileWriter};

pub use index::{Directory, DirectoryWriter, Entry, EntryKind, LocatedEntry};

pub use crate::lossy::v4::{
    AcModel, ExactPlan, LossyEvent, LossyFile, LossyGroupDecoder, LossyGroupInputs, LossyModels,
    LossySetup, LossyUnit, LossyWork,
};
pub use numeric::{NumericGeometry, NumericModel};

#[cfg(feature = "encode")]
pub use numeric_stream::NumericGroupEncoder;
#[cfg(feature = "std")]
pub(crate) use numeric_stream::NumericState;
pub use numeric_stream::{NumericEvent, NumericGroupDecoder, NumericWork, SpatialConfig};

pub use storage::{DirectoryPlacement, DirectoryPrefix, append_directory_prefix};

pub use header::header;

pub use file::NumericFile;
#[cfg(feature = "encode")]
pub use file::{encode_numeric, encode_numeric_checked};
#[cfg(all(feature = "std", feature = "encode"))]
pub use file_write::{ChecksummedNumericFileEncoder, NumericFileEncoder};
pub use read_at::NumericTileReader;
#[cfg(feature = "encode")]
pub(crate) use settings::JOINT_SETTINGS;
pub use settings::{CellSettings, LossySettings};
pub(crate) use settings::{RANGE_PARTITIONS, map_budget};

#[cfg(all(test, feature = "encode"))]
pub(crate) mod file_tests;
#[cfg(all(test, feature = "std", feature = "encode"))]
mod file_write_tests;
#[cfg(all(test, feature = "std", feature = "encode"))]
mod read_at_tests;

#[cfg(all(test, feature = "encode"))]
mod plan_tests;

#[cfg(all(test, feature = "std", feature = "encode"))]
pub(crate) mod feed_tests;

#[cfg(all(test, feature = "std", feature = "encode", feature = "lab"))]
mod boundary_tests;

#[cfg(all(
    test,
    feature = "std",
    feature = "lab",
    feature = "encode",
    not(target_family = "wasm")
))]
mod atlas_tests;

#[cfg(all(test, feature = "std", feature = "encode"))]
pub(crate) mod region_tests;

pub(crate) mod reframe;

#[cfg(all(feature = "std", feature = "encode"))]
mod lossy_write;
#[cfg(all(feature = "std", feature = "encode"))]
pub use lossy_write::{BoundedLossyConfig, encode_lossy_bounded, encode_lossy_bounded_seekable};

#[cfg(feature = "std")]
pub(crate) mod traversal;

#[cfg(feature = "encode")]
mod group_write;
#[cfg(feature = "encode")]
pub use group_write::GroupEncodeSession;

#[cfg(all(test, feature = "std", feature = "encode"))]
mod scoped_checks_tests;

#[cfg(feature = "encode")]
mod check_write;
#[cfg(feature = "encode")]
pub use check_write::add_scoped_checks;
