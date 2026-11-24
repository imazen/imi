#[allow(dead_code)]
#[path = "../../benchmarks/perceptual.rs"]
mod perceptual;

#[test]
fn unsupported_vmaf_shapes_are_explicitly_unavailable() {
    for (w, h) in [(33, 256), (64, 64), (214, 214), (215, 256)] {
        // Admission must return before allocating or reading any pixels.
        assert!(perceptual::vmaf_still(&[], &[], w, h).unwrap().is_none());
    }
}

#[test]
fn vmaf_cambi_boundary_is_supported() {
    let (w, h) = (216, 216);
    let original: Vec<u8> = (0..w * h)
        .flat_map(|i| [((i + i / w) % 256) as u8; 3])
        .collect();
    assert!(
        perceptual::vmaf_still(&original, &original, w, h)
            .unwrap()
            .unwrap()
            .is_finite()
    );
}
