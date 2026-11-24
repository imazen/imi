// Capture bytes requested by the existing checked reader while opening its index.
// Output is a transport cache seed, never a replacement index or a codec format.
import { mkdirSync,writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';
const [web,base,output]=process.argv.slice(2);
if(!output)throw Error('usage: capture-range-bootstrap WEB BASE FRESH_OUTPUT');
mkdirSync(output,{recursive:false});
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch();
try{
  const page=await browser.newPage();await page.goto(base);
  const result=await page.evaluate(async()=>{
    const mod=await import('/dist/pkg-simd/imij.js');await mod.default();
    const {HttpRangeSource}=await import('/gigapixel/range-source.js');
    const manifest=await(await fetch('/image/image.json')).json();
    const source=await HttpRangeSource.open(`/image/${manifest.file}`);
    const image=await source.retry(()=>new mod.RangeImage(source.length,source.read));
    if(!image.info().indexed)throw Error('checked index missing');image.free();
    return {length:source.length,sha256:manifest.sha256,blocks:source.blocks.map(b=>({offset:b.start,bytes:Array.from(b.bytes)})),stats:source.stats()};
  });
  const header=Buffer.alloc(52);header.write('IMIRNG01');header.writeBigUInt64LE(BigInt(result.length),8);
  Buffer.from(result.sha256,'hex').copy(header,16);header.writeUInt32LE(result.blocks.length,48);
  const parts=[header];
  for(const b of result.blocks){const h=Buffer.alloc(12);h.writeBigUInt64LE(BigInt(b.offset));h.writeUInt32LE(b.bytes.length,8);parts.push(h,Buffer.from(b.bytes));}
  const bytes=Buffer.concat(parts);writeFileSync(resolve(output,'index-cache.bin'),bytes);
  writeFileSync(resolve(output,'summary.json'),JSON.stringify({imageSha256:result.sha256,encodedBytes:result.length,blocks:result.blocks.length,bytes:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex'),stats:result.stats},null,2)+'\n');
  console.log(`${result.blocks.length} checked opening spans; ${bytes.length} cache-seed bytes`);
}finally{await browser.close();}
