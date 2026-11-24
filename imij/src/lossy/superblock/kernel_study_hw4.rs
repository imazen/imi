// Included in generated width4: four columns in native NEON lanes.
#[magetypes(define(f32x4),neon,scalar)]
fn hw_tile_impl<const INV:bool,const N:usize,const SUPPORT:usize>(token:Token,input:&[Vec<f32>],stride:usize,output:&mut[[f32;256]]) {
    let k=Kc::new(token);
    for (src,dst) in input.iter().zip(output) {
        for by in 0..16/N {for bx in 0..16/N {
            let base=by*N*16+bx*N;
            if INV && SUPPORT==1 {if N==16 {dst.fill(src[0]);continue;} for y in 0..N {dst[base+y*16..][..N].fill(src[by*N*stride+bx*N]);}continue;}
            let nz=if SUPPORT==16 {N} else {N/4};
            let mut a: [[vf8<_>;16];4] = core::array::from_fn(|oct|core::array::from_fn(|y| {
                if oct<N/4 && y<N && (!INV || oct*4<nz) {
                    f32x4::load_t(token,src[(by*N+y)*stride+bx*N+oct*4..][..4].try_into().unwrap())
                } else {k.z}
            }));
            for oct in 0..N/4 {
                if INV && oct*4>=nz {continue;}
                if INV {if N==16 {pass16(&k,&mut a[oct],SUPPORT);} else if SUPPORT==4 {pass8_2(&k,&mut a[oct][..8]);} else {pass8(&k,&mut a[oct][..8]);}}
                else if N==16 {fwd16(token,&mut a[oct]);}
                else {let mut t=[k.z;8];t.copy_from_slice(&a[oct][..8]);fwd_aan8(token,&mut t);a[oct][..8].copy_from_slice(&t);}
            }
            let quads:[[vf8<_>;4];16]=core::array::from_fn(|i| {
                let (ty,tx)=(i/4,i%4);
                if ty<N/4 && tx<N/4 {
                    let mut t=[a[tx][ty*4],a[tx][ty*4+1],a[tx][ty*4+2],a[tx][ty*4+3]];
                    f32x4::transpose_4x4(&mut t);t
                } else {[k.z;4]}
            });
            let mut b:[[vf8<_>;16];4]=core::array::from_fn(|ty|core::array::from_fn(|x|quads[ty*4+x/4][x%4]));
            for oct in b.iter_mut().take(N/4) {
                if INV {if N==16 {pass16(&k,oct,SUPPORT);} else if SUPPORT==4 {pass8_2(&k,&mut oct[..8]);} else {pass8(&k,&mut oct[..8]);}}
                else if N==16 {fwd16(token,oct);}
                else {let mut t=[k.z;8];t.copy_from_slice(&oct[..8]);fwd_aan8(token,&mut t);oct[..8].copy_from_slice(&t);}
            }
            for ty in 0..N/4 {for tx in 0..N/4 {
                let mut t=[b[ty][tx*4],b[ty][tx*4+1],b[ty][tx*4+2],b[ty][tx*4+3]];
                f32x4::transpose_4x4(&mut t);
                for y in 0..4 {
                    let mut norm=[0.0;4];for x in 0..4 {norm[x]=if N==16 {1.0/16.0} else {0.125/(dct::AAN[ty*4+y]*dct::AAN[tx*4+x])};}
                    let v=if INV {t[y]} else {t[y]*f32x4::from_array_t(token,norm)};
                    v.store((&mut dst[base+(ty*4+y)*16+tx*4..][..4]).try_into().unwrap());
                }
            }}
        }}
    }
}

pub(super) fn hw_tile<const INV:bool,const N:usize,const SUPPORT:usize>(tier:Tier,input:&[Vec<f32>],stride:usize,output:&mut[[f32;256]]) {
    use archmage::SimdToken;
    #[cfg(target_arch="aarch64")]
    if matches!(tier,Tier::Neon) {return hw_tile_impl_neon::<INV,N,SUPPORT>(NeonToken::summon().unwrap(),input,stride,output);}
    hw_tile_impl_scalar::<INV,N,SUPPORT>(ScalarToken,input,stride,output)
}
