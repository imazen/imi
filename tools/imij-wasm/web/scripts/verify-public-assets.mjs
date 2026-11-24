// Independently fetch the deployed graph and compare every packaged byte hash.
import {readFileSync,writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
const [manifest,base,output]=process.argv.slice(2);if(!output)throw Error('usage: verify-public-assets SHA256SUMS HTTPS_BASE FRESH_JSON');
if(new URL(base).protocol!=='https:')throw Error('public HTTPS base required');
const results=[];
for(const line of readFileSync(manifest,'utf8').trim().split('\n')){
 const match=/^([0-9a-f]{64})  ([a-zA-Z0-9_./-]+)$/.exec(line);if(!match||match[2].split('/').includes('..'))throw Error('invalid manifest entry');
 const [,expected,file]=match,response=await fetch(new URL(file,base));if(!response.ok)throw Error(`${file}: HTTP ${response.status}`);
 const bytes=new Uint8Array(await response.arrayBuffer()),sha256=createHash('sha256').update(bytes).digest('hex');if(sha256!==expected)throw Error(`${file}: public asset hash mismatch`);
 results.push({file,bytes:bytes.length,sha256});
}
writeFileSync(output,JSON.stringify(results,null,2),{flag:'wx'});console.log(`${results.length} public assets rehashed PASS`);
