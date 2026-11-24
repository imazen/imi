// Runs WASI (wasm32-wasip1) command modules in a Web Worker: argv, stdin
// bytes in, exit code + stdout/stderr bytes out. Implements the preview1
// calls a Rust std command needs; everything else returns ENOSYS. There is
// no file system: programs read stdin and write stdout.
//
// Message in:  { id, url, args: [string] (without argv[0]), stdin: Uint8Array }
// Message out: { id, code, stdout: Uint8Array, stderr: Uint8Array, ms } or { id, error }

const compiled = new Map();

self.onmessage = async (e) => {
  const { id, url, args, stdin } = e.data;
  try {
    let module = compiled.get(url);
    if (!module) {
      const bytes = await (await fetch(url)).arrayBuffer();
      module = await WebAssembly.compile(bytes);
      compiled.set(url, module);
    }
    const t0 = performance.now();
    // argv[0] is the program name, as a shell would pass it.
    const r = run(module, [url.split('/').pop(), ...args], stdin || new Uint8Array(0));
    r.ms = performance.now() - t0;
    self.postMessage({ id, ...r }, [r.stdout.buffer, r.stderr.buffer]);
  } catch (err) {
    self.postMessage({ id, error: String(err && err.stack || err) });
  }
};

class Exit {
  constructor(code) { this.code = code; }
}

const ESUCCESS = 0, EBADF = 8, ENOSYS = 52, ESPIPE = 70;

function run(module, args, stdin) {
  let memory = null;
  const view = () => new DataView(memory.buffer);
  const u8 = () => new Uint8Array(memory.buffer);
  const enc = new TextEncoder();
  const argv = args.map((a) => enc.encode(a + '\0'));
  let stdinPos = 0;
  const out = [], err = [];

  const iovecs = (ptr, len) => {
    const v = view(), list = [];
    for (let i = 0; i < len; i++) list.push([v.getUint32(ptr + 8 * i, true), v.getUint32(ptr + 8 * i + 4, true)]);
    return list;
  };
  const wasi = {
    args_sizes_get(argc, size) {
      const v = view();
      v.setUint32(argc, argv.length, true);
      v.setUint32(size, argv.reduce((s, a) => s + a.length, 0), true);
      return ESUCCESS;
    },
    args_get(ptrs, buf) {
      const v = view(), m = u8();
      for (const a of argv) {
        v.setUint32(ptrs, buf, true);
        ptrs += 4;
        m.set(a, buf);
        buf += a.length;
      }
      return ESUCCESS;
    },
    environ_sizes_get(count, size) {
      const v = view();
      v.setUint32(count, 0, true);
      v.setUint32(size, 0, true);
      return ESUCCESS;
    },
    environ_get() { return ESUCCESS; },
    fd_write(fd, iovs, n, written) {
      if (fd !== 1 && fd !== 2) return EBADF;
      let total = 0;
      for (const [p, l] of iovecs(iovs, n)) {
        (fd === 1 ? out : err).push(u8().slice(p, p + l));
        total += l;
      }
      view().setUint32(written, total, true);
      return ESUCCESS;
    },
    fd_read(fd, iovs, n, read) {
      if (fd !== 0) return EBADF;
      let total = 0;
      for (const [p, l] of iovecs(iovs, n)) {
        const take = Math.min(l, stdin.length - stdinPos);
        u8().set(stdin.subarray(stdinPos, stdinPos + take), p);
        stdinPos += take;
        total += take;
        if (take < l) break;
      }
      view().setUint32(read, total, true);
      return ESUCCESS;
    },
    fd_fdstat_get(fd, buf) {
      if (fd > 2) return EBADF;
      const v = view();
      v.setUint8(buf, 2); // character device
      v.setUint16(buf + 2, 0, true);
      v.setBigUint64(buf + 8, 0xffffffffn, true);
      v.setBigUint64(buf + 16, 0xffffffffn, true);
      return ESUCCESS;
    },
    fd_prestat_get() { return EBADF; },
    fd_prestat_dir_name() { return EBADF; },
    fd_close() { return ESUCCESS; },
    fd_seek() { return ESPIPE; },
    proc_exit(code) { throw new Exit(code); },
    random_get(buf, len) {
      for (let i = 0; i < len; i += 65536) crypto.getRandomValues(u8().subarray(buf + i, buf + Math.min(len, i + 65536)));
      return ESUCCESS;
    },
    clock_time_get(id, _precision, outPtr) {
      const ns = id === 0
        ? BigInt(Date.now()) * 1000000n
        : BigInt(Math.round((performance.timeOrigin + performance.now()) * 1e6));
      view().setBigUint64(outPtr, ns, true);
      return ESUCCESS;
    },
    clock_res_get(_id, outPtr) {
      view().setBigUint64(outPtr, 1000n, true);
      return ESUCCESS;
    },
    sched_yield() { return ESUCCESS; },
  };
  const imports = {
    wasi_snapshot_preview1: new Proxy(wasi, {
      get: (t, k) => t[k] || (() => ENOSYS),
    }),
  };
  const instance = new WebAssembly.Instance(module, imports);
  memory = instance.exports.memory;
  let code = 0;
  try {
    instance.exports._start();
  } catch (e) {
    if (e instanceof Exit) code = e.code;
    else {
      err.push(enc.encode(`\n${e}`));
      code = 134;
    }
  }
  const cat = (chunks) => {
    const n = chunks.reduce((s, c) => s + c.length, 0), r = new Uint8Array(n);
    let o = 0;
    for (const c of chunks) { r.set(c, o); o += c.length; }
    return r;
  };
  return { code, stdout: cat(out), stderr: cat(err) };
}
