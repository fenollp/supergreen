#!/bin/bash -eu
set -o pipefail

git ls-remote https://github.com/moby/buildkit.git \
    | grep /tags/ \
    | grep -v dockerfile \
    | awk '{print $2}' \
    | sort -Vu \
    | grep -vF rc \
    | grep -vF beta \
    | tail -n1 \
    | cut -d/ -f3 \
    | cut -dv -f2

# If that deviates during releases (github vs dockerhub), then rely on:
#   docker run --rm -it docker.io/moby/buildkit:latest --version
#     buildkitd github.com/moby/buildkit v0.22.0 13cf07c97baebd3d5603feecc03f5a46ac98d2a5
