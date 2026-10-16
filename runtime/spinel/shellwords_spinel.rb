# frozen_string_literal: true
# Module-only Shellwords for Spinel.
#
# NOT NAMED `shellwords.rb`. `walk_dir_flat` copies every
# `runtime/spinel/*.rb` into every target tree, and the emitted
# Rakefile puts `runtime/` on `$LOAD_PATH` (`t.libs << "runtime"`).
# Under the library's own name this file answers `require "shellwords"`
# — Nokogiri's, among others — and it does not reopen String, so
# `String#shellescape` is missing and `bin/rh test ruby` dies at load.
# Same rule as `cgi_spinel.rb` (commit 1ba94ff2): a shim that stands in
# for a library on one target must not carry the library's name.
# CRuby/JRuby boot requires the stdlib; only spinel's boot loads this.
#
# Spinel's `packages/shellwords` reopens String and Array with
# `#shellsplit` / `#shellescape` / `#shelljoin`. That reopen makes
# `String#split`'s answer a PolyArray, and the whole Rails tree then
# fails C compile (`-Werror=incompatible-pointer-types` on every
# `parts = s.split(...)` that expects `sp_StrArray *`).
#
# Defining `Shellwords` here drops BUNDLED's require of the library
# for the tree (see `project::BUNDLED` — a program-defined constant
# skips the bundled row). The surface the corpus writes is
# `Shellwords.escape` only; String/Array core extensions stay out.

module Shellwords
  # Characters Bourne shell leaves alone. Everything else is
  # backslash-escaped; newline becomes a quoted newline, as in
  # ruby/shellwords 0.2.2.
  SAFE = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-.,:+/@"

  def self.escape(str)
    s = str.to_s
    return "''" if s.empty?
    raise ArgumentError, "NUL character" if !s.index("\0").nil?
    out = +""
    i = 0
    while i < s.length
      ch = s[i, 1].to_s
      if ch == "\n"
        out << "'\n'"
      elsif !SAFE.index(ch).nil?
        out << ch
      else
        out << "\\"
        out << ch
      end
      i += 1
    end
    out
  end

  def self.shellescape(str)
    escape(str)
  end
end
