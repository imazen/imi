// Run the exact Rust unit suite in a browser's 32-bit WASM engine.
import { spawn } from 'node:child_process';
import { writeFileSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
const [webArg, wasmUrl, outputArg] = process.argv.slice(2);
const web=resolve(webArg), output=resolve(outputArg);
mkdirSync(output);
const { chromium } = await import(pathToFileURL(join(web,'node_modules/playwright/index.mjs')));
const server=spawn(process.execPath,['scripts/serve.mjs','isolated','3838'],{cwd:web});
server.stderr.pipe(process.stderr);
let browser;
try {
  await new Promise((ok,fail)=>{server.stdout.once('data',ok);server.once('error',fail);server.once('exit',rc=>fail(Error(`server ${rc}`)));});
  browser=await chromium.launch();
  const page=await browser.newPage();
  await page.goto('http://127.0.0.1:3838/package.json');
  const result=await page.evaluate(async url=>{
    const {WASI,File,OpenFile,PreopenDirectory}=await import('/node_modules/@bjorn3/browser_wasi_shim/dist/index.js');
    const bytes=new Uint8Array(await (await fetch(url)).arrayBuffer());
    const sha256=[...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(x=>x.toString(16).padStart(2,'0')).join('');
    const stdout=new File(new Uint8Array()),stderr=new File(new Uint8Array());
    const wasi=new WASI(['quick-tests','--test-threads=1','--nocapture'],[],[new OpenFile(new File(new Uint8Array())),new OpenFile(stdout),new OpenFile(stderr),new PreopenDirectory('/',new Map())]);
    const {instance}=await WebAssembly.instantiate(bytes,{wasi_snapshot_preview1:wasi.wasiImport});
    const rc=wasi.start(instance);
    return {sha256,rc,stdout:new TextDecoder().decode(stdout.data),stderr:new TextDecoder().decode(stderr.data)};
  },wasmUrl);
  writeFileSync(join(output,'results.json'),JSON.stringify({...result,browser:browser.version()},null,2));
  if(result.rc!==0 || !result.stdout.includes('5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out')) throw Error('Rust browser tests failed');
  console.log(result.stdout);
} finally {if(browser) await browser.close();server.kill('SIGTERM');}
