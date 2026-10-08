# frozen_string_literal: true

module Roundsnap
  # Turn `#<SPINEL_SOURCE>file:line` marked Ruby into ISeq units whose
  # backtrace lines match the original app lines (contiguous spans).
  #
  # MRI only shifts a whole string with `first_lineno`. Strategy:
  # 1. Unmarked prefix (requires / `module` wrappers) stays at the top.
  # 2. Each marked statement is placed on its absolute source line in a
  #    slot array (blank-padded). Within a held marker, the first code
  #    line uses that line exactly; further lines auto-increment until
  #    the next marker (MRI cannot stack two statements on one line).
  # 3. `first_lineno = 1 - prefix_len` so backtraces report source line L.
  #
  # One original file per marked unit (Roundhouse emit shape). A second
  # `#<SPINEL_SOURCE>` file in the same source raises — flushing mid-wrapper
  # would split unbalanced `module`/`class`/`end` across ISeqs. Unmarked
  # source keeps the emit path — honest emitted lines, never a fake
  # original:emitted mix.
  module SourceMap
    MARKER_PREFIX = "#<SPINEL_SOURCE>"
    MARKER_RE = /\A\s*#{Regexp.escape(MARKER_PREFIX)}(.+):(\d+)\s*\z/.freeze

    module_function

    def units_from(source, emit_key:)
      raise ArgumentError, "emit_key required" if emit_key.nil? || emit_key.to_s.empty?

      emit_file = emit_key.to_s.end_with?(".rb") ? emit_key.to_s : "#{emit_key}.rb"
      stem = emit_key.to_s.sub(/\.rb\z/, "")

      unless source.include?(MARKER_PREFIX)
        return [{
          "key" => stem,
          "source" => source,
          "file" => emit_file,
          "first_lineno" => 1,
          "mapped" => false,
        }]
      end

      spans = split_spans(source)
      spans.each_with_index.map do |span, i|
        {
          "key" => i.zero? ? stem : "#{stem}__span#{i}",
          "source" => span[:source],
          "file" => span[:file],
          "first_lineno" => span[:first_lineno],
          "mapped" => true,
        }
      end
    end

    def build_unit(prefix, placed, file)
      prefix_len = prefix.length
      max_line = placed.map { |e| e[:line] }.max || 1
      slots = Array.new(max_line, nil)
      placed.each do |e|
        idx = e[:line] - 1
        next if idx < 0

        if slots[idx].nil?
          slots[idx] = e[:code]
        else
          j = idx + 1
          j += 1 while j < slots.length && !slots[j].nil?
          if j < slots.length
            slots[j] = e[:code]
          else
            slots << e[:code]
          end
        end
      end
      body = slots.map { |c| c.nil? ? "" : c }
      {
        file: file,
        source: (prefix + body).join("\n") + "\n",
        first_lineno: 1 - prefix_len,
      }
    end

    def split_spans(source)
      spans = []
      prefix = []
      placed = []
      file = nil
      held = nil
      exact_next = false
      seen_marker = false

      flush = lambda do
        return if file.nil? || placed.empty?

        spans << build_unit(prefix, placed, file)
        # Later spans are reopenings — no require/module prefix.
        prefix = []
        placed = []
        file = nil
        held = nil
        exact_next = false
      end

      source.each_line do |raw|
        line = raw.chomp
        if (m = MARKER_RE.match(line))
          new_file = m[1]
          new_line = m[2].to_i
          if seen_marker && !file.nil? && new_file != file
            raise ArgumentError,
                  "roundsnap: multi-file #<SPINEL_SOURCE> markers in one " \
                  "unit are unsupported (saw #{file.inspect} then " \
                  "#{new_file.inspect}); emit one original file per unit"
          end
          file = new_file
          held = new_line
          exact_next = true
          seen_marker = true
          next
        end

        unless seen_marker
          prefix << line
          next
        end

        next if line.strip.empty?

        if exact_next
          placed << { code: line, line: held }
          exact_next = false
        else
          held += 1
          placed << { code: line, line: held }
        end
      end
      flush.call

      if spans.empty?
        [{ file: "unknown.rb", source: source, first_lineno: 1 }]
      else
        spans
      end
    end
    private_class_method :split_spans, :build_unit
  end
end
