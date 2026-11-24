//! Row-buffer sample storage: u16 (baseline) or u32 (extended tier),
//! with shared safe storage views for scalar and SIMD kernels.

use alloc::vec::Vec;

pub(crate) trait Px: Copy + Default + Send + Sync + 'static {
    fn get(self) -> u32;
    fn put(v: u32) -> Self;
    /// Zero-cost u16 views for the SIMD kernels (None for u32 storage).
    fn rows16(v: &[Vec<Self>]) -> Option<&[Vec<u16>]>;
    fn rows16_mut(v: &mut [Vec<Self>]) -> Option<&mut [Vec<u16>]>;
    // The slice views serve the SIMD row kernels, which exist only on some
    // targets (none on i686 or no_std builds).
    #[allow(dead_code)]
    fn slice16(v: &[Self]) -> Option<&[u16]>;
    #[allow(dead_code)]
    fn slice16_mut(v: &mut [Self]) -> Option<&mut [u16]>;
    /// u32 views for the wide (extended-tier) kernels.
    fn rows32(v: &[Vec<Self>]) -> Option<&[Vec<u32>]>;
    fn rows32_mut(v: &mut [Vec<Self>]) -> Option<&mut [Vec<u32>]>;
    #[allow(dead_code)]
    fn slice32(v: &[Self]) -> Option<&[u32]>;
    #[allow(dead_code)]
    fn slice32_mut(v: &mut [Self]) -> Option<&mut [u32]>;
}

impl Px for u16 {
    #[inline(always)]
    fn get(self) -> u32 {
        self as u32
    }
    #[inline(always)]
    fn put(v: u32) -> Self {
        v as u16
    }
    fn rows16(v: &[Vec<Self>]) -> Option<&[Vec<u16>]> {
        Some(v)
    }
    fn rows16_mut(v: &mut [Vec<Self>]) -> Option<&mut [Vec<u16>]> {
        Some(v)
    }
    fn slice16(v: &[Self]) -> Option<&[u16]> {
        Some(v)
    }
    fn slice16_mut(v: &mut [Self]) -> Option<&mut [u16]> {
        Some(v)
    }
    fn rows32(_: &[Vec<Self>]) -> Option<&[Vec<u32>]> {
        None
    }
    fn rows32_mut(_: &mut [Vec<Self>]) -> Option<&mut [Vec<u32>]> {
        None
    }
    fn slice32(_: &[Self]) -> Option<&[u32]> {
        None
    }
    fn slice32_mut(_: &mut [Self]) -> Option<&mut [u32]> {
        None
    }
}

impl Px for u32 {
    #[inline(always)]
    fn get(self) -> u32 {
        self
    }
    #[inline(always)]
    fn put(v: u32) -> Self {
        v
    }
    fn rows16(_: &[Vec<Self>]) -> Option<&[Vec<u16>]> {
        None
    }
    fn rows16_mut(_: &mut [Vec<Self>]) -> Option<&mut [Vec<u16>]> {
        None
    }
    fn slice16(_: &[Self]) -> Option<&[u16]> {
        None
    }
    fn slice16_mut(_: &mut [Self]) -> Option<&mut [u16]> {
        None
    }
    fn rows32(v: &[Vec<Self>]) -> Option<&[Vec<u32>]> {
        Some(v)
    }
    fn rows32_mut(v: &mut [Vec<Self>]) -> Option<&mut [Vec<u32>]> {
        Some(v)
    }
    fn slice32(v: &[Self]) -> Option<&[u32]> {
        Some(v)
    }
    fn slice32_mut(v: &mut [Self]) -> Option<&mut [u32]> {
        Some(v)
    }
}
