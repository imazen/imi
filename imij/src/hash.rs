//! The digest algorithms IMI files and imij-set archives store: a wire id, a
//! digest width, and one streaming hasher. Every table that holds digests
//! (`cSUM` pieces, `pSUM` rasters, frame references, archive pages and blobs)
//! names its algorithm, so a writer may pick any algorithm here and a reader
//! verifies every algorithm it knows and skips the rest.
//!
//! Image checksum results can be wider than existing fixed archive slots.

/// A digest algorithm and its wire id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HashAlgorithm {
    /// XXH3-64, seed 0, default secret: 8 bytes. Written until 2026-10-25;
    /// still verified.
    Xxh3_64,
    /// XXH3-128, seed 0, default secret: 16 bytes. The default.
    Xxh3_128,
    #[cfg(feature = "blake3")]
    Blake3_256,
    #[cfg(feature = "sha2")]
    Sha256,
    #[cfg(feature = "sha2")]
    Sha512,
    #[cfg(feature = "blake3")]
    Blake3_512,
    #[cfg(feature = "blake3")]
    Blake3_1024,
}

impl HashAlgorithm {
    /// What writers use unless told otherwise.
    pub const DEFAULT: Self = Self::Xxh3_128;

    /// The wire id (one byte in every table that stores digests).
    pub const fn wire(self) -> u8 {
        match self {
            Self::Xxh3_64 => 0,
            Self::Xxh3_128 => 1,
            #[cfg(feature = "blake3")]
            Self::Blake3_256 => 2,
            #[cfg(feature = "sha2")]
            Self::Sha256 => 3,
            #[cfg(feature = "sha2")]
            Self::Sha512 => 4,
            #[cfg(feature = "blake3")]
            Self::Blake3_512 => 5,
            #[cfg(feature = "blake3")]
            Self::Blake3_1024 => 6,
        }
    }

    /// The algorithm with wire id `id`; `None` for one this build does not
    /// know (its tables are skipped, never reported as verified).
    pub const fn from_wire(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Xxh3_64),
            1 => Some(Self::Xxh3_128),
            #[cfg(feature = "blake3")]
            2 => Some(Self::Blake3_256),
            #[cfg(feature = "sha2")]
            3 => Some(Self::Sha256),
            #[cfg(feature = "sha2")]
            4 => Some(Self::Sha512),
            #[cfg(feature = "blake3")]
            5 => Some(Self::Blake3_512),
            #[cfg(feature = "blake3")]
            6 => Some(Self::Blake3_1024),
            _ => None,
        }
    }

    /// Digest bytes.
    pub const fn width(self) -> usize {
        match self {
            Self::Xxh3_64 => 8,
            Self::Xxh3_128 => 16,
            #[cfg(feature = "blake3")]
            Self::Blake3_256 => 32,
            #[cfg(feature = "sha2")]
            Self::Sha256 => 32,
            #[cfg(feature = "sha2")]
            Self::Sha512 => 64,
            #[cfg(feature = "blake3")]
            Self::Blake3_512 => 64,
            #[cfg(feature = "blake3")]
            Self::Blake3_1024 => 128,
        }
    }

    /// The digest of `data` in one call.
    pub fn digest(self, data: &[u8]) -> Digest {
        let mut h = Hasher::new(self);
        h.update(data);
        h.finish()
    }
}

impl Default for HashAlgorithm {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A digest in its algorithm's byte order.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest {
    algo: HashAlgorithm,
    bytes: [u8; Digest::STORAGE_WIDTH],
}

impl Digest {
    /// Fixed archive and sequence slot width in bytes.
    pub const MAX_WIDTH: usize = 32;
    /// Maximum image digest result width in bytes.
    pub const MAX_RESULT_WIDTH: usize = 128;
    #[cfg(any(feature = "blake3", feature = "sha2"))]
    pub(crate) const STORAGE_WIDTH: usize = Self::MAX_RESULT_WIDTH;
    #[cfg(not(any(feature = "blake3", feature = "sha2")))]
    pub(crate) const STORAGE_WIDTH: usize = Self::MAX_WIDTH;

