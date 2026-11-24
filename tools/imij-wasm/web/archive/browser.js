import {ArchiveClient} from './client.js';
import {attribute} from './catalog.js';

const byId = id => document.getElementById(id);
const sizeFormat = new Intl.NumberFormat(undefined, {maximumFractionDigits: 1});
const integerFormat = new Intl.NumberFormat();
const rowCount = 32;
const sampleNames = ['U8', 'U16', 'F16', 'U32', 'I32', 'F32', 'I8', 'I16', 'U64', 'I64', 'F64'];
const channelNames = ['Unspecified', 'Gray', 'Gray + alpha', 'RGB', 'RGBA', 'CMYK', 'Oklab', 'Oklab + alpha', 'Scalar'];
const captions = {source_id: 'Source', canon: 'Canonicalisation', media_type: 'Media type',
                  doi_or_arxiv: 'DOI or arXiv', 'imij.profile': 'Archive profile'};
let archive;
let catalog;
let mode;
let page;
let items = [];
let starts;
let nextKey;
let selection;
let pageVersion = 0;
let selectionVersion = 0;

function node(tag, text, className) {
  const result = document.createElement(tag);
  if (text !== undefined) result.textContent = text;
  if (className) result.className = className;
  return result;
}

function status(text) { byId('status').textContent = text; }

function label(name) {
  return captions[name] || name.replaceAll('_', ' ').replace(/^./, letter => letter.toUpperCase());
}

