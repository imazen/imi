//! Native JPEG component reconstruction. The AAN and color arithmetic are
//! shared with ordinary IMI; the triangle arithmetic follows zenjpeg's f32
//! plane reconstruction. Components are clamped before interpolation.

#[cfg(any(feature = "lab", all(test, feature = "encode")))]
use super::Error;
use super::dct;

pub(crate) fn idct(q: &[i16; 64], table: &[u16; 64]) -> [f32; 64] {
    let factors = dct::dequant_factors(&table.map(f32::from));
    let mut block = core::array::from_fn(|i| f32::from(q[i]) * factors[i]);
    dct::idct8x8(&mut block);
    block
}

/// One output row from two native component rows. Keeping column sums and
/// the order of each f32 operation preserves the measured reconstruction.
pub(crate) fn triangle_row(near: &[f32], far: &[f32], out: &mut [f32]) {
    if near.len() == 1 {
        out.fill((near[0] * 3.0 + far[0]) * 0.25);
        return;
    }
    let col = |x: usize| near[x] * 3.0 + far[x];
    let first = col(0);
    out[0] = first * 0.25;
    if out.len() > 1 {
        out[1] = (first * 3.0 + col(1)) / 16.0;
    }
    let mut previous = first;
    for x in 1..near.len() - 1 {
        let current = col(x);
        if 2 * x < out.len() {
            out[2 * x] = (current * 3.0 + previous) / 16.0;
        }
        if 2 * x + 1 < out.len() {
            out[2 * x + 1] = (current * 3.0 + col(x + 1)) / 16.0;
        }
        previous = current;
    }
    let x = near.len() - 1;
    let current = col(x);
    if 2 * x < out.len() {
        out[2 * x] = (current * 3.0 + previous) / 16.0;
    }
    if 2 * x + 1 < out.len() {
        out[2 * x + 1] = current * 0.25;
    }
}

#[cfg(any(feature = "lab", all(test, feature = "encode")))]
fn fits(len: usize, width: usize, height: usize, stride: usize) -> bool {
    width > 0
        && height > 0
        && stride >= width
        && (height - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(width))
            .is_some_and(|n| n <= len)
}

/// Centered half-both-axes sampling, with whole-plane edge extension. Both
/// buffers support strided rows; the final odd row/column is cropped.
#[cfg(any(feature = "lab", all(test, feature = "encode")))]
pub(crate) fn triangle(
    input: &[f32],
    width: usize,
    height: usize,
    stride: usize,
    output: &mut [f32],
    out_width: usize,
    out_height: usize,
    out_stride: usize,
) -> Result<(), Error> {
    if !fits(input.len(), width, height, stride)
        || !fits(output.len(), out_width, out_height, out_stride)
        || out_width.div_ceil(2) != width
        || out_height.div_ceil(2) != height
    {
        return Err(Error::InvalidInput("native component triangle geometry"));
    }
    for y in 0..out_height {
        let near = y / 2;
        let far = if y % 2 == 0 {
            near.saturating_sub(1)
        } else {
            (near + 1).min(height - 1)
        };
        triangle_row(
            &input[near * stride..near * stride + width],
            &input[far * stride..far * stride + width],
            &mut output[y * out_stride..y * out_stride + out_width],
        );
    }
    Ok(())
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    #[test]
    fn shared_aan_preserves_native_dc_step() {
        let mut q = [0i16; 64];
        q[0] = -8;
        let table = [17u16; 64];
        assert_eq!(idct(&q, &table), [-17.0; 64]);
    }

    #[test]
    fn whole_plane_triangle_crosses_native_block_boundaries() {
        // At x=15/16, reconstruction reads both sides of the native 8x8
        // boundary, rather than independently extending the two blocks.
        let mut input = [0.0; 18];
        input[8] = 100.0;
        input[17] = 100.0;
        let mut output = [0.0; 72];
        triangle(&input, 9, 2, 9, &mut output, 18, 4, 18).unwrap();
        assert_eq!(output[15], 25.0);
        assert_eq!(output[16], 75.0);
        assert_eq!(output[17], 100.0);
    }

    #[test]
    fn triangle_validates_before_writing() {
        let mut output = [42.0; 8];
        assert!(triangle(&[0.0; 4], 2, 2, usize::MAX, &mut output, 4, 2, 4).is_err());
        assert_eq!(output, [42.0; 8]);
        assert!(triangle(&[], 0, 0, 0, &mut output, 4, 2, 4).is_err());
    }
}
