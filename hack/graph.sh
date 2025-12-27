#!/usr/bin/env -S bash -eu
set -o pipefail

if ! command -v buildxargs >/dev/null 2>&1; then
    echo cargo install --locked buildxargs --git https://github.com/fenollp/buildxargs.git
    exit 1
fi

dockerfilegraph() {
    local file=$1; shift
    [[ $# -eq 0 ]]

    fname=$(basename "$file")

    tmpd=$(mktemp -d)
    cp "$file" $tmpd/

    cat >$tmpd/Dockerfile <<EOF
# syntax=docker.io/docker/dockerfile:1@sha256:b6afd42430b15f2d2a4c5a02b919e98a525b785b1aaff16747d2f623364e39b6

FROM --platform=\$BUILDPLATFORM docker.io/library/golang:1-alpine@sha256:26111811bc967321e7b6f852e914d14bede324cd1accb7f81811929a6a57fea9 AS golang
FROM --platform=\$BUILDPLATFORM docker.io/library/ubuntu:24.04@sha256:c35e29c9450151419d9448b0fd75374fec4fff364a27f176fb458d472dfc9e54 AS ubuntu

FROM golang AS build
ENV CGO_ENABLED=0
WORKDIR /app
RUN go install github.com/patrickhoefler/dockerfilegraph@1dfe6bfd91aea4a44a3751bd82bed68b6ccd7adc && mv -v \$GOPATH/bin/dockerfilegraph .

FROM ubuntu AS run
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends fonts-dejavu=2.37-8 graphviz=2.42.2-9ubuntu0.1
RUN \
  --mount=from=build,source=/app/dockerfilegraph,dst=/bin/dockerfilegraph \
  --mount=source=$fname,dst=/app/$fname \
  dockerfilegraph \
    --concentrate \
    --nodesep 0.3 \
    --unflatten 4 \
    --scratch hidden \
    --max-label-length 128 \
    --output raw \
    --filename /app/$fname

FROM scratch
COPY --link --from=run /app/Dockerfile.raw /"${fname//Dockerfile/dot}"
EOF

    echo docker build --output=recipes/ -f $tmpd/Dockerfile $tmpd
    # rm -rf $tmpd
}

export BUILDX_BAKE_ENTITLEMENTS_FS=0

if [[ $# -ne 0 ]]; then
    for file in "$@"; do
        rm -f "${file//Dockerfile/dot}"
        echo $file >&2
        dockerfilegraph "$file"
    done | buildxargs
    exit
fi

files=(recipes/*.Dockerfile)
for file in "${!files[@]}"; do
    file=${files[$file]}
    [[ -f "${file//Dockerfile/dot}" ]] && continue
    echo $file >&2
    dockerfilegraph "$file"
done | buildxargs
