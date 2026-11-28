// Zero-dependency browser re-encoders from tightly packed RGB8 or RGBA8 pixels
// into uncompressed BMP or uncompressed store-mode PNG (DEFLATE BTYPE=00 stored
// blocks) with CICP and optional Display-P3 ICC metadata.
//
// Byte-identical to `tools/imij-wasm/web/crate/src/encode_img.rs`, allowing
// direct JS-vs-Wasm speed benchmarking with bit-exact output parity.

const CRC32_TABLE = new Uint32Array(256);
for (let i = 0; i < 256; i++) {
  let c = i;
  for (let k = 0; k < 8; k++) {
    c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
  }
  CRC32_TABLE[i] = c >>> 0;
}

export function crc32Update(crc, bytes, start = 0, end = bytes.length) {
  let c = (~crc) >>> 0;
  for (let i = start; i < end; i++) {
    c = CRC32_TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
  }
  return (~c) >>> 0;
}

export function adler32Update(s1, s2, bytes, start = 0, end = bytes.length) {
  const MOD = 65521;
  const NMAX = 5552;
  let i = start;
  while (i < end) {
    const stop = Math.min(i + NMAX, end);
    for (; i < stop; i++) {
      s1 += bytes[i];
      s2 += s1;
    }
    s1 %= MOD;
    s2 %= MOD;
  }
  return [s1, s2];
}

let cachedP3Icc = null;

/**
 * Canonical 492-byte ICC v4.3 Display-P3 profile (exact Bradford D65->D50
 * primaries summing to PCS D50, exact sRGB para funcType=3 TRC).
 * Byte-identical to `encode_img::display_p3_icc_v4()`.
 */
export function displayP3IccV4() {
  if (cachedP3Icc) return cachedP3Icc;
  const SIZE = 492;
  const out = new Uint8Array(SIZE);
  const dv = new DataView(out.buffer);
  const putStr = (at, str) => {
    for (let i = 0; i < str.length; i++) out[at + i] = str.charCodeAt(i);
  };

  // 128-byte header
  dv.setUint32(0, SIZE, false);
  dv.setUint32(8, 0x04300000, false); // v4.3.0.0
  putStr(12, 'mntr');
  putStr(16, 'RGB ');
  putStr(20, 'XYZ ');
  dv.setUint16(24, 2026, false);
  dv.setUint16(26, 1, false);
  dv.setUint16(28, 1, false);
  putStr(36, 'acsp');
  dv.setInt32(68, 0x0000f6d6, false);
  dv.setInt32(72, 0x00010000, false);
  dv.setInt32(76, 0x0000d32d, false);

  const tags = [
    ['desc', 252, 48],
    ['cprt', 300, 34],
    ['wtpt', 336, 20],
    ['chad', 356, 44],
    ['rXYZ', 400, 20],
    ['gXYZ', 420, 20],
    ['bXYZ', 440, 20],
    ['rTRC', 460, 32],
    ['gTRC', 460, 32],
    ['bTRC', 460, 32],
  ];
  dv.setUint32(128, tags.length, false);
  tags.forEach(([sig, off, len], idx) => {
    const base = 132 + idx * 12;
    putStr(base, sig);
    dv.setUint32(base + 4, off, false);
    dv.setUint32(base + 8, len, false);
  });

  const writeMluc = (at, text) => {
    putStr(at, 'mluc');
    dv.setUint32(at + 8, 1, false);
    dv.setUint32(at + 12, 12, false);
    putStr(at + 16, 'enUS');
    dv.setUint32(at + 20, text.length * 2, false);
    dv.setUint32(at + 24, 28, false);
    for (let i = 0; i < text.length; i++) {
      dv.setUint16(at + 28 + i * 2, text.charCodeAt(i), false);
    }
  };
  writeMluc(252, 'Display P3');
  writeMluc(300, 'CC0');

  const writeXyz = (at, x, y, z) => {
    putStr(at, 'XYZ ');
    dv.setInt32(at + 8, x, false);
    dv.setInt32(at + 12, y, false);
    dv.setInt32(at + 16, z, false);
  };
  writeXyz(336, 0x0000f6d6, 0x00010000, 0x0000d32d);

  putStr(356, 'sf32');
  const chad = [68674, 1502, -3291, 1939, 64912, -1119, -606, 988, 49262];
  for (let i = 0; i < chad.length; i++) dv.setInt32(364 + i * 4, chad[i], false);

  writeXyz(400, 33759, 15807, -69);
  writeXyz(420, 19135, 45366, 2745);
  writeXyz(440, 10296, 4363, 51385);

  putStr(460, 'para');
  dv.setUint16(468, 3, false);
  dv.setInt32(472, 157286, false);
  dv.setInt32(476, 62119, false);
  dv.setInt32(480, 3417, false);
  dv.setInt32(484, 5072, false);
  dv.setInt32(488, 2651, false);

  cachedP3Icc = out;
  return out;
}

