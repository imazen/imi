// Static compiled-code evidence; operation counts are not dynamic instruction costs.
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import {createHash} from 'node:crypto';
import {pathToFileURL} from 'node:url';
const [web,input,output,pattern='decode_row']=process.argv.slice(2);
if(!output)throw Error('usage: inspect-wasm-functions WEB NAMED_WASM FRESH_OUTPUT [REGEX]');
mkdirSync(output,{recursive:false});
const {default:binaryen}=await import(pathToFileURL(resolve(web,'node_modules/binaryen/index.js')).href);
const bytes=readFileSync(input),module=binaryen.readBinary(bytes),matching=new RegExp(pattern),functions=[];
try{
 for(let index=0;index<module.getNumFunctions();index++){
  const info=binaryen.getFunctionInfo(module.getFunctionByIndex(index));
  if(!info.body||!matching.test(info.name))continue;
  const text=binaryen.emitText(info.body),operations={};
  for(const [,op] of text.matchAll(/\(([a-z][a-z0-9_]*(?:\.[a-z0-9_]+)+)\b/g))operations[op]=(operations[op]||0)+1;
  const file=`function-${index}.wat`;writeFileSync(resolve(output,file),text,{flag:'wx'});
  functions.push({index,name:info.name,locals:info.vars.length,file,sha256:createHash('sha256').update(text).digest('hex'),operations});
 }
 if(!functions.length)throw Error('no matching compiled functions');
 writeFileSync(resolve(output,'results.json'),JSON.stringify({input,sha256:createHash('sha256').update(bytes).digest('hex'),pattern,functions},null,2),{flag:'wx'});
 console.log(`${functions.length} compiled function bodies retained`);
}finally{module.dispose();}
