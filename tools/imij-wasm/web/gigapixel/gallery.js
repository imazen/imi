// Native previews and selection remain usable before WASM is ready.
const fold=value=>String(value||'').normalize('NFD').replace(/\p{Diacritic}/gu,'').toLocaleLowerCase();
export function gallerySelection(scenes,{query='',source='',sort='featured'}={}){
 const words=fold(query).trim().split(/\s+/).filter(Boolean);
 const found=scenes.filter(scene=>(!source||new URL(scene.sourceUrl).hostname===source)&&words.every(word=>fold([scene.title,scene.credit,...(scene.tags||[]),scene.collection].join(' ')).includes(word)));
 if(sort==='largest')found.sort((a,b)=>b.width*b.height-a.width*a.height);
 if(sort==='title')found.sort((a,b)=>a.title.localeCompare(b.title));
 return found;
}
export async function installGallery(url,select,isReady){
 const $=id=>document.getElementById(id),started=performance.now();
 const items=$('gallery-items'),dialog=$('gallery-dialog'),popover=$('gallery-popover'),toggle=$('gallery-toggle');
 let showFeatured;
 const setPopoverOpen=open=>{
  if(!popover||!toggle)return;
  popover.hidden=!open;
  toggle.setAttribute('aria-expanded',String(open));
  if(open)showFeatured?.();
 };
 if(toggle)toggle.onclick=e=>{e.stopPropagation();setPopoverOpen(popover.hidden);};
 const response=await fetch(url);
 if(!response.ok)throw Error(`HTTP ${response.status}`);
 const data=await response.json();
 if(!Array.isArray(data.datasets)||!data.datasets.length)throw Error('empty image gallery');
 let selected=null,page=0;const pageSize=24;
 const choose=scene=>{setSelected(scene.id);select(scene.id);dialog.close();setPopoverOpen(false);$('gallery-status').textContent=isReady()?'Choose another image anytime':'Selected · decoder still loading';};
 function card(scene){
  const button=document.createElement('button');button.type='button';button.className='gallery-card';button.dataset.scene=scene.id;
  button.setAttribute('aria-pressed',String(selected===scene.id));
  if(scene.thumbnail){const img=document.createElement('img');img.loading='lazy';img.decoding='async';img.alt=scene.thumbnail.alt||scene.title;img.src=scene.thumbnail.url;button.append(img);}
  const label=document.createElement('span'),title=document.createElement('strong'),size=document.createElement('small');
  title.textContent=scene.title.replace(/ · [\d.]+ gigapixels$/,'');const count=scene.width*scene.height;
  size.textContent=`${count>=1e9?(count/1e9).toFixed(2)+' gigapixels':(count/1e6).toFixed(1)+' megapixels'} · ${scene.credit}`;
  label.append(title,size);
  if(scene.license){const license=document.createElement('small');license.textContent=scene.license.name||scene.license;label.append(license);}
  button.append(label);button.onclick=()=>choose(scene);return button;
 }
 showFeatured=()=>{
  // Build the row on first open. Reuse the cards across scene changes so
  // selection keeps focus and does not recreate the thumbnail elements.
  if(!items.childElementCount)items.replaceChildren(...data.datasets.map(card));
  items.querySelector('[aria-pressed=true]')?.scrollIntoView({block:'nearest',inline:'nearest'});
 };
 function render(reset=false){
  if(reset)page=0;
  const found=gallerySelection(data.datasets,{query:$('gallery-search').value,source:$('gallery-source').value,sort:$('gallery-sort').value});
  const pages=Math.max(1,Math.ceil(found.length/pageSize));page=Math.min(page,pages-1);
  $('gallery-results').replaceChildren(...found.slice(page*pageSize,(page+1)*pageSize).map(card));
  $('gallery-count').textContent=found.length?`${found.length} image${found.length===1?'':'s'} · ${page*pageSize+1}–${Math.min(found.length,(page+1)*pageSize)} shown`:'No images match. Try another search or source.';
  $('gallery-page').textContent=`${page+1} / ${pages}`;$('gallery-previous').disabled=page===0;$('gallery-next').disabled=page+1>=pages;
 }
 function setSelected(id){
  selected=id;
  for(const item of document.querySelectorAll('.gallery-card'))
   item.setAttribute('aria-pressed',String(item.dataset.scene===id));
  const current=data.datasets.find(s=>s.id===id);
  if(toggle&&current){
   const base=current.title.replace(/ · [\d.]+ (giga|mega)pixels$/i,'');
   const count=current.width*current.height,size=count>=1e9?`${(count/1e9).toFixed(1)} GP`:`${Math.round(count/1e6)} MP`;
   toggle.textContent=`${base} · ${size} ▾`;
  }
 }
 for(const source of [...new Set(data.datasets.map(s=>new URL(s.sourceUrl).hostname))].sort()){
  const option=document.createElement('option');option.value=source;option.textContent=source.replace(/^www\./,'');$('gallery-source').append(option);
 }
 $('gallery-browse').disabled=false;$('gallery-browse').textContent=`Browse all ${data.datasets.length} images`;
 $('gallery-browse').onclick=()=>{render();dialog.showModal();$('gallery-search').focus();};
 $('gallery-close').onclick=()=>dialog.close();
 $('gallery-filters').onsubmit=e=>e.preventDefault();$('gallery-search').oninput=()=>render(true);
 for(const id of ['gallery-source','gallery-sort'])$(id).onchange=()=>render(true);
 $('gallery-previous').onclick=()=>{page--;render();};$('gallery-next').onclick=()=>{page++;render();};
 if(popover&&!popover.hidden)showFeatured();
 window.demoStartup??={};window.demoStartup.galleryReadyMs=performance.now()-started;
 $('gallery-status').textContent=isReady()?'Choose another image anytime':'Choose an image while the decoder loads';
 return {setSelected};
}
