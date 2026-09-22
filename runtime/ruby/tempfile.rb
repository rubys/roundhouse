# Ruby's `Tempfile.create` — a file with a unique name, yielded to a
# block and removed when the block ends.
#
# THE ONE FORM THE CORPUS WRITES, and it is the block form, which is
# also the only one that cleans up on its own:
#
#   Tempfile.create(%w[loader_probe .img], binmode: true) do |file|
#     file.write bytes
#     file.flush
#     Vips.vips_foreign_find_load(file.path)
#   end
#
# campfire's `vips_loader_policy_test` writes each probe image to a
# temp file because `vips_foreign_find_load` takes a PATH, not bytes.
#
# `tempfile` IS A STDLIB THE STRICT TARGETS DO NOT HAVE — spinel says
# so ("uninitialized constant Tempfile: defined nowhere in the
# program") — so this is the ipaddr/zlib/resolv arrangement: a port for
# the targets with no stdlib to bind to, swapped for Ruby's own on the
# CRuby and JRuby trees by `project::ruby_runtime_files`. Nothing here
# is reached on those.
#
# WHAT IS NOT MODELED:
#
# * `Tempfile.new`, and with it the whole object — an unyielded
#   Tempfile is deleted by a finalizer, which is a lifetime no
#   compiled target has. `create`'s block IS the lifetime, and it is
#   what Ruby's own documentation recommends.
# * `tmpdir` as a positional argument. `TMPDIR` is read from the
#   environment with `/tmp` as the fallback, which is what
#   `Dir.tmpdir` answers on every platform this runs on.
# * `mode:`. `binmode:` is the only option the corpus passes and the
#   file is opened `"wb+"` either way — a temp file written and read
#   back in one block wants both halves, and on the platforms here a
#   binary open differs from a text one in nothing.
#
# THE NAME IS NOT A SECURITY BOUNDARY and this is the one place that
# matters. Ruby's `create` opens with `O_EXCL` and retries, so it
# cannot be made to clobber a file an attacker pre-created; this opens
# by name. Every caller in the corpus is a TEST writing to its own
# `TMPDIR`, so the exposure is nil there — but a runtime caller
# handling untrusted input would need the exclusive open, and that is
# a real difference rather than a detail. Recorded in
# docs/pipeline/runtime.md.
# The unique component of a name. `securerandom` is a package on the
# spinel tree and the stdlib on CRuby, the same arrangement
# `runtime/logger.rb` has with `stringio`.
require "securerandom"

module Tempfile
  # The pid and 64 bits of randomness, which is what makes a collision
  # between two processes, two threads and two calls in one thread all
  # impossible in practice. Ruby adds a per-process counter; with a
  # random component this wide it buys nothing, and a mutable
  # module-level counter is a shape worth not having.
  def self.create(basename, binmode: false)
    prefix, suffix = Tempfile.split_basename(basename)
    path = Tempfile.tmpdir + "/" + prefix + "-" + Process.pid.to_s + "-" +
           SecureRandom.hex(8) + suffix
    file = File.open(path, "wb+")
    # The begin/ensure IS the value, so the block's answer reaches the
    # caller with no local in between — a local bound to a `yield`
    # leaves the method's return an open type variable, which the
    # runtime typing gate reads as a body it could not resolve.
    begin
      yield file
    ensure
      # CLOSE THEN DELETE, and both in the ensure: a block that raised
      # still has an open descriptor and a file on disk, and a test
      # suite that leaks one per assertion runs a machine out of both.
      begin
        file.close
      rescue StandardError
        nil
      end
      begin
        File.delete(path)
      rescue StandardError
        nil
      end
    end
  end

  # `"probe"` or `%w[probe .img]` — Ruby accepts either, and the array
  # form is how a caller asks for a SUFFIX, which is the whole reason
  # campfire passes one (libvips is handed the path and some loaders
  # look at its extension).
  def self.split_basename(basename)
    if basename.is_a?(Array)
      prefix = basename.length > 0 ? basename[0].to_s : "tmp"
      suffix = basename.length > 1 ? basename[1].to_s : ""
      return [ prefix, suffix ]
    end
    [ basename.to_s, "" ]
  end

  # `Dir.tmpdir`'s answer without `Dir`: the environment's TMPDIR, or
  # `/tmp`. A trailing slash is stripped so the join below never
  # doubles it.
  def self.tmpdir
    dir = ENV["TMPDIR"].to_s
    dir = "/tmp" if dir.empty?
    dir = dir[0, dir.length - 1] while dir.length > 1 && dir[dir.length - 1] == "/"
    dir
  end
end
