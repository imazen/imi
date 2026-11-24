#![cfg(feature = "cli")]

use imij_set::HashAlgorithm;
use std::{fs, path::PathBuf, process::Command, time::SystemTime};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "archive-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_imij-set"));
        command.current_dir(&self.0);
        command
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn pack_list_and_verify_wide_digests() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("first image.imij"), b"abc").unwrap();
    fs::write(scratch.0.join("scores.json"), b"{}").unwrap();
    let help = scratch.command().arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .contains("blake3-1024")
    );
    for (name, algorithm) in [
        ("xxh3-128", HashAlgorithm::Xxh3_128),
        ("sha512", HashAlgorithm::Sha512),
        ("blake3-512", HashAlgorithm::Blake3_512),
        ("blake3-1024", HashAlgorithm::Blake3_1024),
    ] {
        let archive = format!("{name}.imijset");
        let args = [
            "pack",
            "--hash",
            name,
            &archive,
            "first image.imij",
            "scores.json",
        ];
        let packed = scratch.command().args(args).output().unwrap();
        assert!(
            packed.status.success(),
            "{}",
            String::from_utf8_lossy(&packed.stderr)
        );
        let listing = scratch.command().args(["list", &archive]).output().unwrap();
        assert!(listing.status.success());
        let listing = String::from_utf8(listing.stdout).unwrap();
        assert!(listing.contains(&format!(
            "0\t3\t{name}\t{:x}\t\"first image.imij\"",
            algorithm.digest(b"abc")
        )));
        let verified = scratch
            .command()
            .args(["verify", &archive])
            .output()
            .unwrap();
        assert!(verified.status.success());
        assert_eq!(verified.stdout, b"Verified 2 entries (5 payload bytes).\n");

        let path = scratch.0.join(&archive);
        let mut contents = fs::read(&path).unwrap();
        let exists = scratch.command().args(args).output().unwrap();
        assert!(!exists.status.success());
        assert_eq!(fs::read(&path).unwrap(), contents);

        contents[0] ^= 1;
        fs::write(&path, contents).unwrap();
        let damaged = scratch
            .command()
            .args(["verify", &archive])
            .output()
            .unwrap();
        assert_eq!(damaged.status.code(), Some(1));
        assert!(damaged.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&damaged.stderr).contains("entry 0: entry checksum mismatch")
        );
    }
}
