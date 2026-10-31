# frozen_string_literal: true
module Rack
  class MiniProfiler
    # Not the gem's attribute Hash behind `[]`: spinel types a Hash by its values, and these mix Strings, Integers, Floats and timers.
    module TimerStruct
      # `::JSON.generate`'s escaping, which leaves `<`, `>` and `&` alone, unlike the ActiveSupport encoder in JsonBuilder.
      def self.json_string(s)
        out = +"\""
        i = 0
        n = s.bytesize
        while i < n
          b = s.getbyte(i)
          if b == 0x22
            out << "\\\""
          elsif b == 0x5C
            out << "\\\\"
          elsif b == 0x0A
            out << "\\n"
          elsif b == 0x0D
            out << "\\r"
          elsif b == 0x09
            out << "\\t"
          elsif b == 0x08
            out << "\\b"
          elsif b == 0x0C
            out << "\\f"
          elsif b < 0x20
            out << "\\u00" + HEX[b / 16] + HEX[b % 16]
          else
            out << s.byteslice(i, 1)
          end
          i += 1
        end
        out << "\""
        out
      end

      HEX = "0123456789abcdef"

      # JSON writes Ruby's Integer and Float differently, and the gem's counters start as Integer 0 and turn Float on the first addition.
      def self.number(value, float)
        float ? value.to_s : value.to_i.to_s
      end

      def self.now_ms
        (Process.clock_gettime(Process::CLOCK_MONOTONIC) * 1000).to_i
      end

      class Request
        attr_reader :id, :name, :depth, :is_trivial
        attr_accessor :parent, :start, :start_milliseconds, :start_float, :duration_milliseconds,
          :duration_without_children_milliseconds, :sql_timings_duration_milliseconds, :sql_float

        def initialize(name, page_started, parent)
          @id = MiniProfiler.generate_id
          @name = name
          @parent = parent
          @depth = parent.nil? ? 0 : parent.depth + 1
          @duration_milliseconds = 0.0
          @duration_set = false
          @duration_without_children_milliseconds = 0.0
          @start_milliseconds = (TimerStruct.now_ms - page_started).to_f
          @start_float = false
          @sql_timings_duration_milliseconds = 0.0
          @sql_float = false
          @is_trivial = false
          @start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
        end

        def root?
          @parent.nil?
        end

        def record_time(milliseconds, children_ms)
          @duration_milliseconds = milliseconds
          @duration_set = true
          @is_trivial = true if milliseconds < 2
          @duration_without_children_milliseconds = milliseconds - children_ms
          nil
        end

        def adjust_depth(depth)
          @depth = depth
          nil
        end

        def add_sql_ms(ms)
          @sql_timings_duration_milliseconds += ms
          @sql_float = true
          nil
        end

        def to_json(page)
          children = page.children_of(self)
          sqls = page.sqls_of(self)
          "{\"id\":" + TimerStruct.json_string(@id) +
            ",\"name\":" + TimerStruct.json_string(@name) +
            ",\"duration_milliseconds\":" + TimerStruct.number(@duration_milliseconds, @duration_set) +
            ",\"duration_without_children_milliseconds\":" + TimerStruct.number(@duration_without_children_milliseconds, @duration_set) +
            ",\"start_milliseconds\":" + TimerStruct.number(@start_milliseconds, @start_float) +
            ",\"parent_timing_id\":" + (@parent.nil? ? "null" : TimerStruct.json_string(@parent.id)) +
            ",\"children\":[" + children.map { |c| c.to_json(page) }.join(",") + "]" +
            ",\"has_children\":" + (!children.empty?).to_s + ",\"key_values\":null" +
            ",\"has_sql_timings\":" + (!sqls.empty?).to_s + ",\"has_duplicate_sql_timings\":false" +
            ",\"trivial_duration_threshold_milliseconds\":2,\"sql_timings\":[" + sqls.map { |q| q.to_json }.join(",") + "]" +
            ",\"sql_timings_duration_milliseconds\":" + TimerStruct.number(@sql_timings_duration_milliseconds, @sql_float) +
            ",\"is_trivial\":" + @is_trivial.to_s +
            ",\"is_root\":" + @parent.nil?.to_s + ",\"depth\":" + @depth.to_s + ",\"executed_readers\":0,\"executed_scalars\":0" +
            ",\"executed_non_queries\":0,\"custom_timing_stats\":{},\"custom_timings\":{}}"
        end
      end

      class Sql
        attr_accessor :parent
        attr_reader :start_milliseconds, :duration_milliseconds

        def initialize(query, duration_ms, page_started, parent, cached)
          @query = query
          @duration_milliseconds = duration_ms
          @start_milliseconds = (TimerStruct.now_ms - page_started) - duration_ms
          @parent = parent
          @cached = cached
        end

        # Not a stack_trace_snippet: the gem filters `Kernel.caller` to the app's frames, and a Spinel binary keeps no Ruby call stack.
        def to_json
          "{\"execute_type\":3,\"formatted_command_string\":" + TimerStruct.json_string(TimerStruct.html_escape(@query)) +
            ",\"stack_trace_snippet\":null,\"start_milliseconds\":" + @start_milliseconds.to_s +
            ",\"duration_milliseconds\":" + @duration_milliseconds.to_s +
            ",\"first_fetch_duration_milliseconds\":" + @duration_milliseconds.to_s +
            ",\"parameters\":null,\"row_count\":0,\"class_name\":null,\"parent_timing_id\":" + TimerStruct.json_string(@parent.id) +
            ",\"cached\":" + (@cached ? "true" : "null") + ",\"is_duplicate\":false}"
        end
      end

      def self.html_escape(s)
        s.gsub("&", "&amp;").gsub("<", "&lt;").gsub(">", "&gt;").gsub("\"", "&quot;").gsub("'", "&#39;")
      end

      class Page
        # The stored JSON is rewritten in place on the first view (`Page.viewed`), so these two keys are written side by side.
        UNVIEWED = "\"has_user_viewed\":false,\"client_timings\":null"

        attr_reader :id, :root, :started, :user

        def initialize(id, name, request_method, request_path, machine_name, root_name, user)
          @id = id
          @name = name
          @started_at = (Time.now.to_f * 1000).to_i
          @started = TimerStruct.now_ms
          @machine_name = machine_name
          @request_method = request_method
          @request_path = request_path
          @user = user
          @root = Request.new(root_name, @started, nil)
          # Not a children Array on each timer: spinel cannot seed an Array of a class inside that class's own constructor.
          @timers = [@root]
          @sqls = [Sql.new("", 0.0, @started, @root, false)]
          @sqls.pop
          @sql_count = 0
          @cached_sql_count = 0
          @sql_ms = 0.0
          @sql_float = false
        end

        def duration_ms
          @root.duration_milliseconds
        end

        def add_child(parent, name)
          timer = Request.new(name, @started, parent)
          @timers << timer
          timer
        end

        def add_sql(parent, query, elapsed_ms, cached)
          @sqls << Sql.new(query, elapsed_ms, @started, parent, cached)
          parent.add_sql_ms(elapsed_ms)
          @sql_ms += elapsed_ms
          @sql_float = true
          @sql_count += 1
          @cached_sql_count += 1 if cached
          nil
        end

        def children_of(timer)
          @timers.select { |t| !t.parent.nil? && t.parent.equal?(timer) }
        end

        def sqls_of(timer)
          @sqls.select { |q| q.parent.equal?(timer) }
        end

        def children_ms(timer)
          sum = 0.0
          children_of(timer).each { |c| sum += c.duration_milliseconds }
          sum
        end

        def move_child(child, destination)
          child.parent = destination
          relevel(child, destination.depth + 1)
          nil
        end

        def relevel(timer, depth)
          timer.adjust_depth(depth)
          children_of(timer).each { |c| relevel(c, depth + 1) }
          nil
        end

        def move_sql(sql, destination)
          source = sql.parent
          source.sql_timings_duration_milliseconds -= sql.duration_milliseconds
          source.sql_float = true
          destination.add_sql_ms(sql.duration_milliseconds)
          sql.parent = destination
          nil
        end

        def to_json
          "{\"id\":" + TimerStruct.json_string(@id) +
            ",\"name\":" + TimerStruct.json_string(@name) +
            ",\"started\":" + @started.to_s +
            ",\"started_at\":" + @started_at.to_s +
            ",\"machine_name\":" + TimerStruct.json_string(@machine_name) +
            ",\"level\":0,\"user\":" + TimerStruct.json_string(@user) +
            "," + UNVIEWED +
            ",\"duration_milliseconds\":" + @root.duration_milliseconds.to_s +
            ",\"has_trivial_timings\":true,\"has_all_trivial_timings\":false" +
            ",\"trivial_duration_threshold_milliseconds\":2,\"head\":null,\"sql_count\":" + @sql_count.to_s +
            ",\"cached_sql_count\":" + @cached_sql_count.to_s +
            ",\"duration_milliseconds_in_sql\":" + TimerStruct.number(@sql_ms, @sql_float) + ",\"has_sql_timings\":true" +
            ",\"has_duplicate_sql_timings\":false,\"executed_readers\":0,\"executed_scalars\":0" +
            ",\"executed_non_queries\":0,\"custom_timing_names\":[],\"custom_timing_stats\":{}" +
            ",\"custom_fields\":{},\"has_flamegraph\":false" +
            ",\"request_method\":" + TimerStruct.json_string(@request_method) +
            ",\"request_path\":" + TimerStruct.json_string(@request_path) +
            ",\"root\":" + @root.to_json(self) +
            ",\"started_formatted\":\"/Date(" + @started_at.to_s + ")/\"}"
        end

        def self.viewed?(json)
          !json.include?(UNVIEWED)
        end

        def self.viewed(json, client_timings)
          # Not `sub` with a String: it reads the replacement's backslashes as references, and the client timings are request data.
          at = json.index(UNVIEWED)
          return json if at.nil?
          json[0, at] + "\"has_user_viewed\":true,\"client_timings\":" + client_timings + json[at + UNVIEWED.length, json.length - at - UNVIEWED.length]
        end

        # Page#duration_ms and #name read back from the stored JSON, for the share page.
        def self.read_name(json)
          read_string(json, "\"name\":")
        end

        def self.read_duration(json)
          key = "\"duration_milliseconds\":"
          at = json.index(key)
          return 0.0 if at.nil?
          from = at + key.length
          stop = json.index(",", from)
          return 0.0 if stop.nil?
          json[from, stop - from].to_f
        end

        def self.read_string(json, key)
          at = json.index(key)
          return "" if at.nil?
          i = at + key.length + 1
          out = +""
          while i < json.length && json[i] != "\""
            if json[i] == "\\"
              i += 1
              c = json[i]
              out << (c == "n" ? "\n" : c == "t" ? "\t" : c == "r" ? "\r" : c)
            else
              out << json[i]
            end
            i += 1
          end
          out
        end
      end

      class Client
        def self.from_form(fields)
          base = fields.fetch("clientPerformance[timing][navigationStart]", "").to_i
          timing = [""]
          timing.pop
          navigation = false
          fields.each do |k, _v|
            if k.start_with?("clientPerformance[timing][") && k.end_with?("]")
              timing << k["clientPerformance[timing][".length, k.length - "clientPerformance[timing][".length - 1]
            end
            navigation = true if k.start_with?("clientPerformance[navigation][")
          end
          return "null" if timing.empty?
          # The gem reads `form['clientPerformance']['navigation']['redirect_count']` unguarded, so a form without it is a 500 there too.
          raise NoMethodError, "undefined method '[]' for nil" unless navigation
          out = [""]
          out.pop
          timing.each do |k|
            next unless k.end_with?("Start")
            start = fields["clientPerformance[timing][" + k + "]"].to_i - base
            finish = fields.fetch("clientPerformance[timing][" + k.sub(/Start\z/, "End") + "]", "").to_i - base
            duration = finish > start ? finish - start : 0
            out << timing_json(Client.words(k.sub(/Start\z/, "")), start, duration) if start >= 0
          end
          timing.each do |k|
            next if k.end_with?("Start") || k.end_with?("End")
            out << timing_json(k, fields["clientPerformance[timing][" + k + "]"].to_i - base, -1)
          end
          redirect = fields.key?("clientPerformance[navigation][redirect_count]") ? TimerStruct.json_string(fields["clientPerformance[navigation][redirect_count]"]) : "null"
          "{\"redirect_count\":" + redirect + ",\"timings\":[" + out.join(",") + "]}"
        end

        # `split(/(?=[A-Z])/)` then capitalize: "domContentLoadedEvent" is "Dom Content Loaded Event".
        def self.words(name)
          out = +""
          i = 0
          while i < name.length
            c = name[i]
            if i == 0
              out << c.upcase
            elsif c >= "A" && c <= "Z"
              out << " " << c
            else
              out << c
            end
            i += 1
          end
          out
        end

        def self.timing_json(name, start, duration)
          "{\"Name\":" + TimerStruct.json_string(name) + ",\"Start\":" + start.to_s + ",\"Duration\":" + duration.to_s + "}"
        end
      end
    end
  end
end
