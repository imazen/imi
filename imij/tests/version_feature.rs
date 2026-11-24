use imij::Error;
use imij::planes::{Rect, SliceReader};
use imij::region::RegionReader;

// The same 1×1 RGB8 pixel, encoded with Quick in both grammars.
// Keep these tiny witnesses in source so the decoder-only tests need no encoder.
const V3: &[u8] = &[
    73, 77, 73, 74, 13, 10, 135, 10, 8, 3, 4, 128, 128, 128, 2, 1, 1, 68, 65, 84, 65, 11, 8, 1, 1,
    0, 59, 4, 0, 42, 75, 190,
];
const V4: &[u8] = &[
    73, 77, 73, 74, 13, 10, 135, 10, 8, 4, 4, 128, 128, 128, 2, 1, 1, 68, 65, 84, 65, 97, 8, 1, 64,
    0, 1, 8, 1, 1, 0, 2, 22, 21, 3, 2, 0, 0, 19, 0, 0, 0, 23, 3, 0, 0, 21, 63, 0, 0, 0, 0, 0, 0, 0,
    65, 17, 0, 199, 165, 110, 142, 172, 221, 61, 28, 201, 151, 82, 67, 237, 5, 125, 47, 59, 21, 0,
    0, 42, 75, 190, 16, 126, 234, 57, 244, 198, 71, 243, 230, 148, 249, 242, 176, 2, 161, 8, 67,
    19, 0, 42, 1, 27, 243, 36, 90, 174, 216, 190, 197, 45, 204, 122, 56, 106, 77, 62, 55,
];

#[test]
fn v3_feature_controls_v2_and_v3_image_readers() {
    let v2 = include_bytes!("../../fuzz/seeds/31-seed-v2-unit-hint.imi").as_slice();
    for (version, data) in [(2, v2), (3, V3), (4, V4)] {
        let header = imij::header(data);
        let decoded = imij::decode(data);
        let region = RegionReader::open(SliceReader::new(data));
        let stream = imij::StreamDecoder::new(data);
        if version == 4 || cfg!(feature = "v3") {
            assert_eq!(header.unwrap().version, version);
            let (info, decoded) = decoded.unwrap();
            if version >= 3 {
                assert_eq!(decoded, [42, 75, 190]);
            }
            let stride = info.row_bytes();
            let mut cropped = vec![0; decoded.len()];
            region
                .unwrap()
                .decode_region(
                    Rect::new(0, 0, info.width, info.height),
                    &mut cropped,
                    stride,
                )
                .unwrap();
            assert_eq!(cropped, decoded);
            let mut stream = stream.unwrap();
            for (row, expected) in cropped
                .chunks_exact_mut(stride)
                .zip(decoded.chunks_exact(stride))
            {
                row.fill(0);
                assert!(stream.next_row(row).unwrap());
                assert_eq!(row, expected);
            }
        } else {
            assert!(matches!(header, Err(Error::Unsupported(_))));
            assert!(matches!(decoded, Err(Error::Unsupported(_))));
            assert!(matches!(region, Err(Error::Unsupported(_))));
            assert!(matches!(stream, Err(Error::Unsupported(_))));
        }
    }
}
