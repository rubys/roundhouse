#!/usr/bin/env python3
"""Select coverage, not test results.

Extra-language SDKs need a path owner, `ci:full`, or scheduled full
validation. Unknown inputs keep the Ruby floor plus Spinel.
"""

import argparse
import json
import os
import re
import subprocess
from pathlib import Path

TARGETS = [
    "rust",
    "crystal",
    "kotlin",
    "swift",
    "csharp",
    "typescript",
    "go",
    "elixir",
    "python",
    "ruby",
    "jruby",
]
# PR floor: the Ruby shape plus Campfire. Extra languages, Rust/TS
# compare, WASM, and Spinel are not in this list.
BASE = [
    "generate-fixture",
    "unit",
    "build-roundhouse",
    "store-check",
    "compare-ruby",
    "campfire-conformance",
    "campfire-compare",
]
# Compact publication additionally waits on Rust/TS when those jobs were
# selected (scheduled full, or a change that owns them). Unselected skips
# must not fail the Ruby PR floor.
PUBLICATION = [*BASE, "compare", "browser-smoke-typescript"]
CORE = ["build-spinel", "toolchain-spinel", "compare-spinel"]
PARAM_BIND_TESTS = ["param_binds", "param_binds_values", "param_binds_planner", "param_binds_cleanup"]
SPINEL_TESTS = [
    "date_columns_spinel",
    "framework_tests_spinel",
    "spinel_web_push_crypto",
    "spinel_db_lease",
    *PARAM_BIND_TESTS,
    "spinel_stmt_cache_lru",
    "db_sqlite_concurrency",
    "spinel_param_builder",
    "rails_compat_vectors_spinel",
]
SPINEL11 = [
    "build-spinel",
    "framework-tests-spinel",
    "build-campfire-compare-spinel",
    "campfire-compare-spinel",
    "campfire-db-differential-spinel",
    "toolchain-spinel",
    "compare-spinel",
    "smoke-spinel",
    "build-campfire-archive",
    "smoke-campfire",
    "smoke-campfire-docker",
]
# Opt-in Spinel lane (`ci:spinel`): Ruby floor plus the full Spinel suite,
# without other-language emitters, WASM, or Writebook. smoke-spinel needs
# build-site; archive-results closes packaging evidence.
SPINEL_LANE = [*BASE, *SPINEL11, "build-site", "archive-results"]
ADVISORY = set(SPINEL11) - {"build-campfire-archive"}
# Packaging evidence report only. Selected for completeness and required by
# assemble-site when publishing; never a hard CI-summary / compact failure.
# GitHub can mark the job `abandoned` (queued, never assigned) on large
# ci:full PR graphs even when every producer succeeded — that must not red
# an otherwise green PR summary.
REPORTING = {"archive-results"}
SHA = re.compile(r"[0-9a-f]{40}\Z")
PROJECT_BUILDERS = {
    "ruby_runtime_files": "interpreted",
    "jruby_runtime_files": "interpreted",
    "ruby_family_runtime_files": "interpreted",
    "spinel_files": "ruby-family",
    "spin_shape": "ruby-family",
}