    /// A digest from its stored bytes: exactly `algo.width()` of them.
    pub fn from_bytes(algo: HashAlgorithm, bytes: &[u8]) -> Option<Self> {
        if bytes.len() != algo.width() {
            return None;
        }
        let mut out = [0u8; Self::STORAGE_WIDTH];
        out[..bytes.len()].copy_from_slice(bytes);
        Some(Self { algo, bytes: out })
    }

    /// A digest from a [`Self::MAX_WIDTH`]-byte slot: the width's bytes
    /// followed by zeros (anything else is `None`).
    pub fn from_slot(algo: HashAlgorithm, slot: &[u8; Self::MAX_WIDTH]) -> Option<Self> {
        if algo.width() > slot.len() || slot[algo.width()..].iter().any(|&b| b != 0) {
            return None;
        }
        Self::from_bytes(algo, &slot[..algo.width()])
    }

    pub const fn algorithm(&self) -> HashAlgorithm {
        self.algo
    }

    /// The digest's bytes, in storage order, `algorithm().width()` long.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.algo.width()]
    }

    /// The digest in a [`Self::MAX_WIDTH`]-byte slot, zero-padded.
    /// Panics for wide image digests. Use [`Self::try_to_slot`] to check
    /// whether a caller-selected digest fits.
    pub const fn to_slot(&self) -> [u8; Self::MAX_WIDTH] {
        assert!(
            self.algo.width() <= Self::MAX_WIDTH,
            "digest exceeds fixed slot width"
        );
        let mut slot = [0; Self::MAX_WIDTH];
        let mut i = 0;
        while i < slot.len() {
            slot[i] = self.bytes[i];
            i += 1;
        }
        slot
    }

    /// A fixed slot for a digest whose full result fits.
    pub const fn try_to_slot(&self) -> Option<[u8; Self::MAX_WIDTH]> {
        if self.algo.width() <= Self::MAX_WIDTH {
            Some(self.to_slot())
        } else {
            None
        }
    }

    /// Whether `stored` is a prefix (a truncation) of this digest.
    pub fn matches_prefix(&self, stored: &[u8]) -> bool {
        stored.len() <= self.algo.width() && self.bytes[..stored.len()] == *stored
    }

    /// The low 64 bits (the whole digest for XXH3-64).
    pub fn low_u64(&self) -> u64 {
        u64::from_le_bytes(self.bytes[..8].try_into().unwrap())
    }
}

