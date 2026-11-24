import numpy as np
f32=np.float32
lo,hi=np.sqrt(0.5)-1,np.sqrt(2)-1
x=np.linspace(lo,hi,400001); x=x[x!=0]
QL=np.polynomial.chebyshev.Chebyshev.fit(x,np.log2(1+x)/x,8).convert(kind=np.polynomial.Polynomial).coef.astype(f32)
f=np.linspace(0,1,400001)
PE=np.polynomial.chebyshev.Chebyshev.fit(f,2**f,6).convert(kind=np.polynomial.Polynomial).coef.astype(f32)
def horner(c,x):
    r=np.full_like(x,c[-1])
    for a in c[-2::-1]:
        r=(r*x).astype(f32)+a
    return r.astype(f32)
def log2f(v):
    v=v.astype(f32); b=v.view(np.int32)
    e=((b>>23)&0xFF)-127
    m=((b&0x7FFFFF)|0x3F800000).view(f32)
    big=m>f32(1.41421356)
    m=np.where(big,(m*f32(0.5)).astype(f32),m); e=np.where(big,e+1,e)
    t=(m-f32(1)).astype(f32)
    return (e.astype(f32)+(horner(QL,t)*t).astype(f32)).astype(f32)
def exp2f(y):
    y=np.maximum(y.astype(f32),f32(-126)); k=np.floor(y).astype(f32); fr=(y-k).astype(f32)
    s=((k.astype(np.int32)+127)<<23).view(f32)
    return (horner(PE,fr)*s).astype(f32)
def powf(v,a): return exp2f((log2f(v)*f32(a)).astype(f32))
# sRGB encode
L=np.concatenate([np.linspace(0,1,2000001),np.geomspace(1e-7,1,200001)]).astype(f32)
def srgb_exact(L): L=L.astype(np.float64); return np.where(L<=0.0031308,L*12.92,1.055*L**(1/2.4)-0.055)
srgb=np.where(L<=f32(0.0031308),(L*f32(12.92)).astype(f32),(f32(1.055)*powf(np.maximum(L,f32(1e-30)),1/2.4)-f32(0.055)).astype(f32))
print('srgb max abs err (16-bit codes):',np.abs(srgb-srgb_exact(L)).max()*65535)
# PQ
m1,m2,c1,c2,c3=0.1593017578125,78.84375,0.8359375,18.8515625,18.6875
W=10000/49.25
Lp=np.concatenate([np.linspace(0,49.25,2000001),np.geomspace(1e-9,49.25,200001)]).astype(f32)
def pq_exact(L):
    y=np.clip(L.astype(np.float64)*W/10000,0,1); yp=y**m1
    return ((c1+c2*yp)/(1+c3*yp))**m2
Y=(Lp*f32(W/10000)).astype(f32)
p=powf(np.maximum(Y,f32(1e-30)),m1)
r=((f32(c1)+(f32(c2)*p).astype(f32)).astype(f32)/(f32(1)+(f32(c3)*p).astype(f32)).astype(f32)).astype(f32)
pq=powf(r,m2)
e=np.abs(pq-pq_exact(Lp))*65535
print('pq max err (16-bit codes):',e.max(),'at L=',Lp[np.argmax(e)])