def native_coverage(path):
    """Identify native core/focused suites and interpreter-only exceptions."""
    interpreter_only = path.startswith(
        "runtime/spinel/scaffold/ruby_overlay/"
    ) or path in {
        "runtime/spinel/db_jruby.rb",
        "runtime/spinel/markly_jruby.rb",
        "runtime/spinel/db_cruby.rb",
        "runtime/spinel/message_digest_cruby.rb",
        "runtime/spinel/module_delegate.rb",
    }
    native = (
        path.startswith(("runtime/ruby/", "runtime/spinel/", "src/emit/ruby/"))
        or path == "src/emit/ruby.rs"
        or (path.startswith("tests/spinel") and path.endswith((".rs", ".rb")))
        or path in {f"tests/{name}.rs" for name in SPINEL_TESTS}
        or path == "tests/support/db_concurrency_spinel.rb"
    ) and not interpreter_only
    suites = set()
    if path.startswith("runtime/ruby/") and path.endswith((".rb", ".rbs")):
        suites.add("framework_tests_spinel")
    focused = re.fullmatch(r"tests/([^/]+)\.(?:rs|rb)", path)
    if focused and focused[1] in SPINEL_TESTS:
        suites.add(focused[1])
    if path == "tests/support/db_concurrency_spinel.rb":
        suites.add("db_sqlite_concurrency")
    # Gate drivers stay flat beside their Rust harness. Match the most
    # specific suite first (e.g. param_binds_values before param_binds).
    if path == "tests/param_binds_text_cleanup.rb":
        suites.add("param_binds_cleanup")
    elif path.startswith("tests/") and path.endswith(".rb"):
        stem = path[len("tests/"):-len(".rb")]
        for suite in reversed(PARAM_BIND_TESTS):
            if stem == suite or stem.startswith(suite + "_"):
                suites.add(suite)
                break
    if path == "runtime/spinel/test/statement_cache_cases.rb":
        suites.add("param_binds")
    if path in {
        "tests/support/emit_and_run.rs",
        "src/lower/model_to_library/adapter_emit.rs",
        "src/emit/ruby/library.rs",
    } or path.startswith(("src/lower/arel/", "src/lower/model_to_library/adapter_emit/")):
        suites.update(PARAM_BIND_TESTS)
    if path.startswith(("runtime/spinel/", "runtime/ruby/")) and not interpreter_only:
        name = path.rsplit("/", 1)[-1]
        owned_tests = set()
        if name == "statement_cache_cases.rb":
            owned_tests.add("param_binds")
        if any(word in name for word in ("web_push", "base64")):
            owned_tests.add("spinel_web_push_crypto")
        if any(
            word in name
            for word in (
                "signed_cookie",
                "message_verifier",
                "signed_id",
                "message_digest",
                "base64",
            )
        ) or path.startswith("runtime/spinel/tep/url."):
            owned_tests.add("rails_compat_vectors_spinel")
        if any(
            word in path for word in ("/db", "sqlite", "active_support_time_parsing")
        ):
            # Shared database inputs own lease/ownership, binds, cache recency,
            # and the snapshot / write-permit / checkpoint policy.
            owned_tests.update(
                (
                    "spinel_db_lease",
                    *PARAM_BIND_TESTS,
                    "spinel_stmt_cache_lru",
                    "db_sqlite_concurrency",
                )
            )
        if any(word in name for word in ("param", "multipart", "request")):
            owned_tests.add("spinel_param_builder")
        if name in {
            "date.rb",
            "date.rbs",
            "active_support_date_parsing.rb",
            "active_support_date_parsing.rbs",
            "active_record_date_serialization.rb",
            "active_record_date_serialization.rbs",
            "sqlite_adapter.rb",
        }:
            owned_tests.add("date_columns_spinel")
        if name in {
            "active_record_date_serialization.rb",
            "active_record_serialization.rb",
        }:
            owned_tests.add("framework_tests_spinel")
        if (
            path.startswith("runtime/spinel/")
            and not path.startswith("runtime/spinel/scaffold/")
            and not owned_tests
        ):
            owned_tests.add("framework_tests_spinel")
        suites.update(owned_tests)
    if (
        path.startswith(("tests/rails_compat/", "tests/params_vectors/"))
        or path == "tests/rails_compat_vectors.rb"
    ):
        suites.add(
            "spinel_param_builder"
            if path.startswith("tests/params_vectors/")
            else "rails_compat_vectors_spinel"
        )
    return native, interpreter_only, suites


def archive_and_campfire_jobs(path, interpreter_only):
    """Select packaging and Campfire consumers, not every native runtime edit."""
    jobs = set()
    if path.startswith("runtime/spinel/scaffold/") and not interpreter_only:
        jobs.update((*CORE, "smoke-spinel", "build-site"))
    if path.startswith(("scripts/campfire-compare", "scripts/build-campfire-compare")):
        jobs.update(
            ("build-spinel", "build-campfire-compare-spinel", "campfire-compare-spinel")
        )
    if path.startswith("scripts/campfire-db-differential"):
        jobs.update(("build-spinel", "campfire-db-differential-spinel"))
    campfire_archive = (
        path.startswith(
            (
                "scripts/build-campfire-archive",
                "scripts/campfire-archive",
                "e2e/campfire/",
            )
        )
        or path == "scripts/campfire-docker-files"
    )
    shared_smoke = (
        path.startswith("e2e/") and not path.startswith("e2e/campfire/")
    ) or path in {"scripts/smoke", "scripts/ci-playwright-install"}
    if campfire_archive or shared_smoke:
        jobs.update(
            (
                "build-spinel",
                "build-campfire-archive",
                "smoke-campfire",
                "smoke-campfire-docker",
            )
        )
    if shared_smoke:
        jobs.add("smoke-spinel")
    return jobs


