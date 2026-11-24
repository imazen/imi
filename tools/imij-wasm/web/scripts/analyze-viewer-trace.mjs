import {readFileSync,writeFileSync} from 'node:fs';
import {analyzeTrace,captureFromTrace,cameraCaptureFromTrace,cameraPatterns} from './viewer-trace-analysis.mjs';
const args=process.argv.slice(2),camera=args[0]==='--camera-demand';if(camera)args.shift();
const [input,catalogFile,url,output]=args;
if(!input)throw Error('usage: analyze-viewer-trace TRACE_JSON [CATALOG_JSON PUBLIC_BASE FRESH_CAPTURE_JSON]');
const trace=JSON.parse(readFileSync(input));console.log(JSON.stringify(analyzeTrace(trace),null,2));
if(catalogFile){
  if(!url||!output)throw Error('catalog conversion needs base URL and fresh output');
  const capture=(camera?cameraCaptureFromTrace:captureFromTrace)(trace,JSON.parse(readFileSync(catalogFile)),url);
  console.log(JSON.stringify({importMode:camera?'camera-demand':'complete',patterns:cameraPatterns(capture)},null,2));
  writeFileSync(output,JSON.stringify(capture),{flag:'wx'});
}
