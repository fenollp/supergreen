#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

[[ $# -ne 0 ]] && echo "Usage: $0" && exit 1


"$repo_root"/hack/bake.sh | tee docker-bake.hcl

rm -f .github/workflows/clis-*.yml
"$repo_root"/hack/clis.sh

"$repo_root"/hack/self.sh | tee .github/workflows/self.yml

"$repo_root"/hack/docs.sh


git --no-pager diff --exit-code -- .github docker-bake.hcl cargo-green/docs
