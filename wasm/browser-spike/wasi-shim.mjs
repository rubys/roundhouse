// Minimal wasi_snapshot_preview1 shim, ported verbatim from test-node.mjs's
// makeWasi — note it uses ONLY web-platform APIs (DataView, TextDecoder,
// Math.random, Date.now), so this exact module runs unchanged in both Node
// and the browser. That portability is the whole point of the Phase 0 spike:
// no @bjorn3/browser_wasi_shim, no npm, no bundler.
//
// The transpile path does no real I/O; most calls return success with no work.
// fd_write is routed to an injectable sink (defaults to console) so stdout/
// stderr from the wasm surface in either environment.

export function makeWasi(memoryRef, { onStdout, onStderr } = {}) {
  const view = () => new DataView(memoryRef.value.buffer);
  const decoder = new TextDecoder();
  const stdout = onStdout || ((s) => console.log(s));
  const stderr = onStderr || ((s) => console.error(s));
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
      let total = 0;
      const dv = view();
      const bytes = [];
      for (let i = 0; i < iovs_len; i++) {
        const ptr = dv.getUint32(iovs + i * 8, true);
        const len = dv.getUint32(iovs + i * 8 + 4, true);
        const slice = new Uint8Array(memoryRef.value.buffer, ptr, len);
        bytes.push(...slice);
        total += len;
      }
      dv.setUint32(nwritten, total, true);
      const text = decoder.decode(new Uint8Array(bytes));
      if (fd === 1) stdout(text);
      else if (fd === 2) stderr(text);
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
