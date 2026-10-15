#!/usr/bin/env python3
"""Compare two Roundhouse binaries on bounded, synthetic Rails applications.

Output equality and elapsed time are separate observations. This measures a
whole debug/release CLI, not just an index builder. Run both binaries with the
same build profile; use the unit tests for deterministic pass-wide build counts.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def write(root, path, content):
    dest = root / path
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(content)


def fixture(root, size):
    write(root, "app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n self.abstract_class = true\nend\n")
    write(root, "app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
    write(root, "config/routes.rb", 'Rails.application.routes.draw do\n get "/widgets", to: "widgets#index"\nend\n')
    write(root, "app/controllers/widgets_controller.rb", 'class WidgetsController < ApplicationController\n def index\n render plain: "ok"\n end\nend\n')
    tables = []
    for i in range(size):
        table = f"widgets_{i}"
        tables.append(f" create_table :{table} do |t|\n t.string :label\n end\n")
        write(root, f"app/models/widget_{i}.rb", f'class Widget{i} < ApplicationRecord\n self.table_name = "{table}"\n def dirty_label; label_changed?; end\n def quiet; nil; end\nend\n')
        write(root, f"app/services/catalog_{i}.rb", f'module Catalog{i}\n VALUE = "entry"\n class Reader\n COPY = Catalog{i}::VALUE\n def label; COPY; end\n end\nend\n')
    write(root, "db/schema.rb", "ActiveRecord::Schema.define do\n" + "".join(tables) + "end\n")


def snapshot(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(root.rglob("*")) if p.is_file()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--sizes", type=int, nargs="+", default=[16, 64, 128])
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--execute-ruby", action="store_true", help="also boot each emitted tree and verify model/constant reads")
    args = parser.parse_args()
    if not 1 <= args.rounds <= 10 or not all(1 <= n <= 1000 for n in args.sizes):
        parser.error("rounds must be 1..10 and sizes 1..1000")
    binaries = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    results = []
    with tempfile.TemporaryDirectory(prefix="lookup-index-bench-") as temp:
        temp = Path(temp)
        for size in args.sizes:
            source = temp / f"app-{size}"
            fixture(source, size)
            timings = {key: [] for key in binaries}
            reference = None
            for round_ in range(args.rounds):
                order = list(binaries) if round_ % 2 == 0 else list(reversed(binaries))
                for key in order:
                    # Identical output path avoids path-dependent emission changes.
                    out = temp / "out"
                    if out.exists():
                        import shutil
                        shutil.rmtree(out)
                    started = time.perf_counter()
                    run = subprocess.run([str(binaries[key]), "--target", "ruby", str(source), "-o", str(out)],
                                         env={**os.environ, "NO_COLOR": "1"}, text=True, capture_output=True)
                    elapsed = time.perf_counter() - started
                    if run.returncode:
                        raise RuntimeError(f"{key}, size={size}: {run.stdout}\n{run.stderr}")
                    current = snapshot(out)
                    if reference is None:
                        reference = current
                    if current != reference:
                        changed = sorted(p for p in current.keys() | reference.keys()
                                         if current.get(p) != reference.get(p))
                        raise AssertionError(f"non-identical emitted files: {changed[:20]}")
                    timings[key].append(elapsed)
                    if args.execute_ruby:
                        probe = ('require File.expand_path("main", Dir.pwd); '
                                 'Main.configure_default_adapter!; '
                                 'v = Widget0.new(label: "first"); '
                                 'p [v.label, v.dirty_label, Catalog0::Reader.new.label]')
                        execution = subprocess.run(["ruby", "-e", probe], cwd=out,
                                                   env={**os.environ, "BLOG_DB": ":memory:"},
                                                   text=True, capture_output=True)
                        if execution.returncode or execution.stdout != '["first", true, "entry"]\n':
                            raise AssertionError(f"{key} runtime probe: {execution.stdout}\n{execution.stderr}")
            result = {"models": size, "library_groups": size, "files": len(reference),
                      "ruby_probe_passed": args.execute_ruby,
                      "byte_identical": True, "seconds": timings,
                      "median_seconds": {key: statistics.median(value) for key, value in timings.items()}}
            results.append(result)
            print(json.dumps(result), flush=True)
    print(json.dumps({"results": results, "scope": "whole CLI; synthetic input; elapsed time is not an isolated index benchmark"}))


if __name__ == "__main__":
    main()