def select(
    paths,
    *,
    full=False,
    spinel_lane=False,
    publish=False,
    project_scope=None,
):
    if spinel_lane and not full:
        return finish(
            SPINEL_LANE,
            [],
            [],
            False,
            False,
            True,
            ["ci:spinel: Ruby floor plus Spinel suite"],
            spinel_tests=list(SPINEL_TESTS),
        )
    targets, smoke = set(), set()
    jobs_selected, spinel_tests = set(), set()
    wasm = site = spinel = writebook = False
    reasons = []
    for path in paths:
        if path == "src/project.rs" and project_scope in PROJECT_BUILDERS.values():
            targets.update(("ruby", "jruby"))
            smoke.update(("ruby", "jruby"))
            writebook = True
            if project_scope == "ruby-family":
                spinel = True
                jobs_selected.update(SPINEL11)
                spinel_tests.update(SPINEL_TESTS)
            reasons.append(f"{path}: proven {project_scope} assembly bodies only")
            continue
        if path in {
            "tests/support/jdbc_cleanup_failures.rb",
            "runtime/spinel/test/statement_cache_cases.rb",
        }:
            targets.add("jruby")
            reasons.append(f"{path}: JDBC statement lifecycle")
        match = re.match(r"(?:src/emit/|runtime/)([^/.]+)(?:[/.]|$)", path)
        test = re.match(
            r"tests/(?:framework_tests_)?([a-z]+)_toolchain\.rs$|tests/framework_tests_([a-z]+)\.rs$",
            path,
        )
        target = (
            match[1]
            if match
            else next((v for v in test.groups() if v), None)
            if test
            else None
        )
        native, interpreter_only, owned_tests = native_coverage(path)
        spinel_tests.update(owned_tests)
        if native or owned_tests:
            spinel = True
            jobs_selected.update(CORE)
        if native:
            reasons.append(f"{path}: native Spinel core")
        if target in TARGETS or target == "spinel":
            owners = (
                {"ruby", "jruby", "spinel"}
                if target in {"ruby", "spinel"} and not path.startswith("runtime/ruby/")
                else {target}
            )
            if (
                path.startswith(("runtime/ruby/", "runtime/spinel/"))
                or (path.startswith("tests/spinel") and path.endswith(".rs"))
                or path in {f"tests/{name}.rs" for name in SPINEL_TESTS}
            ):
                owners = set()  # Native framework coverage; no interpreted archives.
            if interpreter_only:
                owners = (
                    {"jruby"}
                    if path.endswith(("db_jruby.rb", "markly_jruby.rb"))
                    else {"ruby", "jruby"}
                )
            targets.update(owners - {"spinel"})
            smoke.update(owners - {"spinel"})
            spinel |= "spinel" in owners
            if owners:
                reasons.append(f"{path}: {', '.join(sorted(owners))}")
        elif target == "shared":
            full = True
            reasons.append(f"{path}: shared code generation")
        if path.startswith("wasm/"):
            wasm = True
            reasons.append(f"{path}: WASM/browser compiler")
        if path.startswith(("site/", "docs/guide/")):
            site = wasm = True
        if (
            path.startswith("e2e/") and not path.startswith("e2e/campfire/")
        ) or path in {
            "scripts/smoke",
            "scripts/ci-playwright-install",
            "scripts/create-blog",
            "scripts/create-store",
            "bin/rh",
        }:
            smoke.update(TARGETS)
            spinel = True
        if (
            path.startswith(("tools/compare/", "tests/framework_test_support"))
            or path == "scripts/compare"
        ):
            targets.update(TARGETS)
            spinel = True
            jobs_selected.update(CORE)
            if path.startswith("tests/framework_test_support"):
                spinel_tests.add("framework_tests_spinel")
        archive_jobs = archive_and_campfire_jobs(path, interpreter_only)
        if archive_jobs:
            spinel = True
            jobs_selected.update(archive_jobs)
        if path in {"tests/writebook.rs", "tests/fixtures/writebook-inventory.json"}:
            writebook = True
    if full:
        targets.update(TARGETS)
        smoke.update(TARGETS)
        wasm = site = spinel = writebook = True
        reasons.append("full validation requested")
        jobs_selected.update(SPINEL11)
        spinel_tests.update(SPINEL_TESTS)
    if spinel:
        jobs_selected.add("build-spinel")
    jobs = list(BASE)
    extra = [
        t
        for t in TARGETS
        if t in targets and t not in {"rust", "typescript", "ruby", "jruby"}
    ]
    if "rust" in targets or "typescript" in targets:
        jobs.append("compare")
    if extra:
        jobs.append("compare-extra")
    if "jruby" in targets:
        jobs.append("compare-jruby")
    if wasm or "typescript" in targets:
        jobs.append("browser-smoke-typescript")
    if wasm:
        jobs.extend(["build-wasm", "browser-smoke-ide"])
    if smoke or site:
        jobs.append("build-site")
    if smoke - {"spinel"}:
        jobs.append("smoke")
    if spinel_tests:
        jobs_selected.add("framework-tests-spinel")
    if "smoke-spinel" in jobs_selected:
        jobs_selected.add("build-site")
    if "build-site" in jobs_selected and "build-site" not in jobs:
        jobs.append("build-site")
    if "build-site" in jobs or {"build-site", "build-campfire-archive"} & jobs_selected:
        jobs_selected.add("archive-results")
    jobs.extend(j for j in [*SPINEL11, "archive-results"] if j in jobs_selected)
    if writebook:
        jobs.append("writebook-inventory")
    if publish:
        if not full:
            raise ValueError("publication requires full validation mode")
        jobs.append("assemble-site")
    return finish(
        jobs,
        extra,
        [t for t in TARGETS if t in smoke],
        wasm,
        site,
        spinel,
        reasons,
        publish,
        [t for t in SPINEL_TESTS if t in spinel_tests],
    )