export function isDisplayP3Cicp(cicp) {
  return Boolean(
    cicp &&
    cicp.primaries === 12 &&
    cicp.transfer === 13 &&
    cicp.matrix === 0 &&
    cicp.fullRange === true,
  );
}

/**
 * Encode tightly packed RGB8 (`channels === 3`) or RGBA8 (`channels === 4`) into
 * an uncompressed bottom-up BMP (`Uint8Array`).
 */
export function encodeBmpJs(pixels, width, height, {
  channels = 4,
  displayP3 = false,
  embedIcc = true,
} = {}) {
  if (
    !Number.isInteger(width) ||
    !Number.isInteger(height) ||
    width <= 0 ||
    height <= 0 ||
    width > 0x7fffffff ||
    height > 0x7fffffff ||
    (channels !== 3 && channels !== 4)
  ) {
    throw new RangeError('invalid BMP geometry or channels');
  }
  const srcRow = width * channels;
  if (!pixels || pixels.length !== srcRow * height) {
    throw new RangeError('BMP pixel buffer length mismatch');
  }
  const useV5 = channels === 4 || (displayP3 && embedIcc);
  const dibSize = useV5 ? 124 : 40;
  const icc = (useV5 && displayP3 && embedIcc) ? displayP3IccV4() : null;
  const pixelOffset = 14 + dibSize;
  const dstRow = (width * channels + 3) & ~3;
  const pixelBytes = dstRow * height;
  const total = pixelOffset + pixelBytes + (icc ? icc.length : 0);
  if (total > 0xffffffff) {
    throw new RangeError('BMP file size exceeds 32-bit limit');
  }

  const out = new Uint8Array(total);
  const dv = new DataView(out.buffer);
  out[0] = 0x42; // 'B'
  out[1] = 0x4d; // 'M'
  dv.setUint32(2, total, true);
  dv.setUint32(10, pixelOffset, true);

  dv.setUint32(14, dibSize, true);
  dv.setInt32(18, width, true);
  dv.setInt32(22, height, true);
  dv.setUint16(26, 1, true);
  dv.setUint16(28, channels * 8, true);
  dv.setUint32(30, channels === 4 ? 3 : 0, true); // BI_BITFIELDS / BI_RGB
  dv.setUint32(34, pixelBytes, true);
  dv.setInt32(38, 2835, true);
  dv.setInt32(42, 2835, true);

  if (useV5) {
    if (channels === 4) {
      dv.setUint32(54, 0x00ff0000, true);
      dv.setUint32(58, 0x0000ff00, true);
      dv.setUint32(62, 0x000000ff, true);
      dv.setUint32(66, 0xff000000, true);
    }
    if (icc) {
      dv.setUint32(70, 0x4d424544, true); // LCS_PROFILE_EMBEDDED ('MBED')
      dv.setUint32(122, 4, true); // LCS_GM_IMAGES
      dv.setUint32(126, dibSize + pixelBytes, true);
      dv.setUint32(130, icc.length, true);
    } else {
      dv.setUint32(70, 0x73524742, true); // LCS_sRGB ('sRGB')
      dv.setUint32(122, 4, true);
    }
  }

  if (channels === 3) {
    for (let y = 0; y < height; y++) {
      let s = (height - 1 - y) * srcRow;
      let d = pixelOffset + y * dstRow;
      for (let x = 0; x < width; x++, s += 3, d += 3) {
        out[d] = pixels[s + 2];
        out[d + 1] = pixels[s + 1];
        out[d + 2] = pixels[s];
      }
    }
  } else {
    for (let y = 0; y < height; y++) {
      let s = (height - 1 - y) * srcRow;
      let d = pixelOffset + y * dstRow;
      for (let x = 0; x < width; x++, s += 4, d += 4) {
        out[d] = pixels[s + 2];
        out[d + 1] = pixels[s + 1];
        out[d + 2] = pixels[s];
        out[d + 3] = pixels[s + 3];
      }
    }
  }

  if (icc) out.set(icc, pixelOffset + pixelBytes);
  return out;
}

/**
 * Encode tightly packed RGB8 (`channels === 3`) or RGBA8 (`channels === 4`) into
 * an uncompressed store-mode PNG (DEFLATE `BTYPE=00` stored blocks) with `cICP`
 * and optional Display-P3 `iCCP` chunk.
 */
