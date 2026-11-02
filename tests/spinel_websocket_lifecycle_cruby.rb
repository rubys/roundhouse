# CRuby supplies the OS bindings; all protocol and lifecycle code is shipped
# source shared with the native fixture. No WebSocket gem is needed.
require "socket"
require "io/wait"

class Module
  def ffi_lib(*); end
  def ffi_func(name, *)
    if name == :shutdown
      define_singleton_method(name) do |fd, how|
        socket = Socket.for_fd(fd)
        socket.autoclose = false
        socket.shutdown(how)
      rescue SystemCallError
        -1
      end
    end
  end
end

module Sock
  SOCKETS = {}
  def self.sp_net_listen_host(host, port, backlog)
    server = TCPServer.new(host, port)
    SOCKETS[server.fileno] = server
    server.fileno
  end
  def self.sp_net_accept(fd)
    socket = SOCKETS.fetch(fd).accept
    SOCKETS[socket.fileno] = socket
    socket.fileno
  end
  def self.sp_net_set_nonblock(fd) = 0
  def self.sp_net_close(fd)
    SOCKETS.delete(fd)&.close
    0
  end
  def self.sp_net_write_bytes(fd, bytes, n)
    return n unless SOCKETS.key?(fd)
    return 0 if n == 0
    SOCKETS.fetch(fd).write(bytes.byteslice(0, n))
  rescue SystemCallError, IOError
    -1
  end
  def self.sp_net_write_str(fd, bytes) = sp_net_write_bytes(fd, bytes, bytes.bytesize)
  def self.sp_net_recv_some(fd, n)
    SOCKETS.fetch(fd).readpartial(n)
  rescue EOFError, SystemCallError
    "".b
  end
end

load File.join(__dir__, "spinel_websocket_lifecycle_native.rb")
