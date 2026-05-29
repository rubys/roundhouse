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
  # Non-blocking accept variant used by Tep::Server::Scheduled.
  # Listen fd must be in non-blocking mode (sphttp_set_nonblock).
  # Returns -1 with errno EAGAIN/EWOULDBLOCK if no pending connection.
  ffi_func :sphttp_accept_nb,     [:int],           :int
  ffi_func :sphttp_read_request,  [:int],           :int
  ffi_func :sphttp_request_buf,   [],               :str
  ffi_func :sphttp_request_len,   [],               :int
  ffi_func :sphttp_drain_body,    [:int, :int],     :str
  ffi_func :sphttp_write_str,     [:int, :str],     :int

  # Binary-safe write + recv pair, used by Tep::WebSocket (and any
  # other caller that needs to send/receive bytes containing 0x00).
  # The recv side mirrors the request_buf / _len accessor pattern.
  # See sphttp.c for the binary-safety contract.
  ffi_func :sphttp_write_bytes,   [:int, :str, :int], :int
  ffi_func :sphttp_recv_into_frame, [:int],         :int
  ffi_func :sphttp_recv_frame_buf, [],              :str
  ffi_func :sphttp_recv_frame_len, [],              :int
  # Per-byte frame accessor (returns 0..255, or -1 OOB). Used by the
  # WebSocket frame codec instead of `sphttp_recv_frame_buf.bytes`:
  # the :str accessor is NUL-terminated on the Ruby side, so a masked
  # frame truncates at its first 0x00 (the 16-bit length high byte is
  # 0x00 for any payload <= 255 bytes). See sphttp.c for the contract.
  ffi_func :sphttp_recv_frame_byte, [:int],         :int

  ffi_func :sphttp_sendfile,      [:int, :str],     :int
  ffi_func :sphttp_filesize,      [:str],           :int
  ffi_func :sphttp_close,         [:int],           :int
  ffi_func :sphttp_fork,          [],               :int
  ffi_func :sphttp_exit,          [:int],           :int
  ffi_func :sphttp_getpid,        [],               :int
  ffi_func :sphttp_wait_any,      [],               :int

  # SIGTERM/SIGINT shutdown plumbing, used by Tep::Server::Scheduled's
  # accept loop. install_term_handlers arms the signal handlers (call
  # once before fork); shutdown_requested returns nonzero once a
  # TERM/INT has been delivered so the accept loop can break cleanly.
  ffi_func :sphttp_install_term_handlers, [],       :int
  ffi_func :sphttp_shutdown_requested,    [],       :int
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

  # popen-shaped shell capture used by Tep::Shell.run. File I/O goes
  # through spinel's built-in File.read / File.write since master
  # (matz/spinel#505 made File.write binary-safe).
  ffi_func :sphttp_shell_capture, [:str, :int],     :str
end

# Crypto FFI -- SHA-256/HMAC/PBKDF2/B64URL/random. Symbols live in
# spinel's libspinel_rt.a (added upstream as lib/sp_crypto.c via
# matz/spinel#514), which the spinel driver auto-links into every
# binary. No ffi_cflags needed; just declare the signatures.
module Crypto
  ffi_func :sp_crypto_hmac_sha256_hex,      [:str, :str],       :str
  ffi_func :sp_crypto_hmac_sha256_b64url,   [:str, :str],       :str
  ffi_func :sp_crypto_b64url_encode,        [:str],             :str
  ffi_func :sp_crypto_b64url_decode,        [:str],             :str
  ffi_func :sp_crypto_pbkdf2_sha256_b64url, [:str, :str, :int], :str
  ffi_func :sp_crypto_random_b64url,        [:int],             :str
  # SHA-1 + WebSocket accept-key compute. SHA-1 is shipped only
  # because RFC 6455 requires it for the Sec-WebSocket-Accept
  # derivation; do NOT use it for anything else (collision-broken).
  ffi_func :sp_crypto_sha1_hex,             [:str],             :str
  ffi_func :sp_crypto_websocket_accept,     [:str],             :str
end
