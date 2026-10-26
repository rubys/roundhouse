require "socket"
require "tmpdir"
require "minitest/autorun"

module Sock
  @sockets = {}
  def self.sphttp_getpid = Process.pid
  def self.sp_net_unix_listen(path, backlog)
    socket = UNIXServer.new(path)
    socket.listen(backlog)
    @sockets[socket.fileno] = socket
    socket.fileno
  end
  def self.sphttp_close(fd) = @sockets.delete(fd)&.close
end
require_relative "../runtime/spinel/tep/unix_listener"

class UnixListenerTest < Minitest::Test
  def setup
    @dir = Dir.mktmpdir("tep-unix-", "/tmp")
    @path = File.join(@dir, "http.sock")
    @listener = Tep::UnixListener.new("unix://#{@path}")
  end

  def teardown
    @listener&.close
    FileUtils.remove_entry(@dir)
  end

  def test_disabled_and_validation
    disabled = Tep::UnixListener.new("")
    disabled.open
    assert_equal(-1, disabled.unix_fd)
    %w[http:///tmp/a unix://host/tmp/a unix://relative unix:///tmp/a?mode=888
       unix:///tmp/a?mode=1000 unix:///tmp/a?owner=1 unix:///tmp/a?mode=600&x=1
       unix:///tmp/a? unix:///tmp/a?mode=600?mode=660 unix:///tmp/a#fragment].each do |url|
      assert_raises(ArgumentError) { Tep::UnixListener.new(url) }
    end
    assert_raises(ArgumentError) { Tep::UnixListener.new("unix:///tmp/a\0b") }
    assert_equal(0660, Tep::UnixListener.new("unix://#{@path}?mode=660").unix_mode)
  end

  def test_mode_umask_and_cleanup
    prior = File.umask(0022)
    begin
      @listener.open
      assert_equal(0022, File.umask)
      assert_equal(0600, File.lstat(@path).mode & 0777)
      client = UNIXSocket.new(@path)
      client.close
      @listener.close
      refute(File.exist?(@path))
      @listener.close
    ensure
      File.umask(prior)
    end
  end

  def test_node_is_inaccessible_until_chmod_and_children_do_not_unlink
    original = File.method(:chmod)
    created_mode = nil
    File.define_singleton_method(:chmod) do |mode, path|
      created_mode = File.lstat(path).mode & 0777
      original.call(mode, path)
    end
    @listener.open
    assert_equal(0, created_mode)
    pid = fork { @listener.close; exit! 0 }
    Process.wait(pid)
    assert(File.socket?(@path))
  ensure
    File.define_singleton_method(:chmod, original)
  end

  def test_explicit_mode_and_stale_restart
    UNIXServer.new(@path).close
    @listener = Tep::UnixListener.new("unix://#{@path}?mode=660")
    @listener.open
    assert_equal(0660, File.lstat(@path).mode & 0777)
  end

  def test_active_socket_regular_file_and_symlink_are_unchanged
    live = UNIXServer.new(@path)
    ino = File.lstat(@path).ino
    assert_raises(IOError) { @listener.open }
    assert_equal(ino, File.lstat(@path).ino)
    live.close
    File.unlink(@path)
    File.write(@path, "keep")
    assert_raises(IOError) { @listener.open }
    assert_equal("keep", File.read(@path))
    File.unlink(@path)
    File.symlink("missing", @path)
    assert_raises(IOError) { @listener.open }
    assert_equal("missing", File.readlink(@path))
  end

  def test_cleanup_preserves_a_replacement_node
    @listener.open
    File.rename(@path, @path + ".owned")
    replacement = UNIXServer.new(@path)
    ino = File.lstat(@path).ino
    @listener.close
    assert_equal(ino, File.lstat(@path).ino)
  ensure
    replacement&.close
  end

  def test_startup_failure_restores_umask_and_removes_owned_node
    original = File.method(:chmod)
    File.define_singleton_method(:chmod) { |*| raise Errno::EPERM }
    prior = File.umask
    assert_raises(Errno::EPERM) { @listener.open }
    assert_equal(prior, File.umask)
    assert_equal(-1, @listener.unix_fd)
    refute(File.exist?(@path))
  ensure
    File.define_singleton_method(:chmod, original)
  end

  def test_probe_errors_preserve_the_socket
    UNIXServer.new(@path).close
    ino = File.lstat(@path).ino
    original = Socket.method(:new)
    [Errno::EACCES, Errno::EAGAIN, Errno::EINPROGRESS].each do |error|
      probe = Object.new
      probe.define_singleton_method(:connect_nonblock) { |_| raise error }
      probe.define_singleton_method(:close) {}
      Socket.define_singleton_method(:new) { |*| probe }
      assert_raises(error) { @listener.open }
      assert_equal(ino, File.lstat(@path).ino)
    end
  ensure
    Socket.define_singleton_method(:new, original)
  end

  def test_unrelated_child_does_not_complete_worker_reaping
    pids = [42, 43]
    reaped = [99, 42, 43]
    Sock.define_singleton_method(:sphttp_shutdown_requested) { 0 }
    Sock.define_singleton_method(:sphttp_wait_any) { reaped.shift }
    Tep.reap_workers(pids)
    assert_empty(pids)
    assert_empty(reaped)
  end
end
