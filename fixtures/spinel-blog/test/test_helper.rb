require "minitest/autorun"

ROOT = File.expand_path("..", __dir__)
$LOAD_PATH.unshift(File.join(ROOT, "runtime"))
$LOAD_PATH.unshift(File.join(ROOT, "app"))
$LOAD_PATH.unshift(File.join(ROOT, "config"))
