#!/usr/bin/env python3
"""Update a deployed Pages artifact without rebuilding its generated downloads."""

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath

REPOSITORY = "rubys/roundhouse"
SCHEMA = 1
METADATA = "ci/site-publication.json"
SHA = re.compile(r"[0-9a-f]{40}\Z")
DIGEST = re.compile(r"sha256:([0-9a-f]{64})\Z")
ARTIFACT = re.compile(r"github-pages-([1-9][0-9]*)\Z")
WORKFLOWS = {".github/workflows/full-ci.yml", ".github/workflows/publish-site.yml"}
SURFACES = ("playground", "studio", "ide", "fractal")
BROWSER = {"lib": {".js", ".mjs"}, **{name: {".html", ".js"} for name in SURFACES}}
WASM_ROOTS = ("src/", "runtime/", "data/", "catalog/", "wasm/src/", "wasm/vendor/", ".cargo/", ".github/actions/build-wasm/", ".github/actions/setup-rust/")
WASM_FILES = {"Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml", "wasm/Cargo.toml", "wasm/Cargo.lock"}
MAX_SITE = 2 * 1024**3
SCRIPT_ROOT = Path(__file__).resolve().parent


def run(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, timeout=180)


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")


def publication_context(env):
    if (env.get("GITHUB_REPOSITORY"), env.get("GITHUB_REF"), env.get("GITHUB_EVENT_NAME")) != (REPOSITORY, "refs/heads/main", "workflow_dispatch"):
        raise ValueError("site publication requires a manual run on rubys/roundhouse main")
    source = env.get("GITHUB_SHA", "")
    if not SHA.fullmatch(source):
        raise ValueError("missing immutable source SHA")
    return source


class GitHub:
    def get(self, path):
        return json.loads(run("gh", "api", f"repos/{REPOSITORY}/{path}"))

    def pages(self, path, key=None):
        separator = "&" if "?" in path else "?"
        for page in range(1, 11):
            result = self.get(f"{path}{separator}per_page=100&page={page}")
            items = result[key] if key else result
            yield from items
            if len(items) < 100:
                return
        raise ValueError("publication lookup exceeded its pagination bound")

    def download(self, artifact, destination):
        with Path(destination).open("wb") as stream:
            subprocess.run(["gh", "api", f"repos/{REPOSITORY}/actions/artifacts/{artifact['id']}/zip"], stdout=stream, check=True, timeout=180)
        match = DIGEST.fullmatch(artifact.get("digest", ""))
        if not match or sha256(destination) != match[1]:
            raise ValueError("the deployed site artifact failed its SHA-256 check")


def deployment_run(status):
    match = re.fullmatch(rf"https://github\.com/{re.escape(REPOSITORY)}/actions/runs/([1-9][0-9]*)/job/([1-9][0-9]*)", status.get("log_url", ""))
    if status.get("state") != "success" or not match:
        raise ValueError("deployment has no successful canonical Actions job")
    return tuple(map(int, match.groups()))


