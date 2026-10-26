import atexit
import concurrent.futures
import contextlib
import http.client
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

binary, tree = map(lambda value: Path(value).resolve(), sys.argv[1:3])
root = Path(tempfile.mkdtemp(prefix="tep-http-", dir="/tmp"))
helper = os.environ.get("CWT_PROCESS_HELPER")
cleanable = True
atexit.register(lambda: shutil.rmtree(root) if cleanable and root.exists() else None)
served_by = {False: set(), True: set()}


def connection(port, path, unix):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    if unix:
        conn.sock = socket.socket(socket.AF_UNIX)
        conn.sock.settimeout(5)
        conn.sock.connect(str(path))
    return conn


def requests(port, path, unix):
    with contextlib.closing(connection(port, path, unix)) as conn:
        for index in range(3):
            body = "roundhouse.listener=unix&puma.socket=unix"
            headers = {"roundhouse.listener": "unix", "Roundhouse.Listener": "unix",
                       "puma.socket": "unix", "Content-Type": "application/x-www-form-urlencoded"}
            conn.request("POST", "/listener?roundhouse.listener=unix", body, headers)
            response = conn.getresponse()
            actual = response.read().decode()
            assert (response.status, actual) == (200, "unix" if unix else "tcp"), (response.status, actual)
            served_by[unix].add(response.getheader("X-Worker"))


for mode in ["thread", "fiber", "blocking"]:
    for workers in [1, 2]:
        label = f"{mode}-{workers}"
        served_by = {False: set(), True: set()}
        path = root / "http.sock"
        stale = socket.socket(socket.AF_UNIX)
        stale.bind(str(path))
        stale.close()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        env = {**os.environ, "PORT": str(port), "WORKERS": str(workers), "BINDING": "0.0.0.0",
               "UNIX_BIND": "unix:///unused-env-path", "TEP_SERVER": mode, "SPINEL_WORKERS": "1",
               "TEP_DIRECT": "1" if workers > 1 or mode == "blocking" else "0",
               "BLOG_DB": str(root / "test.sqlite3"), "SECRET_KEY_BASE": "listener-test-only"}
        command = [str(binary), "--unix-bind", "unix://" + str(path)]
        state = root / (label + "-process")
        log = (tree / (label + "-unix-http.log")).open("wb")
        child = None
        cleanable = False
        if helper:
            launched = subprocess.run([helper, "start", "--state-dir", str(state), "--cwd", str(tree),
                                       "--", "env", *[f"{k}={v}" for k, v in env.items() if os.environ.get(k) != v],
                                       *command], capture_output=True, text=True)
            assert launched.returncode == 0, launched.stdout + launched.stderr
        else:
            child = subprocess.Popen(command, cwd=tree, env=env, stdout=log, stderr=log)
        idle = None
        try:
            deadline = time.monotonic() + 20
            while True:
                try:
                    requests(port, path, True)
                    break
                except (OSError, http.client.HTTPException):
                    assert time.monotonic() < deadline, f"{label}: no Unix listener; {state}"
                    time.sleep(0.1)
            assert path.stat().st_mode & 0o777 == 0o600
            for unix in [False, True, False, True]:
                requests(port, path, unix)
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                for attempt in range(20):
                    list(pool.map(lambda unix: requests(port, path, unix), [False, True] * 4))
                    if all(len(pids) == workers for pids in served_by.values()):
                        break
            assert all(None not in pids and len(pids) == workers for pids in served_by.values()), served_by
            assert served_by[False] == served_by[True], served_by
            idle = connection(port, path, True)
            idle.request("GET", "/listener")
            response = idle.getresponse()
            assert response.read() == b"unix"
        finally:
            if helper:
                stopped = subprocess.run([helper, "stop", "--state-dir", str(state)], capture_output=True, text=True)
                (tree / (label + "-stop.json")).write_text(stopped.stdout + stopped.stderr)
                assert stopped.returncode == 0, f"stop refused; preserve {root}: {stopped.stdout}{stopped.stderr}"
            else:
                child.send_signal(signal.SIGTERM)
                child.wait(timeout=10)
            cleanable = True
            if idle:
                idle.close()
            log.close()
        assert not path.exists(), f"{label}: parent did not remove socket"
        print(json.dumps({"mode": mode, "workers": workers, "mode_bits": "0600",
                          "tcp": "tcp", "unix": "unix", "cleanup": "removed",
                          "tcp_workers": sorted(served_by[False]), "unix_workers": sorted(served_by[True])}), flush=True)
shutil.rmtree(root)