export function encodeStorePngJs(pixels, width, height, {
  channels = 4,
  cicp = { primaries: 1, transfer: 13, matrix: 0, fullRange: true },
  includeIccp = true,
} = {}) {
  if (
    !Number.isInteger(width) ||
    !Number.isInteger(height) ||
    width <= 0 ||
    height <= 0 ||
    width > 0x7fffffff ||
    height > 0x7fffffff ||
    (channels !== 3 && channels !== 4)
  ) {
    throw new RangeError('invalid PNG geometry or channels');
  }
  const rowBytes = width * channels;
  if (!pixels || pixels.length !== rowBytes * height) {
    throw new RangeError('PNG pixel buffer length mismatch');
  }

  const rawRow = rowBytes + 1;
  const rawTotal = rawRow * height;
  const blockCount = Math.max(1, Math.ceil(rawTotal / 65535));
  const zlibLen = 2 + rawTotal + blockCount * 5 + 4;

  const emitIccp = Boolean(includeIccp && isDisplayP3Cicp(cicp));
  const icc = emitIccp ? displayP3IccV4() : null;
  const iccpDataLen = icc ? 12 + 2 + 5 + icc.length + 4 : 0;

  const totalLen =
    8 + // signature
    (12 + 13) + // IHDR
    (12 + 4) + // cICP
    (emitIccp ? 12 + iccpDataLen : 0) + // iCCP
    (12 + zlibLen) + // IDAT
    12; // IEND

  const out = new Uint8Array(totalLen);
  const dv = new DataView(out.buffer);
  let pos = 0;

  const writeChunk = (tagStr, data) => {
    dv.setUint32(pos, data.length, false);
    pos += 4;
    const tagStart = pos;
    for (let i = 0; i < 4; i++) out[pos++] = tagStr.charCodeAt(i);
    if (data.length) {
      out.set(data, pos);
      pos += data.length;
    }
    const crc = crc32Update(0, out, tagStart, pos);
    dv.setUint32(pos, crc, false);
    pos += 4;
  };

  // Signature
  out.set([137, 80, 78, 71, 13, 10, 26, 10], 0);
  pos = 8;

  // IHDR
  const ihdr = new Uint8Array(13);
  const ihdrDv = new DataView(ihdr.buffer);
  ihdrDv.setUint32(0, width, false);
  ihdrDv.setUint32(4, height, false);
  ihdr[8] = 8;
  ihdr[9] = channels === 3 ? 2 : 6;
  writeChunk('IHDR', ihdr);

  // cICP
  writeChunk('cICP', Uint8Array.of(
    cicp.primaries ?? 1,
    cicp.transfer ?? 13,
    cicp.matrix ?? 0,
    cicp.fullRange === false ? 0 : 1,
  ));

  // Optional iCCP fallback (uncompressed zlib stored block)
  if (emitIccp && icc) {
    const iccp = new Uint8Array(iccpDataLen);
    const idv = new DataView(iccp.buffer);
    const name = 'Display P3';
    for (let i = 0; i < name.length; i++) iccp[i] = name.charCodeAt(i);
    iccp[10] = 0; // NUL
    iccp[11] = 0; // compression method 0 = zlib
    iccp[12] = 0x78;
    iccp[13] = 0x01;
    iccp[14] = 0x01; // BFINAL=1, BTYPE=00
    idv.setUint16(15, icc.length, true);
    idv.setUint16(17, (~icc.length) & 0xffff, true);
    iccp.set(icc, 19);
    const [as1, as2] = adler32Update(1, 0, icc);
    idv.setUint32(19 + icc.length, ((as2 << 16) | as1) >>> 0, false);
    writeChunk('iCCP', iccp);
  }

  // IDAT
  dv.setUint32(pos, zlibLen, false);
  pos += 4;
  const idatTagStart = pos;
  out[pos++] = 73; // I
  out[pos++] = 68; // D
  out[pos++] = 65; // A
  out[pos++] = 84; // T
  out[pos++] = 0x78;
  out[pos++] = 0x01;

  let s1 = 1;
  let s2 = 0;
  let remaining = rawTotal;
  let rowIdx = 0;
  let rowOff = 0;

  while (remaining > 0) {
    const blk = Math.min(remaining, 65535);
    remaining -= blk;
    out[pos++] = remaining === 0 ? 1 : 0;
    out[pos++] = blk & 0xff;
    out[pos++] = (blk >>> 8) & 0xff;
    const nblk = (~blk) & 0xffff;
    out[pos++] = nblk & 0xff;
    out[pos++] = (nblk >>> 8) & 0xff;

    let left = blk;
    while (left > 0) {
      if (rowOff === 0) {
        out[pos++] = 0; // filter byte 0 = None
        s2 += s1;
        if (s2 >= 65521) s2 %= 65521;
        rowOff = 1;
        left -= 1;
        if (left === 0) break;
      }
      const pxOff = rowOff - 1;
      const take = Math.min(left, rowBytes - pxOff);
      const srcStart = rowIdx * rowBytes + pxOff;
      out.set(pixels.subarray(srcStart, srcStart + take), pos);
      [s1, s2] = adler32Update(s1, s2, pixels, srcStart, srcStart + take);
      pos += take;
      rowOff += take;
      left -= take;
      if (rowOff === rawRow) {
        rowOff = 0;
        rowIdx += 1;
      }
    }
  }

  dv.setUint32(pos, ((s2 << 16) | s1) >>> 0, false);
  pos += 4;
  const idatCrc = crc32Update(0, out, idatTagStart, pos);
  dv.setUint32(pos, idatCrc, false);
  pos += 4;

  // IEND
  writeChunk('IEND', new Uint8Array(0));
  return out;
}
