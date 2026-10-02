# The docker lane's VM image: Docker's own dind image, plus the bash and
# coreutils Bazel's test runner needs (its harness is a bash script).
FROM docker:27-dind
RUN apk add --no-cache bash coreutils curl
LABEL org.opencontainers.image.source=https://github.com/rubys/roundhouse