def trusted_baseline(deployment, status, workflow, job, artifacts):
    run_id, job_id = deployment_run(status)
    if (
        deployment.get("environment") != "github-pages"
        or deployment.get("ref") not in {"main", "refs/heads/main"}
        or not SHA.fullmatch(deployment.get("sha", ""))
        or workflow.get("id") != run_id
        or workflow.get("repository", {}).get("full_name") != REPOSITORY
        or workflow.get("path") not in WORKFLOWS
        or workflow.get("head_branch") != "main"
        or workflow.get("head_sha") != deployment["sha"]
        or workflow.get("event") not in {"schedule", "workflow_dispatch"}
        or job.get("id") != job_id
        or job.get("run_id") != run_id
        or job.get("status") != "completed"
        or job.get("conclusion") != "success"
        or type(job.get("run_attempt")) is not int
        or job["run_attempt"] < 1
    ):
        raise ValueError("the deployed site does not have trusted canonical-main provenance")
    candidates = []
    for artifact in artifacts:
        match = ARTIFACT.fullmatch(artifact.get("name", ""))
        if match and int(match[1]) <= job["run_attempt"]:
            candidates.append((int(match[1]), artifact))
    if not candidates:
        raise ValueError("the latest deployed Pages artifact is missing; publish through Full validation once")
    attempt = max(item[0] for item in candidates)
    selected = [artifact for number, artifact in candidates if number == attempt]
    if len(selected) != 1:
        raise ValueError("the deployed Pages artifact is ambiguous")
    artifact = selected[0]
    if artifact.get("expired") is not False or not DIGEST.fullmatch(artifact.get("digest") or ""):
        raise ValueError("the latest deployed Pages artifact expired or has no digest; publish through Full validation once")
    if not 0 < artifact.get("size_in_bytes", 0) <= MAX_SITE:
        raise ValueError("invalid deployed site artifact size")
    return {
        "deployment_id": deployment["id"], "source_sha": deployment["sha"],
        "run_id": run_id, "attempt": job["run_attempt"], "workflow": workflow["path"],
        "artifact": artifact,
    }


def latest_baseline(api):
    for deployment in api.pages("deployments?environment=github-pages"):
        statuses = api.get(f"deployments/{deployment['id']}/statuses?per_page=1")
        if not statuses or statuses[0].get("state") != "success":
            continue
        status = statuses[0]
        run_id, job_id = deployment_run(status)
        return trusted_baseline(
            deployment, status, api.get(f"actions/runs/{run_id}"), api.get(f"actions/jobs/{job_id}"),
            api.pages(f"actions/runs/{run_id}/artifacts", "artifacts"),
        )
    raise ValueError("no successfully deployed site is available; publish through Full validation once")


def relative_path(value):
    path = PurePosixPath(value)
    if not value or path.is_absolute() or ".." in path.parts or "\\" in value:
        raise ValueError(f"unsafe site path: {value}")
    return path


def extract_site(bundle, root):
    root = Path(root)
    if root.exists() and (root.is_symlink() or not root.is_dir() or any(root.iterdir())):
        raise ValueError("restore requires a new or empty site directory")
    root.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(bundle) as archive, tempfile.TemporaryDirectory() as temp:
        entries = archive.infolist()
        if len(entries) != 1 or entries[0].filename != "artifact.tar" or not 0 < entries[0].file_size <= MAX_SITE or stat.S_ISLNK(entries[0].external_attr >> 16):
            raise ValueError("unexpected Pages artifact bundle")
        tar_path = Path(temp) / "artifact.tar"
        with archive.open(entries[0]) as source, tar_path.open("wb") as destination:
            shutil.copyfileobj(source, destination)
        with tarfile.open(tar_path) as tar:
            seen = set()
            size = 0
            for member in tar.getmembers():
                path = relative_path(member.name)
                if not path.parts and member.isdir():
                    continue
                if not path.parts or path in seen or not (member.isfile() or member.isdir()):
                    raise ValueError(f"unsupported site archive entry: {member.name}")
                seen.add(path)
                size += member.size
                if size > MAX_SITE:
                    raise ValueError("deployed site exceeds the extraction bound")
            tar.extractall(root, filter="data")
    if not (root / "index.html").is_file() or not (root / "ci/archive-results.json").is_file():
        raise ValueError("the deployed artifact is not a complete site with archive evidence")


def git_files(checkout, revision):
    if not SHA.fullmatch(revision):
        raise ValueError("invalid source revision")
    result = {}
    for row in run("git", "ls-tree", "-rz", "--full-tree", revision, cwd=checkout).split(b"\0"):
        if row:
            metadata, path = row.split(b"\t", 1)
            mode, _, blob = metadata.decode().split()
            result[path.decode()] = (mode, blob)
    return result


