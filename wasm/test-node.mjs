// End-to-end smoke test: load roundhouse_wasm.wasm under Node with a
// minimal WASI shim, feed it the real-blog fixture as JSON, verify the
// output has the expected file count.
//
// Run from this directory: node test-node.mjs

import { readFile, readdir } from "node:fs/promises";
import { resolve, relative, join } from "node:path";

const WASM = "./target/wasm32-wasip1/release/roundhouse_wasm.wasm";
const FIXTURE = resolve("../fixtures/real-blog");

async function loadFixture(root) {
  const out = {};
  async function walk(dir) {
    const entries = await readdir(dir, { withFileTypes: true });
    for (const e of entries) {
      const full = join(dir, e.name);
      if (e.isDirectory()) {
        await walk(full);
      } else if (e.isFile()) {
        const rel = relative(root, full);
        out[rel] = await readFile(full, "utf8");
      }
    }
  }
  await walk(root);
  return out;
}

// Minimal wasi_snapshot_preview1 stubs — same shape as ruby2js's
// prism_browser.js. The transpile path doesn't actually do I/O, so
// most calls return success with no work.
function makeWasi(memoryRef) {
  const view = () => new DataView(memoryRef.value.buffer);
  return {
    args_get: () => 0,
    args_sizes_get: (argc, argv_buf_size) => {
      view().setUint32(argc, 0, true);
      view().setUint32(argv_buf_size, 0, true);
      return 0;
    },
    environ_get: () => 0,
    environ_sizes_get: (count, buf_size) => {
      view().setUint32(count, 0, true);
      view().setUint32(buf_size, 0, true);
      return 0;
    },
    clock_res_get: () => 0,
    clock_time_get: (id, precision, time) => {
      view().setBigUint64(time, BigInt(Date.now()) * 1_000_000n, true);
      return 0;
    },
    fd_advise: () => 0,
    fd_allocate: () => 0,
    fd_close: () => 0,
    fd_datasync: () => 0,
    fd_fdstat_get: () => 0,
    fd_fdstat_set_flags: () => 0,
    fd_fdstat_set_rights: () => 0,
    fd_filestat_get: () => 0,
    fd_filestat_set_size: () => 0,
    fd_filestat_set_times: () => 0,
    fd_pread: () => 0,
    fd_prestat_get: () => 8, // BADF — no preopens
    fd_prestat_dir_name: () => 0,
    fd_pwrite: () => 0,
    fd_read: () => 0,
    fd_readdir: () => 0,
    fd_renumber: () => 0,
    fd_seek: () => 0,
    fd_sync: () => 0,
    fd_tell: () => 0,
    fd_write: (fd, iovs, iovs_len, nwritten) => {
      // Treat as stdout/stderr — collect bytes for printing.
      let total = 0;
      const dv = view();
      let bytes = [];
      for (let i = 0; i < iovs_len; i++) {
        const ptr = dv.getUint32(iovs + i * 8, true);
        const len = dv.getUint32(iovs + i * 8 + 4, true);
        const slice = new Uint8Array(memoryRef.value.buffer, ptr, len);
        bytes.push(...slice);
        total += len;
      }
      dv.setUint32(nwritten, total, true);
      const text = new TextDecoder().decode(new Uint8Array(bytes));
      if (fd === 1) process.stdout.write(text);
      else if (fd === 2) process.stderr.write(text);
      return 0;
    },
    path_create_directory: () => 0,
    path_filestat_get: () => 8,
    path_filestat_set_times: () => 0,
    path_link: () => 0,
    path_open: () => 8,
    path_readlink: () => 0,
    path_remove_directory: () => 0,
    path_rename: () => 0,
    path_symlink: () => 0,
    path_unlink_file: () => 0,
    poll_oneoff: () => 0,
    proc_exit: (code) => { throw new Error(`proc_exit(${code})`); },
    sched_yield: () => 0,
    random_get: (ptr, len) => {
      const buf = new Uint8Array(memoryRef.value.buffer, ptr, len);
      for (let i = 0; i < len; i++) buf[i] = Math.floor(Math.random() * 256);
      return 0;
    },
    sock_accept: () => 0,
    sock_recv: () => 0,
    sock_send: () => 0,
    sock_shutdown: () => 0,
  };
}

const memoryRef = { value: null };
const wasi = makeWasi(memoryRef);

const wasmBytes = await readFile(WASM);
const { instance } = await WebAssembly.instantiate(wasmBytes, {
  wasi_snapshot_preview1: wasi,
});
memoryRef.value = instance.exports.memory;

// Initialize wasi reactor (calls _initialize if present).
if (instance.exports._initialize) instance.exports._initialize();

const { rh_alloc, rh_dealloc, transpile, memory } = instance.exports;

const src = await loadFixture(FIXTURE);
console.error(`loaded fixture: ${Object.keys(src).length} files`);

const input = JSON.stringify({ language: "typescript", src });
const inputBytes = new TextEncoder().encode(input);

const t0 = performance.now();
const inputPtr = rh_alloc(inputBytes.length);
new Uint8Array(memory.buffer, inputPtr, inputBytes.length).set(inputBytes);

const packed = transpile(inputPtr, inputBytes.length);
const outPtr = Number(packed & 0xffffffffn);
const outLen = Number(packed >> 32n);

const outputBytes = new Uint8Array(memory.buffer, outPtr, outLen).slice();
const output = new TextDecoder().decode(outputBytes);

rh_dealloc(inputPtr, inputBytes.length);
rh_dealloc(outPtr, outLen);

const t1 = performance.now();
const result = JSON.parse(output);
console.error(`transpiled in ${(t1 - t0).toFixed(1)}ms`);

if (result.error) {
  console.error(`ERROR: ${result.error}`);
  process.exit(1);
}

console.error(`emitted ${result.files.length} files for ${result.language}`);
for (const f of result.files) {
  console.error(`  ${f.path}  (${f.content.length} bytes)`);
}
