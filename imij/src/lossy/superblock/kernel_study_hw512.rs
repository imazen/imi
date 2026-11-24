// Included in generated width16: native 16-column tiles, no AoS batch packing.
// Native 16x16 transpose: cheap within-128-bit unpacks/shuffles first,
// then two fixed cross-lane shuffle stages. No variable-index permute tables.
#[rite(import_intrinsics)]
fn transpose16(token:X64V4Token,v:&mut[vf8<X64V4Token>;16]) {
    let z=_mm512_setzero_ps();
    let mut t=[z;16];let mut s=[z;16];
    for pair in 0..8 {
        let i=pair*2;let a=v[i].into_repr();let b=v[i+1].into_repr();
        t[i]=_mm512_unpacklo_ps(a,b);t[i+1]=_mm512_unpackhi_ps(a,b);
    }
    for group in 0..4 {
        let i=group*4;
        s[i]=_mm512_shuffle_ps::<0x44>(t[i],t[i+2]);
        s[i+1]=_mm512_shuffle_ps::<0xee>(t[i],t[i+2]);
        s[i+2]=_mm512_shuffle_ps::<0x44>(t[i+1],t[i+3]);
        s[i+3]=_mm512_shuffle_ps::<0xee>(t[i+1],t[i+3]);
    }
    for j in 0..4 {
        let a=_mm512_shuffle_f32x4::<0x88>(s[j],s[j+4]);
        let b=_mm512_shuffle_f32x4::<0xdd>(s[j],s[j+4]);
        let c=_mm512_shuffle_f32x4::<0x88>(s[j+8],s[j+12]);
        let d=_mm512_shuffle_f32x4::<0xdd>(s[j+8],s[j+12]);
        v[j]=vf8::from_repr_t(token,_mm512_shuffle_f32x4::<0x88>(a,c));
        v[j+8]=vf8::from_repr_t(token,_mm512_shuffle_f32x4::<0xdd>(a,c));
        v[j+4]=vf8::from_repr_t(token,_mm512_shuffle_f32x4::<0x88>(b,d));
        v[j+12]=vf8::from_repr_t(token,_mm512_shuffle_f32x4::<0xdd>(b,d));
    }
}

#[rite]
fn transpose_pair(token:X64V4Token,v:&mut[vf8<X64V4Token>;8]) {
    let mut raw=core::array::from_fn(|i|v[i].into_repr());
    crate::lossy::recon_v4::study_transpose2(token,&mut raw);
    for i in 0..8 {v[i]=vf8::from_repr_t(token,raw[i]);}
}

#[arcane]
pub(super) fn hw_tile<const INV:bool,const N:usize,const SUPPORT:usize>(token:X64V4Token,input:&[Vec<f32>],stride:usize,output:&mut[[f32;256]]) {
    let k=Kc::new(token);
    for (src,dst) in input.iter().zip(output) {
        if INV && SUPPORT==1 {
            if N==16 {dst.fill(src[0]);continue;}
            for by in 0..16/N {for bx in 0..16/N {
                for y in 0..N {dst[(by*N+y)*16+bx*N..][..N].fill(src[by*N*stride+bx*N]);}
            }}
        } else if N==16 {
            let mut v=[k.z;16];
            for y in 0..16 {v[y]=vf8::load_t(token,src[y*stride..][..16].try_into().unwrap());}
            if INV {pass16(&k,&mut v,SUPPORT);} else {fwd16(token,&mut v);}
            transpose16(token,&mut v);
            if INV {pass16(&k,&mut v,SUPPORT);} else {fwd16(token,&mut v);}
            transpose16(token,&mut v);
            for y in 0..16 {
                let v=if INV {v[y]} else {v[y]*vf8::splat_t(token,1.0/16.0)};
                v.store((&mut dst[y*16..][..16]).try_into().unwrap());
            }
        } else {
            for by in 0..2 {
                let mut v=[k.z;8];
                for y in 0..8 {v[y]=vf8::load_t(token,src[(by*8+y)*stride..][..16].try_into().unwrap());}
                if INV {if SUPPORT==4 {pass8_2(&k,&mut v);} else {pass8(&k,&mut v);}}
                else {fwd_aan8(token,&mut v);}
                transpose_pair(token,&mut v);
                if INV {if SUPPORT==4 {pass8_2(&k,&mut v);} else {pass8(&k,&mut v);}}
                else {fwd_aan8(token,&mut v);}
                transpose_pair(token,&mut v);
                for y in 0..8 {
                    let mut norm=[0.0;16];for x in 0..16 {norm[x]=0.125/(dct::AAN[y]*dct::AAN[x%8]);}
                    let v=if INV {v[y]} else {v[y]*vf8::from_array_t(token,norm)};
                    v.store((&mut dst[(by*8+y)*16..][..16]).try_into().unwrap());
                }
            }
        }
    }
}