def source_assets(files):
    assets = {}
    for path, (mode, _) in files.items():
        destination = None
        if path.startswith("site/"):
            destination = path.removeprefix("site/")
            if destination.startswith("ci/"):
                raise ValueError("site/ must not replace generated publication evidence")
        elif path.startswith("wasm/"):
            relative = PurePosixPath(path.removeprefix("wasm/"))
            if len(relative.parts) == 2 and relative.parts[0] in BROWSER and relative.suffix in BROWSER[relative.parts[0]]:
                destination = str(relative)
        if destination is not None:
            if mode not in {"100644", "100755"}:
                raise ValueError(f"site source must be a regular file: {path}")
            if destination in assets:
                raise ValueError(f"source assets have conflicting ownership: {destination}")
            assets[destination] = path
    return assets


def inventory(root):
    files = {}
    for path in sorted(Path(root).rglob("*")):
        if path.is_symlink():
            raise ValueError("the site contains a symbolic link")
        if path.is_file():
            files[path.relative_to(root).as_posix()] = sha256(path)
    return files


def plan_update(checkout, root, baseline, source):
    if run("git", "rev-parse", "HEAD", cwd=checkout).decode().strip() != source:
        raise ValueError("checkout does not match the publication source")
    subprocess.run(["git", "merge-base", "--is-ancestor", baseline["source_sha"], source], cwd=checkout, check=True)
    previous = {}
    if baseline["workflow"] == ".github/workflows/publish-site.yml":
        previous = json.loads((Path(root) / METADATA).read_text())
        if previous.get("schema") != SCHEMA or previous.get("source_sha") != baseline["source_sha"] or not SHA.fullmatch(previous.get("wasm_source_sha", "")):
            raise ValueError("the previous site-only publication has invalid provenance")
    wasm_source = previous.get("wasm_source_sha", baseline["source_sha"])
    subprocess.run(["git", "merge-base", "--is-ancestor", wasm_source, source], cwd=checkout, check=True)
    before = git_files(checkout, baseline["source_sha"])
    current = git_files(checkout, source)
    compiler = git_files(checkout, wasm_source)
    compiler_inputs = lambda files: {path: value for path, value in files.items() if path in WASM_FILES or path.startswith(WASM_ROOTS)}
    wasm = compiler_inputs(compiler) != compiler_inputs(current)
    changed = {path for path in before.keys() | current.keys() if before.get(path) != current.get(path)}
    files = inventory(root)
    old_assets, assets = source_assets(before), source_assets(current)
    changed_assets = {destination for destination, path in assets.items() if files.get(destination) != hashlib.sha256(run("git", "show", f"{source}:{path}", cwd=checkout)).hexdigest()}
    shared_browser = wasm or any(path.startswith("lib/") for path in changed_assets) or any(path.startswith(("wasm/lib/", "tests/browser_smoke/")) or path in {"scripts/ci-npm", "scripts/ci-playwright-install"} for path in changed)
    browser = [name for name in SURFACES if f"{name}/index.html" in assets and (shared_browser or any(path.startswith(f"{name}/") for path in changed_assets) or any(path.startswith(f"wasm/{name}/") for path in changed))]
    pages = {"index.html"}
    for destination, source_path in assets.items():
        if source_path.startswith("site/") and destination.endswith(".html") and destination in changed_assets:
            pages.add(destination)
    return {
        "schema": SCHEMA, "source_sha": source, "baseline": baseline,
        "wasm": wasm, "wasm_source_sha": source if wasm else wasm_source,
        "browser": browser, "pages": sorted(pages), "assets": assets,
        "remove": sorted(old_assets.keys() - assets.keys()),
        "preserved": {path: digest for path, digest in files.items() if path not in old_assets and path != METADATA and (path != "lib/roundhouse_wasm.wasm" or not wasm)},
    }


def copy_assets(checkout, root, revision, assets):
    for destination, source in assets.items():
        path = Path(root) / relative_path(destination)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(run("git", "show", f"{revision}:{source}", cwd=checkout))


def browser_assets(args):
    revision = run("git", "rev-parse", "HEAD", cwd=args.checkout).decode().strip()
    assets = {destination: source for destination, source in source_assets(git_files(args.checkout, revision)).items() if source.startswith("wasm/")}
    copy_assets(args.checkout, args.root, revision, assets)