def finish(
    jobs, extra, smoke, wasm, site, spinel, reasons, publish=False, spinel_tests=None
):
    archives = (
        ["blog", "spinel", *TARGETS, "typescript-worker"]
        if site
        else [*smoke, *(["spinel"] if "smoke-spinel" in jobs else [])]
    )
    return {
        "jobs": jobs,
        "required": [j for j in jobs if j not in ADVISORY and j not in REPORTING],
        "extra_compare": extra,
        "smoke": smoke,
        "archives": archives,
        "wasm": wasm,
        "site": site,
        "spinel": spinel,
        "spinel_tests": spinel_tests or [],
        "publish": publish,
        "reasons": reasons,
    }


def git(*args):
    return subprocess.check_output(["git", *args])


def ensure_commit(sha):
    """Fetch a commit by SHA when the plan checkout is too shallow to see it."""
    try:
        git("cat-file", "-e", f"{sha}^{{commit}}")
    except subprocess.CalledProcessError:
        git("fetch", "--no-tags", "--depth=1", "origin", sha)
        git("cat-file", "-e", f"{sha}^{{commit}}")


def project_change_scope(before, after):
    """Narrow only body-only edits in known builders; all other bytes must match.

    This is not a Rust parser. Only indented bodies without raw strings or
    block comments qualify; unknown shapes/signatures/items do not narrow
    and stay on the Ruby floor.
    """
    pattern = re.compile(
        r"(?P<header>^fn (?P<name>"
        + "|".join(sorted(PROJECT_BUILDERS))
        + r")\([^{};]*\{\n)(?P<body>(?:[ \t]+[^\n]*\n|\n)*)^}\n",
        re.MULTILINE,
    )
    # Exclude function-looking text in Rust literals/comments. Block comments
    # (including nested ones) are deliberately unsupported, not half-parsed.
    literals = re.compile(
        r'//[^\n]*|\b[bc]?r(?P<hash>#+)"[\s\S]*?"(?P=hash)'
        r'|"(?:\\[\s\S]|[^"\\])*"'
        r"|'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'"
        r"|/\*"
    )
    bodies = []
    skeletons = []
    for source in (before, after):
        found = {}
        excluded = list(literals.finditer(source))
        if any(token[0] == "/*" for token in excluded):
            return None
        lexical = literals.sub(lambda token: re.sub(r"[^\n]", " ", token[0]), source)

        def mask(match):
            if any(token.start() <= match.start() < token.end() for token in excluded):
                return match[0]
            prefix = lexical[: match.start()]
            if any(
                prefix.count(left) != prefix.count(right)
                for left, right in [("{", "}"), ("(", ")"), ("[", "]")]
            ):
                return match[0]  # Nested/macro input is not a top-level builder.
            name, body = match["name"], match["body"]
            code = "\n".join(
                line for line in body.splitlines() if not line.lstrip().startswith("//")
            )
            if name in found or re.search(r'(?<!\w)[bc]?r#*"|/\*', code):
                raise ValueError("ambiguous project assembly body")
            found[name] = body
            return match["header"] + "}\n"

        try:
            skeletons.append(pattern.sub(mask, source))
        except ValueError:
            return None
        bodies.append(found)
    if skeletons[0] != skeletons[1] or bodies[0].keys() != bodies[1].keys():
        return None
    changed = {name for name in bodies[0] if bodies[0][name] != bodies[1][name]}
    if changed:
        scopes = {PROJECT_BUILDERS[name] for name in changed}
        return "ruby-family" if "ruby-family" in scopes else "interpreted"
    return None


