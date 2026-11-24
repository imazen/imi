// Chromium range delivery for the real kVAL fixtures emitted by item_metadata tests.
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { join } from "node:path";
import assert from "node:assert/strict";

const [fixtures, playwrightModule] = process.argv.slice(2);
assert(fixtures && playwrightModule, "provide a fixture directory and Playwright module path");
const { chromium } = createRequire(import.meta.url)(playwrightModule);

function metadataRange(file, valueBytes) {
  let pos = 8;
  function varint() {
    let n = 0;
    let factor = 1;
    for (let i = 0; i < 8 && pos < file.length; i++) {
      const byte = file[pos++];
      n += (byte & 127) * factor;
      if (byte < 128 && Number.isSafeInteger(n)) return n;
      factor *= 128;
    }
    throw Error("bad fixture length");
  }
  const headerBytes = varint();
  pos += headerBytes;
  while (pos < file.length) {
    const tag = file.toString("ascii", pos, pos + 4);
    pos += 4;
    const length = varint() - 1;
    assert(length >= 0 && pos + length <= file.length);
    if (tag === "kVAL" || tag === "kVMD") {
      assert.equal(file[pos], 2);
      const end = pos + length;
      const start = end - valueBytes;
      assert(start > pos);
      for (let i = 0; i < valueBytes; i++) assert.equal(file[start + i], i % 251);
      return { start, end };
    }
    assert.notEqual(tag, "DATA", "expected attributes before DATA");
    pos += length;
  }
  throw Error("fixture has no typed attributes");
}

const inputs = new Map([32 << 10, 1 << 20].map(size => {
  const file = readFileSync(join(fixtures, `${size}.imi`));
  return [size, { file, ...metadataRange(file, size) }];
}));
const requests = new Map();
const server = createServer((req, res) => {
  if (req.url === "/") {
    const html = "<title>Metadata range</title>";
    res.writeHead(200, { "Content-Type": "text/html", "Content-Length": html.length });
    res.end(html);
    return;
  }
  const match = /^\/image\/(\d+)\/(0|50)\/(\d+)(?:\/(prefix|value))?$/.exec(req.url);
  const range = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range ?? "");
  if (!match || !range) {
    res.writeHead(404).end();
    return;
  }
  const [, size, delay, id] = match.map(Number);
  const [, first, last] = range.map(Number);
  const input = inputs.get(size);
  if (!input || first > last || last >= input.file.length) {
    res.writeHead(416).end();
    return;
  }
  const body = input.file.subarray(first, last + 1);
  const read = { first, last, bytes: body.length, receivedMs: performance.now() };
  requests.get(id).push(read);
  setTimeout(() => {
    res.writeHead(206, {
      "Content-Type": "application/octet-stream",
      "Content-Length": body.length,
      "Content-Range": `bytes ${first}-${last}/${input.file.length}`,
      "Accept-Ranges": "bytes",
      "Cache-Control": "public, max-age=120",
      "ETag": `"kvmd-${size}"`,
    }).end(body);
  }, delay);
});
// These are loopback-only test servers. Stay in the workspace port range.
for (let port = 3600; port < 3700; port++) {
  try {
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(port, "127.0.0.1", () => {
        server.off("error", reject);
        resolve();
      });
    });
    break;
  } catch (error) {
    if (error.code !== "EADDRINUSE") throw error;
  }
}
assert(server.listening, "no test port available");
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({ args: ["--disable-dev-shm-usage"] });
const layouts = ["inline", "browse", "serial", "parallel", "parallel-split", "parallel-no-store"];
let id = 0;
try {
  console.log(JSON.stringify({ chromium: browser.version(), samples: 7, protocol: "http/1.1" }));
  for (const delay of [0, 50]) {
    for (const [size, { start, end }] of inputs) {
      for (let trial = 0; trial < 7; trial++) {
        for (let slot = 0; slot < layouts.length; slot++) {
          const layout = layouts[(slot + trial) % layouts.length];
          const context = await browser.newContext();
          try {
            const page = await context.newPage();
            await page.goto(base);
            const url = `/image/${size}/${delay}/${id}`;
            requests.set(id, []);
            for (const pass of ["cold", "repeat"]) {
              const network = requests.get(id);
              network.length = 0;
              const ms = await page.evaluate(async ({ url, start, end, layout }) => {
                async function read(first, last, suffix = "") {
                  const response = await fetch(url + suffix, {
                    cache: layout === "parallel-no-store" ? "no-store" : "default",
                    headers: { Range: `bytes=${first}-${last}` },
                  });
                  if (response.status !== 206) throw Error("expected partial content");
                  const expected = `bytes ${first}-${last}/`;
                  if (!response.headers.get("content-range")?.startsWith(expected)) {
                    throw Error("unexpected content range");
                  }
                  const bytes = new Uint8Array(await response.arrayBuffer());
                  if (bytes.length !== last - first + 1) throw Error("short range body");
                  if (last === end - 1 && bytes.at(-1) !== (end - start - 1) % 251) {
                    throw Error("unexpected value");
                  }
                }
                const before = performance.now();
                if (layout === "inline") await read(0, end - 1);
                if (layout === "browse") await read(0, start - 1);
                if (layout === "serial") {
                  await read(0, start - 1);
                  await read(start, end - 1);
                }
                if (layout.startsWith("parallel")) {
                  const split = layout === "parallel-split";
                  await Promise.all([
                    read(0, start - 1, split ? "/prefix" : ""),
                    read(start, end - 1, split ? "/value" : ""),
                  ]);
                }
                return performance.now() - before;
              }, { url, start, end, layout });
              if (pass === "cold") assert.equal(network.length, layout === "inline" || layout === "browse" ? 1 : 2);
              console.log(JSON.stringify({ delay, size, preludeBytes: start, trial, layout, pass, ms, network }));
            }
            requests.delete(id++);
          } finally {
            await context.close();
          }
        }
      }
    }
  }
} finally {
  await browser.close();
  server.close();
}