function bytes(n) {
  if (n < 1024) return `${n} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let i = 0;
  for (n /= 1024; n >= 1024 && i < units.length - 1; n /= 1024) i++;
  return `${sizeFormat.format(n)} ${units[i]}`;
}

function facts(rows) {
  return rows.flatMap(([name, value]) => value === undefined || value === '' ? [] :
    [node('dt', label(name)), node('dd', String(value))]);
}

function button(text, action) {
  const result = node('button', text);
  result.type = 'button';
  result.addEventListener('click', async () => {
    try { await action(); }
    catch (error) { if (error.name !== 'AbortError') status(error.message); }
  });
  return result;
}

function liTitle(record) {
  return mode === 'pairs' ? record.distorted.name : record.name || `Entry ${record.key}`;
}

function liSummary(record) {
  if (mode === 'pairs') return [record.distortion, record.level && `Level ${record.level}`, record.source,
                                record.split].filter(Boolean).join(' · ');
  return [`Key ${record.key}`, record.width && record.height && `${record.width} × ${record.height}`,
          attribute(record, 'role'), bytes(record.bytes)].filter(Boolean).join(' · ');
}

function drawList() {
  const filter = byId('filter').value.trim().toLocaleLowerCase();
  const rows = [];
  for (const record of items) {
    const title = liTitle(record);
    const summary = liSummary(record);
    if (filter && !`${title} ${summary}`.toLocaleLowerCase().includes(filter)) continue;
    const row = button('', () => select(record));
    row.className = 'record';
    row.setAttribute('aria-pressed', String(record === selection));
    row.append(node('strong', title), node('small', summary));
    rows.push(row);
  }
  const list = byId('items');
  const scroll = list.scrollTop;
  list.replaceChildren(...(rows.length ? rows : [node('p', 'No matching records.', 'empty')]));
  list.scrollTop = scroll;
}

function pagination() {
  byId('previous').disabled = page === 0;
  byId('next').disabled = mode === 'pairs' ? page + 1 >= catalog.pairs.pageCount : nextKey === undefined;
  byId('position').textContent = mode === 'pairs'
    ? `Page ${page + 1} of ${integerFormat.format(catalog.pairs.pageCount)}` : `Page ${page + 1}`;
}

async function renderPage() {
  const id = ++pageVersion;
  selectionVersion++;
  selection = undefined;
  const currentMode = mode;
  const position = page;
  byId('selection').replaceChildren(node('p', 'Select a record to inspect it.', 'empty'));
  byId('contents-title').textContent = mode === 'pairs' ? 'Full-reference pairs' : 'Archive entries';
  byId('pairs-tab').setAttribute('aria-pressed', String(mode === 'pairs'));
  byId('entries-tab').setAttribute('aria-pressed', String(mode === 'entries'));
  byId('filter').value = '';
  byId('previous').disabled = byId('next').disabled = true;
  status('Reading index…');
  try {
    const results = currentMode === 'pairs' ? await archive.pairs(position) :
      await archive.list(starts[position], rowCount + 1);
    if (id !== pageVersion) return;
    items = results;
    nextKey = undefined;
    if (mode === 'entries' && results.length > rowCount) {
      nextKey = results.pop().key;
    }
    drawList();
    byId('items').scrollTop = 0;
    pagination();
    status('');
  } catch (error) {
    if (id === pageVersion && error.name !== 'AbortError') status(error.message);
  }
}

async function open(input) {
  const id = ++pageVersion;
  selectionVersion++;
  archive?.close();
  archive = new ArchiveClient();
  byId('archive').hidden = true;
  status('Opening archive…');
  try {
    const opened = await archive.open(input);
    if (id !== pageVersion) return;
    catalog = opened.catalog;
    byId('archive').hidden = false;
    byId('dataset-title').textContent = catalog?.dataset.name || 'IMI archive';
    byId('snapshot').textContent = `Generation ${opened.snapshot.generation} · ${bytes(opened.snapshot.bytes)}`;
    const rows = Object.entries(catalog?.dataset.metadata ?? {});
    if (catalog) rows.unshift(['full-reference pairs', integerFormat.format(catalog.pairs.count)]);
    byId('dataset-facts').replaceChildren(...facts(rows));
    byId('pairs-tab').hidden = !catalog;
    await view(catalog ? 'pairs' : 'entries');
  } catch (error) {
    if (id === pageVersion && error.name !== 'AbortError') status(error.message);
  }
}

async function view(target) {
  mode = target;
  page = 0;
  starts = ['0'];
  await renderPage();
}

function drawPreview(slot, preview, name) {
  if (!preview) {
    slot.textContent = 'Preview unavailable';
    return;
  }
  if (preview.text !== undefined) {
    slot.classList.add('metadata-preview');
    if (preview.media === 'application/json') {
      slot.replaceChildren(node('pre', JSON.stringify(JSON.parse(preview.text), null, 2)));
    } else {
      const table = node('table');
      const [columns, ...rows] = preview.text.trimEnd().split(/\r?\n/, 31);
      const header = table.createTHead().insertRow();
      for (const name of columns.split('\t', 32)) header.append(node('th', label(name)));
      const body = table.createTBody();
      for (const line of rows) {
        const row = body.insertRow();
        for (const value of line.split('\t', 32)) row.append(node('td', value));
      }
      slot.replaceChildren(node('p', 'First 30 rows and 32 columns'), table);
    }
    return;
  }
  if (preview.rgba) {
    const canvas = node('canvas');
    canvas.width = preview.width;
    canvas.height = preview.height;
    canvas.setAttribute('aria-label', name);
    canvas.getContext('2d').putImageData(new ImageData(preview.rgba, preview.width, preview.height), 0, 0);
    slot.replaceChildren(canvas);
    return;
  }
  const url = URL.createObjectURL(new Blob([preview.bytes], {type: preview.media}));
  const image = node('img');
  image.alt = name;
  image.addEventListener('load', () => URL.revokeObjectURL(url), {once: true});
  image.addEventListener('error', () => {
    URL.revokeObjectURL(url);
    slot.textContent = 'Preview could not be displayed';
  }, {once: true});
  image.src = url;
  slot.replaceChildren(image);
}

function download(data, filename, media = 'application/octet-stream') {
  const url = URL.createObjectURL(new Blob([data], {type: media}));
  const link = node('a');
  link.href = url;
  link.download = filename.split('/').at(-1);
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

async function downloadExact(asset) {
  status('Fetching exact scoring input…');
  const exact = await archive.exact(asset);
  if (exact.bytes) {
    download(exact.bytes, asset.name || `entry-${asset.key}.bin`);
  } else {
    const {pixels, icc, ...layout} = exact;
    download(pixels, `${asset.name}.pixels`);
    if (icc) {
      layout.iccFile = `${asset.name}.icc`.split('/').at(-1);
      download(icc, layout.iccFile);
    }
    download(JSON.stringify(layout, null, 2), `${asset.name}.pixels.json`, 'application/json');
  }
  status('');
}

function scoreTable(scores) {
  const table = node('table');
  const head = table.createTHead().insertRow();
  for (const name of ['Score', 'Value', 'Scale and direction', 'Source']) head.append(node('th', name));
  const body = table.createTBody();
  for (const score of scores) {
    const row = body.insertRow();
    const range = score.min !== undefined || score.max !== undefined ? `${score.min ?? '…'}–${score.max ?? '…'}` : '';
    const direction = score.higherIsBetter === undefined ? '' :
      score.higherIsBetter ? 'Higher is better' : 'Lower is better';
    const name = [score.name, score.kind === 'human' ? 'Human' : 'Metric', score.version].filter(Boolean).join(' · ');
    const value = score.std === undefined ? String(score.value) : `${score.value} (SD ${score.std})`;
    for (const [i, valueText] of [name, value, [range, direction].filter(Boolean).join(' · '), score.source].entries()) {
      row.append(node('td', valueText, i === 1 ? 'score' : undefined));
    }
  }
  return table;
}

async function showPair(pair, region, version) {
  region.append(node('h3', pair.distorted.name));
  region.append(node('p', liSummary(pair), 'subtitle'));
  const panel = node('div', undefined, 'previews');
  const assets = [pair.reference, pair.distorted];
  const slots = [];
  for (const [i, asset] of assets.entries()) {
    const figure = node('figure');
    const caption = i ? 'Distorted' : 'Reference';
    const slot = node('div', asset.thumbnail ? 'Loading thumbnail…' : 'Thumbnail unavailable', 'preview-image');
    slots.push(slot);
    figure.append(slot, node('figcaption', `${caption} · ${asset.name}`),
                  button(`Download ${caption.toLowerCase()}`, () => downloadExact(asset)));
    panel.append(figure);
  }
  region.append(node('h4', 'Stored scores'));
  region.append(pair.scores.length ? scoreTable(pair.scores) : node('p', 'No stored scores for this pair.'), panel);
  const downloads = node('div', undefined, 'downloads');
  downloads.append(button('Download pair record', () => download(
    JSON.stringify({dataset: catalog.dataset, pair}, null, 2), `pair-${pair.id}.json`, 'application/json',
  )));
  region.append(downloads);
  const selected = assets.filter(asset => asset.thumbnail);
  try {
    const thumbnails = await archive.previews(selected.map(asset => asset.thumbnail), true);
    if (version !== selectionVersion) return;
    for (const [i, asset] of selected.entries()) {
      drawPreview(slots[assets.indexOf(asset)], thumbnails[i], asset.name);
    }
  } catch (error) {
    if (version !== selectionVersion || error.name === 'AbortError') return;
    for (const slot of slots) slot.textContent = 'Thumbnail unavailable';
    status(error.message);
  }
}

function showEntry(record, region, version) {
  const title = record.name || `Entry ${record.key}`;
  region.append(node('h3', title));
  region.append(node('p', `Key ${record.key} · ${bytes(record.bytes)}`, 'subtitle'));
  const fields = [['offset', integerFormat.format(record.offset)], ['digest', record.digest]];
  if (record.width && record.height) {
    fields.unshift(['dimensions', `${record.width} × ${record.height}`]);
    if (!attribute(record, 'media_type')) {
      fields.push(['channels', channelNames[record.channels] ?? record.channels],
                  ['sample type', sampleNames[record.sample] ?? record.sample],
                  ['coding', (['Lossless', 'Lossy'][record.coding]) ?? record.coding]);
    }
  }
  if (record.imageCount) fields.push(['items', record.imageCount]);
  if (record.cicp) {
    const {primaries, transfer, matrix, fullRange} = record.cicp;
    fields.push(['color (CICP)', `${primaries} / ${transfer} / ${matrix} · ${fullRange ? 'Full' : 'Limited'} range`]);
  }
  fields.push(...record.attributes.map(({name, value}) => [name, value]));
  const dl = node('dl');
  dl.append(...facts(fields));
  const actions = node('div', undefined, 'downloads');
  const preview = node('div', undefined, 'preview-image');
  preview.hidden = true;
  actions.append(button('Preview', async () => {
    preview.hidden = false;
    preview.textContent = 'Loading preview…';
    try {
      const [image] = await archive.previews([record.key]);
      if (version === selectionVersion) drawPreview(preview, image, title);
    } catch (error) {
      if (version === selectionVersion) preview.textContent = `Preview unavailable. ${error.message}`;
    }
  }), button('Download entry', () => downloadExact(record)));
  region.append(dl, actions, preview);
}

async function select(record) {
  selection = record;
  drawList();
  const version = ++selectionVersion;
  const target = byId('selection');
  target.replaceChildren();
  if (matchMedia('(max-width: 780px)').matches) target.scrollIntoView({block: 'start'});
  if (mode === 'pairs') await showPair(record, target, version);
  else showEntry(record, target, version);
}

byId('open-form').addEventListener('submit', event => {
  event.preventDefault();
  const url = byId('archive-url').value;
  if (url) void open(url);
});
byId('archive-file').addEventListener('change', event => {
  if (event.target.files[0]) void open(event.target.files[0]);
});
byId('pairs-tab').addEventListener('click', () => void view('pairs'));
byId('entries-tab').addEventListener('click', () => void view('entries'));
byId('filter').addEventListener('input', drawList);
byId('previous').addEventListener('click', () => {
  if (page > 0) { page--; void renderPage(); }
});
byId('next').addEventListener('click', () => {
  if (mode === 'entries') {
    if (nextKey === undefined) return;
    starts[page + 1] = nextKey;
  }
  page++;
  void renderPage();
});

const initial = new URL(location.href).searchParams.get('archive');
if (initial) {
  try {
    const url = new URL(initial, location.href).href;
    byId('archive-url').value = url;
    void open(url);
  } catch (error) { status(error.message); }
}
