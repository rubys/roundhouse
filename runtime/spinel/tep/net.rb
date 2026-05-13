# All FFI plumbing lives at the top level so spinel's name resolver
# finds it from anywhere in the Tep tree (nested modules confuse it).
#
# The `@TEP_SPHTTP_O@` placeholder is substituted by `bin/tep` (or
# the Makefile) with the absolute path to the built sphttp.o on the
# current host. Spinel doesn't support `__dir__` or `ENV.fetch` in
# top-level ffi_cflags, so a build-time substitution is the cleanest
# portable shape.
module Sock
  ffi_cflags "@TEP_SPHTTP_O@"

  ffi_func :sphttp_listen,        [:int, :int],     :int
  ffi_func :sphttp_accept,        [:int],           :int
  ffi_func :sphttp_read_request,  [:int],           :int
  ffi_func :sphttp_request_buf,   [],               :str
  ffi_func :sphttp_request_len,   [],               :int
  ffi_func :sphttp_drain_body,    [:int, :int],     :str
  ffi_func :sphttp_write_str,     [:int, :str],     :int
  ffi_func :sphttp_sendfile,      [:int, :str],     :int
  ffi_func :sphttp_filesize,      [:str],           :int
  ffi_func :sphttp_close,         [:int],           :int
  ffi_func :sphttp_fork,          [],               :int
  ffi_func :sphttp_exit,          [:int],           :int
  ffi_func :sphttp_getpid,        [],               :int
  ffi_func :sphttp_wait_any,      [],               :int
  ffi_func :sphttp_hmac_sha256_hex,    [:str, :str], :str
  ffi_func :sphttp_hmac_sha256_b64url, [:str, :str], :str
  ffi_func :sphttp_b64url_encode,      [:str],       :str
  ffi_func :sphttp_b64url_decode,      [:str],       :str
  ffi_func :sphttp_pbkdf2_sha256_b64url, [:str, :str, :int], :str
  ffi_func :sphttp_random_b64url,      [:int],       :str
  ffi_func :sphttp_write_chunk,   [:int, :str],     :int
  ffi_func :sphttp_write_chunk_end, [:int],         :int

  # Poll-based I/O readiness, used by Tep::Scheduler.io_wait. Mode
  # bits in/out: 1=READ, 2=WRITE.
  ffi_func :sphttp_poll_reset,    [],               :int
  ffi_func :sphttp_poll_add,      [:int, :int],     :int
  ffi_func :sphttp_poll_run,      [:int],           :int
  ffi_func :sphttp_poll_ready,    [:int],           :int
  ffi_func :sphttp_set_nonblock,  [:int],           :int

  # Outbound TCP for clients (Tep::Http, etc.).
  ffi_func :sphttp_connect,       [:str, :int],     :int
  ffi_func :sphttp_recv_some,     [:int, :int],     :str
  ffi_func :sphttp_recv_all,      [:int, :int],     :str

  # Process + file helpers used by Tep::Shell.
  ffi_func :sphttp_shell_capture, [:str, :int],     :str
  ffi_func :sphttp_file_read,     [:str, :int],     :str
  ffi_func :sphttp_file_write,    [:str, :str],     :int
end