impl core::fmt::LowerHex for Digest {
    /// The digest bytes as lowercase hex, in storage order.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for b in self.as_bytes() {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl core::fmt::Debug for Digest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}:", self.algo)?;
        for b in self.as_bytes() {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

/// A streaming hasher for one [`HashAlgorithm`].
#[derive(Clone)]
pub struct Hasher {
    state: State,
    algo: HashAlgorithm,
}

#[derive(Clone)]
enum State {
    Xxh3(zenhash::Xxh3),
    #[cfg(feature = "blake3")]
    Blake3(alloc::boxed::Box<blake3::Hasher>),
    #[cfg(feature = "sha2")]
    Sha256(sha2::Sha256),
    #[cfg(feature = "sha2")]
    Sha512(sha2::Sha512),
}

impl Hasher {
    pub fn new(algo: HashAlgorithm) -> Self {
        #[cfg(feature = "sha2")]
        use sha2::Digest as _;
        let state = match algo {
            HashAlgorithm::Xxh3_64 | HashAlgorithm::Xxh3_128 => State::Xxh3(zenhash::Xxh3::new()),
            #[cfg(feature = "blake3")]
            HashAlgorithm::Blake3_256 | HashAlgorithm::Blake3_512 | HashAlgorithm::Blake3_1024 => {
                State::Blake3(alloc::boxed::Box::new(blake3::Hasher::new()))
            }
            #[cfg(feature = "sha2")]
            HashAlgorithm::Sha256 => State::Sha256(sha2::Sha256::new()),
            #[cfg(feature = "sha2")]
            HashAlgorithm::Sha512 => State::Sha512(sha2::Sha512::new()),
        };
        Self { state, algo }
    }

    pub const fn algorithm(&self) -> HashAlgorithm {
        self.algo
    }

    pub fn update(&mut self, data: &[u8]) {
        #[cfg(feature = "sha2")]
        use sha2::Digest as _;
        match &mut self.state {
            State::Xxh3(hash) => hash.update(data),
            #[cfg(feature = "blake3")]
            State::Blake3(hash) => {
                hash.update(data);
            }
            #[cfg(feature = "sha2")]
            State::Sha256(hash) => hash.update(data),
            #[cfg(feature = "sha2")]
            State::Sha512(hash) => hash.update(data),
        }
    }

    /// The digest of everything fed so far; the hasher may go on.
    pub fn finish(&self) -> Digest {
        #[cfg(feature = "sha2")]
        use sha2::Digest as _;
        let mut bytes = [0u8; Digest::STORAGE_WIDTH];
        match &self.state {
            State::Xxh3(hash) if self.algo == HashAlgorithm::Xxh3_64 => {
                bytes[..8].copy_from_slice(&hash.digest().to_le_bytes())
            }
            State::Xxh3(hash) => bytes[..16].copy_from_slice(&hash.digest128().to_le_bytes()),
            #[cfg(feature = "blake3")]
            State::Blake3(hash) => hash.finalize_xof().fill(&mut bytes[..self.algo.width()]),
            #[cfg(feature = "sha2")]
            State::Sha256(hash) => bytes[..32].copy_from_slice(&hash.clone().finalize()),
            #[cfg(feature = "sha2")]
            State::Sha512(hash) => bytes[..64].copy_from_slice(&hash.clone().finalize()),
        }
        Digest {
            algo: self.algo,
            bytes,
        }
    }

    /// Forgets everything fed so far.
    pub fn reset(&mut self) {
        #[cfg(feature = "sha2")]
        use sha2::Digest as _;
        match &mut self.state {
            State::Xxh3(hash) => hash.reset(),
            #[cfg(feature = "blake3")]
            State::Blake3(hash) => {
                hash.reset();
            }
            #[cfg(feature = "sha2")]
            State::Sha256(hash) => hash.reset(),
            #[cfg(feature = "sha2")]
            State::Sha512(hash) => hash.reset(),
        }
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn digests_match_the_one_shot_functions_and_round_trip_through_slots() {
        let data = b"hello world";
        let a = HashAlgorithm::Xxh3_64.digest(data);
        assert_eq!(a.as_bytes(), zenhash::xxh3_64(data).to_le_bytes());
        let b = HashAlgorithm::Xxh3_128.digest(data);
        assert_eq!(b.as_bytes(), zenhash::xxh3_128(data).to_le_bytes());
        assert_eq!(
            Digest::from_slot(HashAlgorithm::Xxh3_128, &b.to_slot()),
            Some(b)
        );
        assert!(b.matches_prefix(&b.as_bytes()[..4]));
        assert!(!b.matches_prefix(&a.as_bytes()[..4]) || a.as_bytes()[..4] == b.as_bytes()[..4]);
        let mut slot = b.to_slot();
        slot[31] = 1;
        assert_eq!(Digest::from_slot(HashAlgorithm::Xxh3_128, &slot), None);
        assert_eq!(HashAlgorithm::from_wire(255), None);
        for algo in [HashAlgorithm::Xxh3_64, HashAlgorithm::Xxh3_128] {
            assert_eq!(HashAlgorithm::from_wire(algo.wire()), Some(algo));
            assert_eq!(algo.digest(data).as_bytes().len(), algo.width());
        }
    }

    #[cfg(feature = "sha2")]
    #[test]
    fn sha2_matches_standard_abc_vectors_and_supports_fragmented_updates() {
        use alloc::format;
        let expected = [
            (
                HashAlgorithm::Sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                HashAlgorithm::Sha512,
                concat!(
                    "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a",
                    "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
                ),
            ),
        ];
        for (algorithm, expected) in expected {
            let mut state = Hasher::new(algorithm);
            state.update(b"a");
            state.update(b"bc");
            assert_eq!(format!("{:x}", state.finish()), expected);
            state.reset();
            state.update(b"abc");
            assert_eq!(state.finish(), algorithm.digest(b"abc"));
        }
    }
}
