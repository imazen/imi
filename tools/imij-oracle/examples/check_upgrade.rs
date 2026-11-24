//! Independently decode migrated bytes against sealed source/reconstruction hashes.
fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: check_upgrade SET_DIR");
    let report = imij_oracle::check::check_set_with_decode(std::path::Path::new(&path), |bytes| {
        // Preserve the independent decoder's coverage classification. Every input
        // it can decode must also migrate and decode to the manifest's raw hash.
        imij_oracle::decode(bytes)?;
        let upgraded = imij::upgrade_image(bytes, &Default::default())
            .expect("upgrade of independently decoded fixture failed");
        Ok(imij_oracle::decode(&upgraded).expect("independent decode of upgraded fixture failed"))
    })
    .expect("read sealed set");
    print!("{}", report.text);
    assert_eq!(
        report.failures, 0,
        "migrated output differs from sealed pixels"
    );
}
