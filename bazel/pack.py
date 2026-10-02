"""Pack a directory into a byte-reproducible tar (sorted names, zeroed metadata)."""
import os
import sys
import tarfile

src, dest = sys.argv[1], sys.argv[2]
with tarfile.open(dest, "w", format=tarfile.PAX_FORMAT) as tar:
    for root, dirs, files in os.walk(src):
        dirs.sort()
        for name in sorted(files):
            path = os.path.join(root, name)
            info = tar.gettarinfo(path, arcname=os.path.relpath(path, src))
            info.mtime = 0
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            info.mode = 0o755 if os.access(path, os.X_OK) else 0o644
            with open(path, "rb") as f:
                tar.addfile(info, f)
