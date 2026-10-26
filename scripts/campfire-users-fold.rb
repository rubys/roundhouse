# scripts/campfire-users-fold.rb OUT LANE... — folds bench-campfire's users
# phase into OUT/users.json: each lane's `loadgen users` result, with the
# peak PSS/RSS its server reached in each step's window and the cores its
# process tree used there (the samples are unix-ms stamped; so are the
# loadgen's PHASE lines).
require "json"

# Cable::Stats' bucket bounds in ms (runtime/spinel/scaffold/ruby_overlay/cable.rb).
CABLE_BOUNDS = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000].freeze

out, *lanes = ARGV
fold = {}
lanes.each do |lane|
  path = File.join(out, "users-#{lane}.json")
  next unless File.exist?(path) && !File.zero?(path)
  result = JSON.parse(File.read(path))
  log = File.join(out, "logs", "users-loadgen-#{lane}.log")
  phases = File.exist?(log) ? File.readlines(log).grep(/\APHASE /).map { |l| l.split.then { |p| [p[1..-2].join(" "), p[-1].to_i] } } : []
  mem = File.exist?(File.join(out, "users-memory-#{lane}.tsv")) ?
    File.readlines(File.join(out, "users-memory-#{lane}.tsv")).map { |l| l.split("\t").map(&:to_i) } : []
  threads_tsv = File.join(out, "users-threads-#{lane}.tsv")
  threads = File.exist?(threads_tsv) ?
    File.readlines(threads_tsv, chomp: true).map { |l| t, pid, tid, ticks, process, thread = l.split("\t", 6); [t.to_i, pid.to_i, tid.to_i, ticks.to_i, process.to_s, thread.to_s] } : []
  # Cable::Stats lines from the lane's own log: [unix ms, stage, bucket counts].
  lane_log = File.join(out, "logs", "users-#{lane}.log")
  cable = File.exist?(lane_log) ?
    File.foreach(lane_log).grep(/\A\[roundhouse stats\] /).map { |l| f = l.chomp.split(/[ \t]/); [f[2].to_i, f[4], f[4] == "major_by" ? f[5..] : f[5..].map(&:to_i)] } : []
  starts = phases.each_with_index.select { |(name, _), _| name.start_with?("step ") && !name.include?("warmup") }
  result["steps"].each_with_index do |step, i|
    start = starts[i] or next
    idx = start[1]
    from = phases[idx][1]
    to = phases[idx + 1]&.at(1) || Float::INFINITY
    window = mem.select { |t, _, _| t >= from && t < to }
    step["memory"] = { "peak_pss_mb" => (window.map { |r| r[1] }.max.to_i / 1024.0).round(1),
                       "peak_rss_mb" => (window.map { |r| r[2] }.max.to_i / 1024.0).round(1) } unless window.empty?
    # Cores the server's process tree used over the window (USER_HZ ticks).
    timed = window.select { |r| r.length > 3 }
    if timed.length >= 2 && timed.last[0] > timed.first[0]
      step["server_cpu_cores"] = ((timed.last[3] - timed.first[3]) / 100.0 / ((timed.last[0] - timed.first[0]) / 1000.0)).round(2)
    end
    # Cable::Stats' stages (cable.rb): for each histogram, count,
    # bucket-bound p50/p99 (ms; nil past the last bound) and how many took
    # longer than 50 ms and than the client's 500 ms subscribe re-send.
    # GC is summed over the workers: ms of GC per second, collections.
    cwin = cable.select { |t, _, _| t >= from && t < to }
    unless cwin.empty?
      secs = [(to.finite? ? to : cwin.map(&:first).max) - from, 1].max / 1000.0
      step["stages_ms"] = cwin.group_by { |_, stage, _| stage }.to_h do |stage, rows|
        if stage == "major_by"
          next [stage, rows.flat_map(&:last).each_with_object(Hash.new(0)) { |pair, h| r, n = pair.split(":"); h[r] += n.to_i }]
        end
        sums = rows.map(&:last).transpose.map(&:sum)
        if stage == "gc"
          next [stage, { "ms_per_s" => (sums[0] / secs).round(1), "major" => sums[1], "minor" => sums[2] }]
        end
        n = sums.sum
        at = ->(q) { i = 0; acc = 0; (acc += sums[i]; i += 1) while acc < q * n; CABLE_BOUNDS[i - 1] }
        over = ->(ms) { sums[(CABLE_BOUNDS.index(ms) + 1)..].sum }
        [stage, { "n" => n, "p50_le" => at.(0.5), "p99_le" => at.(0.99), "over_50" => over.(50), "over_500" => over.(500) }]
      end
    end
    # The same cores by process and by thread (campfire-users-threads.rb).
    # Digits are dropped from names so six workers, or four cable workers,
    # add up to one row.
    tsamples = threads.select { |r| r[0] >= from && r[0] < to }
    unless tsamples.empty?
      first_t, last_t = tsamples.map(&:first).minmax
      secs = (last_t - first_t) / 1000.0
      if secs > 0
        by_process = Hash.new(0.0)
        by_thread = Hash.new(0.0)
        tsamples.group_by { |r| r[2] }.each_value do |rows|
          a, b = rows.minmax_by(&:first)
          used = (b[3] - a[3]) / 100.0 / secs
          process = a[4].gsub(/\d+/, "N")
          by_process[process] += used
          by_thread["#{process} / #{a[5].gsub(/\d+/, "N")}"] += used
        end
        step["server_cpu_by_process"] = by_process.sort_by { |_, c| -c }.to_h.transform_values { |c| c.round(2) }
        step["server_cpu_by_thread"] = by_thread.sort_by { |_, c| -c }.reject { |_, c| c < 0.01 }.to_h.transform_values { |c| c.round(2) }
      end
    end
  end
  fold[lane] = result
end
File.write(File.join(out, "users.json"), JSON.pretty_generate(fold) + "\n") unless fold.empty?
