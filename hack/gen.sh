#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

[[ $# -ne 0 ]] && echo "Usage: $0" && exit 1

"$repo_root"/hack/bake.sh | tee docker-bake.hcl
"$repo_root"/hack/clis.sh | tee .github/workflows/clis.yml
"$repo_root"/hack/docs.sh
"$repo_root"/hack/self.sh | tee .github/workflows/self.yml

git --no-pager diff --exit-code
