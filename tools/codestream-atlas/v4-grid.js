/* Resolved V4 transform cells; no model IDs or entropy bytes are reinterpreted. */
(() => {
  'use strict';
  const merges = [[], [15], [3], [12], [3, 12], [5], [10], [5, 10]];
  function read(record) {
    const note = (record.notes || []).find(n => n[0] === 'V4 effective cells');
    if (!note) return null;
    const grid = JSON.parse(note[1]), [,,w,h] = record.region || [];
    if (!Number.isSafeInteger(grid.columns) || !Number.isSafeInteger(grid.rows) ||
        grid.columns !== Math.ceil(w / 16) || grid.rows !== Math.ceil(h / 16) ||
        grid.columns < 1 || grid.rows < 1 || !Array.isArray(grid.runs)) throw Error('V4 cell geometry');
    let cells = 0;
    for (const run of grid.runs) {
      if (!Array.isArray(run) || run.length !== 4) throw Error('V4 cell run');
      const [count, partition, plans, ac] = run;
      if (!Number.isSafeInteger(count) || count < 1 || !Number.isInteger(partition) ||
          partition < 0 || partition > 7 || !Array.isArray(plans) || plans.length !== 12 ||
          !plans.every(p => Number.isInteger(p) && p >= 0 && p <= 13 && p !== 10) ||
          !Array.isArray(ac) || ac.length !== 4 || !ac.every(v => Number.isInteger(v) && v >= 0 && v <= 255)) throw Error('V4 cell state');
      cells += count;
      if (!Number.isSafeInteger(cells)) throw Error('V4 cell count');
    }
    if (cells !== grid.columns * grid.rows) throw Error('V4 cell coverage');
    return grid;
  }
  function plan(code) {
    if (code === 0) return { width: 8, height: 8, sx: 1, sy: 1, name: '8×8' };
    if (code === 1) return { width: 4, height: 4, sx: 1, sy: 1, name: '4×4 (small-transform path)' };
    const layout = code - 2, sx = 1 << (layout & 1), sy = 1 << ((layout >> 1) & 1);
    const block = 1 << ((layout >> 2) + 1), width = Math.min(block, 8 / sx), height = Math.min(block, 8 / sy);
    return { width, height, sx, sy, name: `${width}×${height} (plane-layout path, ${sx}×${sy} sampling)` };
  }
  function visit(record, plane, each) {
    if (!Number.isInteger(plane) || plane < 0 || plane > 2) throw Error('V4 color plane');
    const grid = read(record);
    if (!grid) return 0;
    const [x0,y0,width,height] = record.region;
    let ordinal = 0, shapes = 0;
    const emit = (x,y,w,h,p,partition) => {
      w = Math.min(w, x0 + width - x); h = Math.min(h, y0 + height - y);
      if (w > 0 && h > 0) { each({x,y,width:w,height:h,transform:p,partition}); shapes++; }
    };
    for (const [count,partition,plans] of grid.runs) for (let i = 0; i < count; i++, ordinal++) {
      const x = x0 + ordinal % grid.columns * 16, y = y0 + Math.floor(ordinal / grid.columns) * 16;
      let merged = 0;
      for (const mask of merges[partition]) {
        const positions = [0,1,2,3].filter(p => mask & (1 << p));
        const xs = positions.map(p => p % 2), ys = positions.map(p => Math.floor(p / 2));
        const w = (Math.max(...xs) - Math.min(...xs) + 1) * 8;
        const h = (Math.max(...ys) - Math.min(...ys) + 1) * 8;
        if (positions.some(p => x + p % 2 * 8 >= x0 + width || y + Math.floor(p / 2) * 8 >= y0 + height || plans[p * 3 + plane] !== 0)) throw Error('V4 merge footprint');
        emit(x + Math.min(...xs)*8, y + Math.min(...ys)*8, w, h, {width:w,height:h,sx:1,sy:1,name:`${w}×${h} merged`}, partition);
        merged |= mask;
      }
      for (let p = 0; p < 4; p++) {
        if (merged & (1 << p)) continue;
        const px = x + p % 2 * 8, py = y + Math.floor(p / 2) * 8;
        const t = plan(plans[p * 3 + plane]), stepX = t.width * t.sx, stepY = t.height * t.sy;
        for (let yy = 0; yy < 8; yy += stepY) for (let xx = 0; xx < 8; xx += stepX) emit(px+xx, py+yy, stepX, stepY, t, partition);
      }
    }
    return shapes;
  }
  globalThis.ImiV4Grid = Object.freeze({ read, visit, plan });
})();
