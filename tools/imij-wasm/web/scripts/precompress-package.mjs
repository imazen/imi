// Prepare a fresh served copy; preserve the original compiler/package artifacts.
import { cpSync,readFileSync,writeFileSync,existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { gzipSync,brotliCompressSync,gunzipSync,brotliDecompressSync,constants } from 'node:zlib';
const [input,output]=process.argv.slice(2);
if(!output || existsSync(output))throw Error('usage: precompress-package INPUT_PKG FRESH_OUTPUT_PKG');
cpSync(input,output,{recursive:true,errorOnExist:true,force:false});
const path=resolve(output,'imij_bg.wasm'),raw=readFileSync(path);
const gz=gzipSync(raw,{level:9}),br=brotliCompressSync(raw,{params:{[constants.BROTLI_PARAM_QUALITY]:11}});
if(!gunzipSync(gz).equals(raw)||!brotliDecompressSync(br).equals(raw))throw Error('compressed package roundtrip');
writeFileSync(path+'.gz',gz);writeFileSync(path+'.br',br);
console.log(JSON.stringify({raw:raw.length,gzip:gz.length,brotli:br.length}));