def restore(args):
    source = publication_context(os.environ)
    api = GitHub()
    baseline = latest_baseline(api)
    with tempfile.TemporaryDirectory() as temp:
        bundle = Path(temp) / "site.zip"
        api.download(baseline["artifact"], bundle)
        extract_site(bundle, args.root)
    state = plan_update(args.checkout, args.root, baseline, source)
    write_json(args.state, state)
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a") as stream:
            stream.write(f"wasm={str(state['wasm']).lower()}\nbrowser={str(bool(state['browser'])).lower()}\n")
    print(json.dumps({key: state[key] for key in ("source_sha", "wasm", "browser", "pages")}, indent=2))


def assemble(args):
    state = json.loads(Path(args.state).read_text())
    if publication_context(os.environ) != state["source_sha"] or run("git", "rev-parse", "HEAD", cwd=args.checkout).decode().strip() != state["source_sha"]:
        raise ValueError("site update source changed")
    root = Path(args.root)
    for path in state["assets"]:
        if path in state["preserved"]:
            raise ValueError(f"static source would overwrite a generated file: {path}")
    if state["wasm"] and (not args.wasm or not Path(args.wasm).is_file()):
        raise ValueError("changed compiler inputs require a fresh WebAssembly build")
    for path in state["remove"]:
        (root / relative_path(path)).unlink(missing_ok=True)
    copy_assets(args.checkout, root, state["source_sha"], state["assets"])
    if state["wasm"]:
        shutil.copyfile(args.wasm, root / "lib/roundhouse_wasm.wasm")
    for path, expected in state["preserved"].items():
        if sha256(root / relative_path(path)) != expected:
            raise ValueError(f"generated site file changed: {path}")
    report = root / "ci/archive-results.json"
    identity = json.loads(report.read_text())
    with tempfile.TemporaryDirectory() as temp:
        witness = Path(temp) / "archive-results.json"
        shutil.copyfile(report, witness)
        subprocess.run([sys.executable, str(SCRIPT_ROOT / "ci-archive-evidence.py"), "verify", "--root", str(root), "--report", str(witness)], check=True, env=os.environ | {
            "GITHUB_SHA": identity["source_sha"], "GITHUB_RUN_ID": identity["run_id"], "GITHUB_RUN_ATTEMPT": str(identity["attempt"]),
        })
    write_json(root / METADATA, {
        "schema": SCHEMA, "source_sha": state["source_sha"], "wasm_source_sha": state["wasm_source_sha"],
        "baseline": {key: state["baseline"][key] for key in ("deployment_id", "source_sha", "run_id", "attempt")},
        "archive_source_sha": identity["source_sha"], "archive_run_id": identity["run_id"],
    })
    if state["browser"]:
        fixture = Path(args.checkout) / "fixtures/real-blog"
        if fixture.exists():
            raise ValueError("browser checks require a clean checkout without fixtures/real-blog")
        for path, text in json.loads((root / "lib/fixture.json").read_text()).items():
            destination = fixture / relative_path(path)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(text)
        shutil.copyfile(root / "lib/roundhouse_wasm.wasm", Path(args.checkout) / "wasm/lib/roundhouse_wasm.wasm")
    print(f"Updated {len(state['assets'])} source assets; preserved {len(state['preserved'])} generated files")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name, function in [("restore", restore), ("assemble", assemble), ("browser-assets", browser_assets)]:
        command = commands.add_parser(name)
        command.add_argument("--root", required=True)
        command.add_argument("--checkout", default=".")
        if name != "browser-assets":
            command.add_argument("--state", required=True)
        if name == "assemble":
            command.add_argument("--wasm")
        command.set_defaults(function=function)
    args = parser.parse_args()
    try:
        args.function(args)
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, zipfile.BadZipFile, tarfile.TarError) as error:
        print(f"ci-site: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
