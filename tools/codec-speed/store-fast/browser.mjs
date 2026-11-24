// Browser WASI deployment of the exact standalone experiment; not shipped IMIJ.
import { spawn } from 'node:child_process';
import { readFileSync, writeFileSync, appendFileSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createHash } from 'node:crypto';
const [webArg, fixturesArg, nativeArg, wasmUrl, outputArg] = process.argv.slice(2);
const web = resolve(webArg), fixtures = resolve(fixturesArg), native = resolve(nativeArg), output = resolve(outputArg);
mkdirSync(output);
const { chromium } = await import(pathToFileURL(join(web, 'node_modules/playwright/index.mjs')));
const manifest = JSON.parse(readFileSync(join(fixtures, 'manifest.json')));
const prepared = JSON.parse(readFileSync(join(native, 'prepared.json')));
const hash = b => createHash('sha256').update(b).digest('hex');
const port = 3837;
const server = spawn(process.execPath, ['scripts/serve.mjs', 'isolated', String(port)], { cwd: web, env: { ...process.env, IMIJ_BENCH_FIXTURES: fixtures } });
server.stderr.pipe(process.stderr);
let browser;
try {
  await new Promise((yes, no) => {server.stdout.once('data', yes); server.once('error', no);server.once('exit', rc => no(Error(`server rc ${rc}`)));});
  browser = await chromium.launch();
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${port}/package.json`);
  const admission = await page.evaluate(async url => {
    window.shim = await import('/node_modules/@bjorn3/browser_wasi_shim/dist/index.js');
    const b = new Uint8Array(await (await fetch(url)).arrayBuffer());
    window.module = await WebAssembly.compile(b);
    return {sha256: [...new Uint8Array(await crypto.subtle.digest('SHA-256', b))].map(v => v.toString(16).padStart(2,'0')).join('')};
  }, wasmUrl);
  writeFileSync(join(output, 'admission.json'), JSON.stringify({...admission, browser: browser.version(), nativePreparedSha: hash(readFileSync(join(native, 'prepared.json')))}, null, 2));
  const rows=[];
  for (const p of prepared) {
    const f = manifest.find(f => f.name === p.image);
    const measured = await page.evaluate(async ({f,p}) => {
      const {WASI, File, OpenFile, PreopenDirectory} = window.shim;
      const raw = new Uint8Array(await (await fetch(`/bench/${f.raw}`)).arrayBuffer());
      const sha = async bytes => [...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(v => v.toString(16).padStart(2,'0')).join('');
      if (await sha(raw) !== f.sha256) throw Error('raw hash');
      const results=[];
      for (let rep=0;rep<3;rep++) {
        const encoded = new File(new Uint8Array()), stdout = new File(new Uint8Array()), stderr = new File(new Uint8Array());
        const files = new Map([['input',new File(raw)],['output',encoded]]);
        const args=['probe','/input',String(f.width),String(f.height),String({gray:1,ga:2,rgb:3,rgba:4}[f.channels]),String({u8:1,u16:2,f32:4}[f.sample]),String(p.mode),'/output'];
        const wasi = new WASI(args,[],[new OpenFile(new File(new Uint8Array())),new OpenFile(stdout),new OpenFile(stderr),new PreopenDirectory('/',files)]);
        const instance = await WebAssembly.instantiate(window.module, {wasi_snapshot_preview1:wasi.wasiImport});
        const rc=wasi.start(instance);
        const text=new TextDecoder().decode(stdout.data), errors=new TextDecoder().decode(stderr.data);
        if(rc!==0) throw Error(`WASI rc ${rc}: ${errors}`);
        const info=JSON.parse(text.trim().split('\n').find(l=>l.startsWith('{"encode_ns"')));
        if (![info.encode_ns,info.decode_ns].every(n=>Number.isFinite(n)&&n>0)) throw Error('invalid timing');
        if(await sha(encoded.data)!==p.sha256) throw Error('native/WASM encoded mismatch');
        results.push(info);
      }
      return results;
    },{f,p});
    const row={image:p.image,mode:p.mode,width:f.width,height:f.height,bytes:p.bytes,results:measured};
    rows.push(row);appendFileSync(join(output,'rows.jsonl'),JSON.stringify(row)+'\n');console.log(JSON.stringify(row));
  }
  writeFileSync(join(output,'complete.json'),JSON.stringify({rows:rows.length,observations:rows.length*3}));
} finally {if(browser) await browser.close();server.kill('SIGTERM');}
