# Resolv, reopened over spinel's `Socket` so `Resolv.getaddresses` is a
# real lookup on the compiled binary — `runtime/resolv.rb` carries the
# stub walk and leaves `resolve` (the unstubbed hook) failing loudly for
# a target with nothing to bind to. This one has `Socket.getaddrinfo`.
#
# NOT NAMED `resolv.rb`: the ruby family reaches the stdlib's `Resolv`
# through `project::RESOLV_STUB_REOPEN`, and `walk_dir_flat` copies every
# runtime/spinel/*.rb into every tree, so under the library's own name
# this would answer a bare `require "resolv"` there. Same rule as
# `cgi_spinel.rb`. Loaded from spinel's boot.rb only.
#
# Every address, not the first: `Surfguard.resolve_public_ips` validates
# ALL of them, which is the point of the policy (a host that answers one
# public and one private address is rejected, not admitted on the luck of
# the draw). getaddrinfo answers one row per (family, socktype) pair, so
# the same address repeats; `uniq` by hand keeps the order the resolver
# gave. An unknown host is `[]`, as Ruby's `Resolv.getaddresses` answers
# (spinel's getaddrinfo returns no rows rather than raising; the rescue
# is for the day it does).
require "socket"
require_relative "resolv"

class Resolv
  def self.resolve(host)
    out = []
    begin
      Socket.getaddrinfo(host, nil).each do |row|
        # The row is `[family, port, host, ip, …]`, read through a poly
        # slot; `to_s` is what makes `out` an Array[String] to the
        # compiler and to `runtime/resolv.rbs`.
        ip = row[3].to_s
        out << ip if ip != "" && !out.include?(ip)
      end
    rescue StandardError
      out.clear
    end
    out
  end
end
