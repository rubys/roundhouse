# scripts/campfire-users-threads.rb PID [INTERVAL] — samples the CPU of
# every thread in PID's process tree, for bench-campfire's users phase.
#
# One line per thread per sample, tab-separated:
#
#   unix-ms  pid  tid  ticks(user+system, USER_HZ)  process  thread
#
# `process` is the process title (Falcon's container names its hub and
# checkpointer; workers carry the server's own title) and `thread` is the
# kernel's comm, which Ruby sets from Thread#name ("cable-reactor",
# "cable-worker-0", ...). campfire-users-fold.rb turns consecutive samples
# into cores per process and per thread for each step. One long-lived
# process rather than a shell loop: forking awk per thread every two
# seconds would spend the generator's cores on the sampling.
pid = Integer(ARGV.fetch(0))
interval = Float(ARGV.fetch(1, "2"))
$stdout.sync = true

def children(pid)
  Dir.glob("/proc/#{pid}/task/*/children").flat_map { |f| File.read(f).split.map(&:to_i) }
rescue SystemCallError
  []
end

def tree(root)
  pids = [root]
  stack = [root]
  until stack.empty?
    kids = children(stack.pop)
    pids.concat(kids)
    stack.concat(kids)
  end
  pids
end

def title(pid)
  File.read("/proc/#{pid}/cmdline").tr("\0", " ").strip.tr("\t\n", "  ")[0, 80]
rescue SystemCallError
  "?"
end

while File.exist?("/proc/#{pid}")
  now = Process.clock_gettime(Process::CLOCK_REALTIME, :millisecond)
  tree(pid).each do |p|
    name = title(p)
    Dir.glob("/proc/#{p}/task/*").each do |task|
      stat = File.read("#{task}/stat")
      comm = stat[/\((.*)\)/m, 1].to_s.tr("\t\n", "  ")
      fields = stat.sub(/.*\) /m, "").split
      puts [now, p, File.basename(task), fields[11].to_i + fields[12].to_i, name, comm].join("\t")
    rescue SystemCallError
      nil # the thread exited between the listing and the read
    end
  end
  sleep interval
end
