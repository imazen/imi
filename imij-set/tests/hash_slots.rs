use imij::planes::SliceReader;
use imij_set::{HashAlgorithm, IndexPlacement, Reader, Summary, Writer};

const FOOTER: usize = 136;

fn word(bytes: &[u8], at: usize) -> usize {
    usize::try_from(u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())).unwrap()
}

fn archive(algorithm: HashAlgorithm, count: u64) -> Vec<u8> {
    let mut writer = Writer::new(Vec::new())
        .with_hash_algorithm(algorithm)
        .unwrap()
        .with_index_placement(IndexPlacement::AtCommit)
        .unwrap();
    for key in 0..count {
        writer
            .insert(
                key,
                8,
                0,
                Summary::default(),
                &mut key.to_le_bytes().as_slice(),
            )
            .unwrap();
    }
    let (file, snapshot) = writer.finish().unwrap();
    assert_eq!(snapshot.algorithm(), algorithm);
    file
}

fn check_footer(file: &[u8], algorithm: HashAlgorithm) {
    let footer = &file[file.len() - FOOTER..];
    let offset = word(footer, 24);
    let length = word(footer, 32);
    let width = algorithm.width().min(32);
    let root = algorithm.digest(&file[offset..offset + length]);
    let digest = algorithm.digest(&footer[..104]);
    assert_eq!(&footer[40..40 + width], &root.as_bytes()[..width]);
    assert_eq!(&footer[104..104 + width], &digest.as_bytes()[..width]);
    assert!(footer[40 + width..72].iter().all(|&n| n == 0));
    assert!(footer[104 + width..].iter().all(|&n| n == 0));
}

#[test]
fn short_slots_keep_zero_padding_and_require_it() {
    let algorithm = HashAlgorithm::Xxh3_64;
    let mut file = archive(algorithm, 1);
    check_footer(&file, algorithm);
    let last = file.len() - 1;
    file[last] = 1;
    assert!(Reader::open(SliceReader::new(&file)).is_err());
}

#[cfg(any(feature = "blake3", feature = "sha2"))]
fn check_wide(algorithm: HashAlgorithm) {
    const COUNT: u64 = 300; // More than one leaf.
    let file = archive(algorithm, COUNT);
    check_footer(&file, algorithm);
    let mut reader = Reader::open(SliceReader::new(&file)).unwrap();
    let entries = reader.get_many(&[0, 299, 4]).unwrap();
    for (key, entry) in [0u64, 299, 4].into_iter().zip(entries) {
        let entry = entry.unwrap();
        assert_eq!(entry.checksum(), algorithm.digest(&key.to_le_bytes()));
        assert_eq!(entry.checksum().as_bytes().len(), algorithm.width());
        assert_eq!(
            reader.read_entry_bounded(&entry, 8).unwrap(),
            key.to_le_bytes()
        );
    }
    let (upgraded, _) = imij_set::upgrade_container(&mut reader, Vec::new()).unwrap();
    let (compacted, _) = imij_set::compact(&mut reader, Vec::new()).unwrap();
    for bytes in [&upgraded, &compacted] {
        let mut rewritten = Reader::open(SliceReader::new(bytes)).unwrap();
        assert_eq!(rewritten.snapshot().algorithm(), algorithm);
        let entry = rewritten.get(299).unwrap().unwrap();
        assert_eq!(entry.checksum(), algorithm.digest(&299u64.to_le_bytes()));
    }

    let mut append = Writer::append(file.clone(), reader.snapshot(), file.len() as u64).unwrap();
    append
        .insert(COUNT, 3, 0, Summary::default(), &mut b"new".as_slice())
        .unwrap();
    let (mixed, _) = append.finish().unwrap();
    let mut mixed = Reader::open(SliceReader::new(&mixed)).unwrap();
    let entries = mixed.get_many(&[COUNT - 1, COUNT]).unwrap();
    assert_eq!(
        entries[0].as_ref().unwrap().checksum().algorithm(),
        algorithm
    );
    assert_eq!(
        entries[1].as_ref().unwrap().checksum().algorithm(),
        HashAlgorithm::DEFAULT
    );

    let mut corrupt = file.clone();
    let footer = corrupt.len() - FOOTER;
    corrupt[footer + 104 + 31] ^= 1;
    assert!(Reader::open(SliceReader::new(&corrupt)).is_err());
    let recovered = Reader::recover(SliceReader::new(&file), file.len() as u64).unwrap();
    assert_eq!(recovered.snapshot().algorithm(), algorithm);

    // Modify only the suffix of a child hash, recompute the root prefix and
    // footer, and show that child validation requires that full suffix.
    let mut corrupt = file.clone();
    let root = word(&file[footer..], 24);
    let len = word(&file[footer..], 32);
    assert_eq!(file[root], 6); // Filename-bounded branch, 4-byte page header.
    corrupt[root + 4 + 24 + 32] ^= 1;
    let root_hash = algorithm.digest(&corrupt[root..root + len]);
    corrupt[footer + 40..footer + 72].copy_from_slice(&root_hash.as_bytes()[..32]);
    let footer_hash = algorithm.digest(&corrupt[footer..footer + 104]);
    corrupt[footer + 104..].copy_from_slice(&footer_hash.as_bytes()[..32]);
    let mut reader = Reader::open(SliceReader::new(&corrupt)).unwrap();
    assert!(reader.get(0).is_err());
    assert_eq!(reader.get(299).unwrap().unwrap().key(), 299);

    let mut corrupt = file;
    corrupt[0] ^= 1;
    let mut reader = Reader::open(SliceReader::new(&corrupt)).unwrap();
    let entry = reader.get(0).unwrap().unwrap();
    assert!(reader.read_entry_bounded(&entry, 8).is_err());
}

#[cfg(feature = "blake3")]
#[test]
fn blake3_wide_slots_preserve_complete_index_digests() {
    check_footer(
        &archive(HashAlgorithm::Blake3_256, 1),
        HashAlgorithm::Blake3_256,
    );
    check_wide(HashAlgorithm::Blake3_512);
    check_wide(HashAlgorithm::Blake3_1024);
}

#[cfg(feature = "sha2")]
#[test]
fn sha512_slots_preserve_complete_index_digests() {
    check_footer(&archive(HashAlgorithm::Sha256, 1), HashAlgorithm::Sha256);
    check_wide(HashAlgorithm::Sha512);
}
