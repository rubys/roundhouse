#!/bin/sh
# ci.yml smoke-campfire-docker: the archive's Dockerfile built and served.
# docker build is quiet: its full log overflows Bazel's 1MB test-output cap and hides the result.
set -eu
tar xzf "$1"
cd campfire-docker
docker build -q -t campfire .
docker run -d --name campfire -p 3000:3000 campfire
for i in $(seq 1 30); do
  curl -sf -o /dev/null localhost:3000/first_run && break
  sleep 1
done
root=$(curl -s -o /dev/null -w '%{http_code}' localhost:3000/)
first=$(curl -s -o /dev/null -w '%{http_code}' localhost:3000/first_run)
logo=$(curl -s -o /dev/null -w '%{http_code}' localhost:3000/account/logo)
echo "GET / -> $root; GET /first_run -> $first; GET /account/logo -> $logo"
docker logs campfire 2>&1 | tail -50
docker rm -f campfire >/dev/null
[ "$root" = 302 ] && [ "$first" = 200 ] && [ "$logo" = 200 ]