def changed_inputs(event, event_name, sha, *, need_project_scope=True):
    if not SHA.fullmatch(sha) or git("rev-parse", "HEAD").decode().strip() != sha:
        raise ValueError("checkout is not the event SHA")
    if event_name == "pull_request":
        pr = event["pull_request"]
        base, head = pr["base"]["sha"], pr["head"]["sha"]
        if not SHA.fullmatch(base) or not SHA.fullmatch(head):
            raise ValueError("PR event is missing base/head SHAs")
        parents = git("show", "-s", "--format=%P", "HEAD").decode().split()
        # Prefer the merge commit's first parent when this is the PR merge
        # tree: GitHub's merge ref can land on a newer main than the event's
        # base.sha. Do not fetch the fork head from origin; it is already a
        # parent of the merge commit, or HEAD itself.
        if len(parents) == 2 and parents[1] == head:
            base = parents[0]
        elif sha != head:
            raise ValueError("checkout is not the event's PR merge tree or head")
        ensure_commit(base)
    elif event_name == "push":
        base = event["before"]
        if not SHA.fullmatch(base) or base == "0" * 40:
            raise ValueError("no previous main tree")
        ensure_commit(base)
    else:
        return [], None
    # Renames become a deletion and addition; both ownership sets are selected.
    paths = [
        p.decode("utf-8")
        for p in git("diff", "--name-only", "--no-renames", "-z", base, sha).split(
            b"\0"
        )
        if p
    ]
    scope = None
    if need_project_scope and "src/project.rs" in paths:
        entries = [
            git("ls-tree", ref, "--", "src/project.rs").split() for ref in (base, sha)
        ]
        if all(entry and entry[0] == b"100644" for entry in entries):
            scope = project_change_scope(
                git("show", f"{base}:src/project.rs").decode("utf-8"),
                git("show", f"{sha}:src/project.rs").decode("utf-8"),
            )
    return paths, scope


def check_results(plan, needs, *, compact=False):
    if compact:
        # Compact gate only observes the publication floor jobs in its needs
        # graph. Spinel/full extras are enforced by ci-summary, not here.
        required = [job for job in PUBLICATION if job in plan["jobs"]]
    else:
        required = plan["required"]
    failures = [
        f"{j}: {needs.get(j, {}).get('result', 'missing')}"
        for j in required
        if needs.get(j, {}).get("result") != "success"
    ]
    if needs.get("plan", {}).get("result") != "success":
        failures.append("plan: no successful routing decision")
    if (
        not compact
        and needs.get("compact-required", {}).get("result") != "success"
    ):
        failures.append("compact-required: no successful baseline gate")
    # Advisory work never blocks the gate, but incomplete work is not complete.
    # Compact only claims completeness for the publication floor it can see.
    tracked = required if compact else plan["jobs"]
    complete = not failures and all(
        needs.get(j, {}).get("result") == "success"
        and (
            j not in ADVISORY
            or all(
                needs[j].get("outputs", {}).get(key) == "success"
                for key in (
                    ["default", "minor-gc", "verify-gen"]
                    if j == "campfire-compare-spinel"
                    else ["execution"]
                )
            )
        )
        for j in tracked
    )
    return failures, complete


