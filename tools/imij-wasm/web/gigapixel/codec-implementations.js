// Source-based defaults for explicitly reviewed browser majors, not binary attestation.
const chrome='https://github.com/chromium/chromium/blob/153.0.8010.12/';
const firefox='https://github.com/mozilla-firefox/firefox/blob/FIREFOX_155_0_RELEASE/';
export const decoderMappings=[
 {browser:'Chromium',major:153,codec:'jpeg',library:'libjpeg-turbo',kind:'C + assembly kernels',source:chrome+'third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_image_decoder.cc'},
 {browser:'Chromium',major:153,codec:'png',library:'Skia Rust png',kind:'Rust',source:chrome+'third_party/blink/renderer/platform/image-decoders/png/png_image_decoder.cc'},
 {browser:'Chromium',major:153,codec:'avif',library:'crabbyavif + dav1d',kind:'Rust/C + assembly kernels',source:chrome+'third_party/blink/renderer/platform/image-decoders/avif/avif_image_decoder.cc'},
 {browser:'Chromium',major:153,codec:'jxl',library:'jxl-rs 0.5',kind:'Rust',source:chrome+'third_party/blink/renderer/platform/image-decoders/jxl/jxl_image_decoder.cc'},
 {browser:'Chromium',major:153,codec:'webp',library:'libwebp 1.6',kind:'C + SIMD kernels',source:chrome+'third_party/libwebp/README.chromium'},
 {browser:'Firefox',major:155,codec:'jpeg',library:'libjpeg-turbo 3.1.4.1',kind:'C + assembly kernels',source:firefox+'media/libjpeg/moz.yaml'},
 {browser:'Firefox',major:155,codec:'png',library:'libpng',kind:'C',source:firefox+'image/decoders/nsPNGDecoder.cpp'},
 {browser:'Firefox',major:155,codec:'avif',library:'mp4parse + dav1d (or libaom)',kind:'Rust/C + assembly kernels',source:firefox+'image/decoders/nsAVIFDecoder.cpp'},
 {browser:'Firefox',major:155,codec:'jxl',library:'jxl-rs 0.5.1',kind:'Rust',source:firefox+'image/rust/jxl/Cargo.toml'},
 {browser:'Firefox',major:155,codec:'webp',library:'libwebp 1.6',kind:'C + SIMD kernels',source:firefox+'media/libwebp/moz.yaml'},
 {browser:'Safari/WebKit',major:null,codec:'all',library:'OS ImageIO or port-selected libraries',kind:'implementation unverified',source:'https://github.com/WebKit/WebKit/blob/main/Source/WebCore/platform/graphics/cg/ImageDecoderCG.cpp'}
];
export function browserIdentity(ua=globalThis.navigator?.userAgent||''){
 const ff=/Firefox\/(\d+)/.exec(ua);if(ff)return {browser:'Firefox',major:Number(ff[1])};
 const ch=/(?:Chrome|Chromium)\/(\d+)/.exec(ua);if(ch)return {browser:'Chromium',major:Number(ch[1])};
 if(/AppleWebKit\//.test(ua))return {browser:'Safari/WebKit',major:Number(/Version\/(\d+)/.exec(ua)?.[1])||null};
 return {browser:'Unknown',major:null};
}
export function implementationLabel(fixture,imi,ua){
 if(fixture.codec.startsWith('imi-')){
  if(!imi)return '(WebAssembly, decoder loading)';
  if(imi.encodedBytes>0)return `(WebAssembly, ${(imi.encodedBytes/1024).toFixed(0)} KiB decoder ${imi.transferBytes===0?'payload · cached':'downloaded'})`;
  return `(WebAssembly, ${(imi.rawBytes/1024).toFixed(0)} KiB decoder · uncompressed)`;
 }
 const b=browserIdentity(ua),codec=fixture.codec.split('-')[0];
 const row=decoderMappings.find(m=>m.browser===b.browser&&m.major===b.major&&m.codec===codec);
 return `(browser-native, ${row?.kind||'implementation unverified'})`;
}
export function renderDecoderTable(container){
 const b=browserIdentity();const identity=document.createElement('p');identity.textContent=`Declared browser: ${b.browser} ${b.major??''}. Labels below use documented source defaults, not inspection of loaded machine code. Unmapped versions, browser forks, build flags and OS ports can differ. Assembly labels refer to optimized kernels, not entire decoders.`;container.replaceChildren(identity);
 const table=document.createElement('table');table.innerHTML='<thead><tr><th>Browser version</th><th>Format</th><th>Bundled implementation</th><th>Code</th><th>Evidence</th></tr></thead>';
 const body=table.appendChild(document.createElement('tbody'));
 for(const m of decoderMappings){const tr=body.appendChild(document.createElement('tr'));for(const text of [`${m.browser} ${m.major??'(port-dependent)'}`,m.codec,m.library,m.kind]){const td=tr.appendChild(document.createElement('td'));td.textContent=text;}const link=document.createElement('a');link.href=m.source;link.textContent='Source';link.target='_blank';link.rel='noreferrer';tr.appendChild(document.createElement('td')).appendChild(link);}
 container.appendChild(table);
}
