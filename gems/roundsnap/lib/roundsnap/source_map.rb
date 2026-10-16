# frozen_string_literal: true

require "ripper"

module Roundsnap
  # Source markers describe IR spans, not a license to move emitted code.
  # Keep the source byte-for-byte, including heredocs and blank lines, and
  # record a sidecar from emitted lines to the last pinned source position.
  # A held marker does NOT increment: one lowered statement can span many
  # lines. Native ISeq/Coverage locations remain honest emitted locations.
  module SourceMap
    MARKER_PREFIX = "#<SPINEL_SOURCE>"
    MARKER_RE = /\A\s*#{Regexp.escape(MARKER_PREFIX)}(.+):(\d+)\s*\z/.freeze

    module_function

    def units_from(source, emit_key:)
      raise ArgumentError, "emit_key required" if emit_key.nil? || emit_key.to_s.empty?

      file = emit_key.to_s.end_with?(".rb") ? emit_key.to_s : "#{emit_key}.rb"
      lines = source.lines
      markers = {}
      # Only actual whole-line comments count. A marker-looking string or
      # heredoc line is program data, not source-map metadata.
      Ripper.lex(source).each do |(line, column), type, token, _|
        next unless type == :on_comment && lines[line - 1][0...column].strip.empty?
        next unless (match = MARKER_RE.match(token.chomp))

        position = match[2].to_i
        raise ArgumentError, "roundsnap: source marker line must be positive" unless position.positive?

        markers[line] = { "file" => match[1], "line" => position }
      end
      locations = {}
      active = nil
      source.each_line.with_index(1) do |_, line|
        if markers.key?(line)
          active = markers[line]
        elsif active
          locations[line.to_s] = active
        end
      end
      [{
        "key" => file.sub(/\.rb\z/, ""),
        "source" => source,
        "file" => file,
        "first_lineno" => 1,
        "mapped" => !locations.empty?,
        "source_map" => locations,
      }]
    end
  end
end
