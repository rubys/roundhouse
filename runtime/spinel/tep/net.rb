# All FFI plumbing lives at the top level so spinel's name resolver
# finds it from anywhere in the Tep tree (nested modules confuse it).
#
# Transport migration (matz/spinel#1466): the socket + process layer is
# being moved off our vendored C shim (sphttp.c) onto spinel's maintained,
# auto-linked `sp_net`. STAGE 1 (this commit) routes accept/poll/recv/
# write/connect/fork/clock through `sp_net` via thin `sphttp_*` delegators
# (call sites unchanged), with `now_us` reimplemented on the native
# monotonic clock. The HTTP request buffer, body drain, the binary WS
# frame-recv buffer, sendfile, and chunked write still ride sphttp.c
# (Stage 2 ports those to `sp_net` + `:binstr` and deletes sphttp.c).
#
# `@TEP_SPHTTP_O@` is substituted by `bin/tep` / the Makefile with the
# built sphttp.o path; it stays until Stage 2 removes the last sphttp.c
# dependency.
module Sock
  ffi_cflags "@TEP_SPHTTP_O@"

  # ── Socket + process layer: spinel's maintained sp_net (auto-linked) ──
  # recv uses the `:binstr` return mode (matz/spinel ac1e0d2c) — builds a
  # binary-safe String from (ptr, sp_net_bin_len) rather than strlen, so a
  # recv'd 0x00 no longer truncates.
  ffi_func :sp_net_listen,                [:int, :int],       :int
  ffi_func :sp_net_accept,                [:int],             :int
  ffi_func :sp_net_accept_nb,             [:int],             :int
  ffi_func :sp_net_set_nonblock,          [:int],             :int
  ffi_func :sp_net_close,                 [:int],             :int
  ffi_func :sp_net_write_str,             [:int, :str],       :int
  ffi_func :sp_net_write_bytes,           [:int, :str, :int], :int
  ffi_func :sp_net_recv_some,             [:int, :int],       :binstr
  ffi_func :sp_net_recv_all,              [:int, :int],       :binstr
  ffi_func :sp_net_connect,               [:str, :int],       :int
  ffi_func :sp_net_fork,                  [],                 :int
  ffi_func :sp_net_exit,                  [:int],             :int
  ffi_func :sp_net_getpid,                [],                 :int
  ffi_func :sp_net_wait_any,              [],                 :int
  ffi_func :sp_net_install_term_handlers, [],                 :int
  ffi_func :sp_net_shutdown_requested,    [],                 :int
  ffi_func :sp_net_poll_reset,            [],                 :int
  ffi_func :sp_net_poll_add,              [:int, :int],       :int
  ffi_func :sp_net_poll_run,              [:int],             :int
  ffi_func :sp_net_poll_ready,            [:int],             :int
  ffi_func :sp_net_shell_capture,         [:str, :int],       :binstr

  # Delegators: keep the existing `Sock.sphttp_*` call sites unchanged
  # while routing them through sp_net (Stage 1 of retiring sphttp.c).
  def self.sphttp_listen(port, reuse);      Sock.sp_net_listen(port, reuse);      end
  def self.sphttp_accept(sfd);              Sock.sp_net_accept(sfd);              end
  def self.sphttp_accept_nb(sfd);           Sock.sp_net_accept_nb(sfd);           end
  def self.sphttp_set_nonblock(fd);         Sock.sp_net_set_nonblock(fd);         end
  def self.sphttp_close(fd);                Sock.sp_net_close(fd);                end
  def self.sphttp_write_str(fd, s);         Sock.sp_net_write_str(fd, s);         end
  def self.sphttp_write_bytes(fd, data, n); Sock.sp_net_write_bytes(fd, data, n); end
  def self.sphttp_recv_some(fd, n);         Sock.sp_net_recv_some(fd, n);         end
  def self.sphttp_recv_all(fd, n);          Sock.sp_net_recv_all(fd, n);          end
  def self.sphttp_connect(host, port);      Sock.sp_net_connect(host, port);      end
  def self.sphttp_fork;                     Sock.sp_net_fork;                     end
  def self.sphttp_exit(status);             Sock.sp_net_exit(status);             end
  def self.sphttp_getpid;                   Sock.sp_net_getpid;                   end
  def self.sphttp_wait_any;                 Sock.sp_net_wait_any;                 end
  def self.sphttp_install_term_handlers;    Sock.sp_net_install_term_handlers;    end
  def self.sphttp_shutdown_requested;       Sock.sp_net_shutdown_requested;       end
  def self.sphttp_poll_reset;               Sock.sp_net_poll_reset;               end
  def self.sphttp_poll_add(fd, mode);       Sock.sp_net_poll_add(fd, mode);       end
  def self.sphttp_poll_run(timeout_ms);     Sock.sp_net_poll_run(timeout_ms);     end
  def self.sphttp_poll_ready(slot);         Sock.sp_net_poll_ready(slot);         end
  def self.sphttp_shell_capture(cmd, n);    Sock.sp_net_shell_capture(cmd, n);    end

  # Monotonic clock in microseconds. Replaces the C `sphttp_now_us`:
  # spinel exposes a native monotonic clock (Process.clock_gettime →
  # sp_process_clock_gettime / CLOCK_MONOTONIC). Float seconds → µs Int.
  def self.sphttp_now_us
    (Process.clock_gettime(Process::CLOCK_MONOTONIC) * 1000000.0).to_i
  end

  # ── Still on sphttp.c (ported in Stage 2): the HTTP request buffer +
  # body drain, the binary WS frame-recv buffer + per-byte accessor,
  # sendfile, filesize, and chunked write. ──
  ffi_func :sphttp_read_request,    [:int],         :int
  ffi_func :sphttp_request_buf,     [],             :str
  ffi_func :sphttp_request_len,     [],             :int
  ffi_func :sphttp_drain_body,      [:int, :int],   :str
  ffi_func :sphttp_recv_into_frame, [:int],         :int
  ffi_func :sphttp_recv_frame_buf,  [],             :str
  ffi_func :sphttp_recv_frame_len,  [],             :int
  ffi_func :sphttp_recv_frame_byte, [:int],         :int
  ffi_func :sphttp_sendfile,        [:int, :str],   :int
  ffi_func :sphttp_filesize,        [:str],         :int
  ffi_func :sphttp_write_chunk,     [:int, :str],   :int
  ffi_func :sphttp_write_chunk_end, [:int],         :int
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