def write_outputs(values):
    output = os.environ.get("GITHUB_OUTPUT")
    lines = "".join(
        f"{key}={json.dumps(value, separators=(',', ':')) if not isinstance(value, str) else value}\n"
        for key, value in values.items()
    )
    if output:
        with open(output, "a") as f:
            f.write(lines)
    print(lines, end="")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["plan", "gate", "compact-gate"])
    args = parser.parse_args()
    if args.command != "plan":
        raw_plan = os.environ.get("CI_PLAN", "")
        if not raw_plan.strip():
            # Plan cancelled/skipped leaves an empty output; do not crash the
            # gates or claim a green floor.
            write_outputs({"complete": False})
            print("::notice::No plan output (cancelled or skipped); incomplete")
            return True
        plan = json.loads(raw_plan)
        failures, complete = check_results(
            plan,
            json.loads(os.environ["CI_NEEDS"]),
            compact=args.command == "compact-gate",
        )
        write_outputs({"complete": complete})
        for failure in failures:
            print(f"::error::{failure}")
        return bool(failures)
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    event_name = os.environ["GITHUB_EVENT_NAME"]
    pr = event.get("pull_request", {})
    labels = {label["name"] for label in pr.get("labels", [])}
    full = os.environ.get("CI_FULL") == "true" or "ci:full" in labels
    spinel_lane = "ci:spinel" in labels
    if (
        event_name == "push"
        and os.environ.get("GITHUB_REF") == "refs/heads/main"
        and not pr
        and not full
    ):
        # Extra-language SDKs are the scheduled full-ci ledger, not every merge.
        spinel_lane = True
    reason = None
    try:
        # project_scope only narrows path selection; spinel/full short-circuit
        # before that, so skip the expensive project.rs body scan there.
        paths, project_scope = changed_inputs(
            event,
            event_name,
            os.environ["GITHUB_SHA"],
            need_project_scope=not full and not spinel_lane,
        )
    except (KeyError, ValueError, UnicodeError, subprocess.CalledProcessError) as e:
        paths, project_scope = [], None
        if full:
            reason = f"Unknown changed inputs: {e}; running full validation"
        else:
            spinel_lane = True
            reason = (
                f"Unknown changed inputs: {e}; "
                "Ruby+Spinel only (extra-language SDKs not selected)"
            )
    publish = os.environ.get("CI_PUBLISH") == "true"
    if publish and (
        os.environ["GITHUB_REPOSITORY"] != "rubys/roundhouse"
        or os.environ["GITHUB_REF"] != "refs/heads/main"
        or event_name not in {"schedule", "workflow_dispatch"}
    ):
        raise ValueError("publication is only allowed by canonical main's full caller")
    plan = select(
        paths,
        full=full,
        spinel_lane=spinel_lane,
        publish=publish,
        project_scope=project_scope,
    )
    if reason:
        plan["reasons"].append(reason)
    spinel = os.environ.get("CI_SPINEL_REVISION", "")
    if plan["spinel"] and not spinel:
        try:
            spinel = subprocess.check_output(
                ["gh", "api", "repos/matz/spinel/commits/master", "--jq", ".sha"],
                text=True,
            ).strip()
        except subprocess.CalledProcessError:
            spinel = "master"
            plan["reasons"].append(
                "Spinel lookup unavailable: fresh master build; actual revision recorded by producer"
            )
    if spinel and spinel != "master" and not SHA.fullmatch(spinel):
        raise ValueError("invalid Spinel revision")
    write_outputs(
        {
            "plan": plan,
            "jobs": plan["jobs"],
            "extra-compare": plan["extra_compare"],
            "spinel-tests": plan["spinel_tests"],
            "smoke": plan["smoke"],
            "archives": ",".join(plan["archives"]),
            "wasm": plan["wasm"],
            "site": plan["site"],
            "publish": plan["publish"],
            "spinel-revision": spinel,
        }
    )
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        Path(summary).write_text(
            "## Selected CI coverage\n\n```json\n"
            + json.dumps(plan, indent=2)
            + "\n```\n"
        )
    return False


if __name__ == "__main__":
    raise SystemExit(main())
