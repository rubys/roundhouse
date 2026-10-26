# frozen_string_literal: true
require "socket"

module Tep
  class UnixListener
    attr_reader :unix_fd, :unix_path, :unix_mode

    def initialize(url)
      @unix_fd = -1
      @unix_path = ""
      @unix_mode = 0600
      @unix_dev = -1
      @unix_ino = -1
      @unix_owner = Sock.sphttp_getpid
      return if url.empty?

      raise ArgumentError, "UNIX_BIND needs unix:///absolute/path?mode=600" unless url.start_with?("unix:///")
      parts = url.delete_prefix("unix://").split("?", -1)
      @unix_path = parts[0]
      if parts.length > 2 || @unix_path.include?("\0") || @unix_path.include?("#")
        raise ArgumentError, "invalid UNIX_BIND path"
      end
      if parts.length == 2
        query = parts[1]
        unless query.match?(/\Amode=[0-7]{1,4}\z/)
          raise ArgumentError, "UNIX_BIND accepts only an octal mode"
        end
        @unix_mode = query.delete_prefix("mode=").to_i(8)
        raise ArgumentError, "UNIX_BIND mode exceeds 0777" if @unix_mode > 0777
      end
      Socket.sockaddr_un(@unix_path)
    end

    def open
      return if @unix_path.empty? || @unix_fd >= 0
      remove_stale
      prior_umask = File.umask(0777)
      begin
        @unix_fd = Sock.sp_net_unix_listen(@unix_path, 1024)
        raise IOError, "cannot bind UNIX_BIND: " + @unix_path if @unix_fd < 0
        stat = File.lstat(@unix_path)
        raise IOError, "UNIX_BIND path changed during bind" unless stat.socket?
        @unix_dev = stat.dev
        @unix_ino = stat.ino
        File.chmod(@unix_mode, @unix_path)
      rescue StandardError
        close
        raise
      ensure
        File.umask(prior_umask)
      end
    end

    def close
      if @unix_fd >= 0
        Sock.sphttp_close(@unix_fd)
        @unix_fd = -1
      end
      return unless @unix_owner == Sock.sphttp_getpid && @unix_ino >= 0
      unlink_same(@unix_dev, @unix_ino)
      @unix_ino = -1
    end

    def remove_stale
      begin
        stat = File.lstat(@unix_path)
      rescue Errno::ENOENT
        return
      end
      raise IOError, "UNIX_BIND path is not a socket" unless stat.socket?
      dev = stat.dev
      ino = stat.ino
      probe = Socket.new(Socket::AF_UNIX, Socket::SOCK_STREAM, 0)
      stale = false
      begin
        probe.connect_nonblock(Socket.sockaddr_un(@unix_path))
      rescue Errno::ECONNREFUSED
        stale = true
      rescue Errno::ENOENT
        return
      ensure
        probe.close
      end
      raise IOError, "UNIX_BIND listener is active" unless stale
      unlink_same(dev, ino)
    end

    # Save numeric identity: Spinel's Stat accessors read the pathname again.
    def unlink_same(dev, ino)
      begin
        stat = File.lstat(@unix_path)
      rescue Errno::ENOENT
        return
      end
      if stat.socket? && stat.dev == dev && stat.ino == ino
        File.unlink(@unix_path)
      end
    end
  end

  def self.reap_workers(pids)
    remaining = pids.length
    signalled = false
    while remaining > 0
      if Sock.sphttp_shutdown_requested != 0 && !signalled
        pids.each do |pid|
          begin
            Process.kill("TERM", pid)
          rescue Errno::ESRCH
          end
        end
        signalled = true
      end
      gone = Sock.sphttp_wait_any
      if gone > 0
        pids.delete(gone)
        remaining = pids.length
      elsif Sock.sphttp_shutdown_requested == 0
        break
      end
    end
  end
end
