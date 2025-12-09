#!/usr/bin/env -S bash -eu
set -o pipefail

format=svg

dockerfilegraph() {
    local file=$1; shift
    [[ $# -eq 0 ]]

    fname=$(basename "$file")

    tmpd=$(mktemp -d)
    cp "$file" $tmpd/

    docker build \
        --build-context=recipe=$tmpd \
        --output=recipes/ \
        -<<EOF
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
  --mount=from=recipe,source=/$fname,dst=/app/$fname \
  dockerfilegraph \
    --concentrate \
    --nodesep 0.3 \
    --unflatten 4 \
    --scratch hidden \
    --max-label-length 128 \
    --output $format \
    --filename /app/$fname

FROM scratch
COPY --link --from=run /app/Dockerfile.$format /"${fname//Dockerfile/$format}"
EOF

    rm -rf $tmpd
}

files=(recipes/*.Dockerfile)
if [[ $# -ne 0 ]]; then
    files=($@)
fi

for file in "${!files[@]}"; do
    file=${files[$file]}
    echo $file
    [[ -f "${file//Dockerfile/$format}" ]] && continue
    dockerfilegraph "$file"
done
