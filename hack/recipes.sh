#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

[[ $# -ne 0 ]] && echo "Usage: $0" && exit 1


branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
for runID in $(gh run list --branch "$branch"  --json databaseId,workflowName --jq '.[] | select(.workflowName | test("clis"; "i")) | .databaseId' | head -n $(ls .github/workflows/clis-*.yml | wc -l)); do
	gh run download "$runID" --pattern '*.Dockerfile'

	for f in *.Dockerfile/*.Dockerfile; do
		echo $f
		mv $f recipes/
		rmdir $(dirname $f)
		f=recipes/$(basename $f)

		# When diffing, ignore changes that mention PKG version and image digests.
		if git --no-pager diff --exit-code \
			--ignore-matching-lines=' AS rust-base$' \
			-- $f; then
			git checkout -- $f 2>/dev/null || true
		# else
		# 	"$repo_root"/hack/graph.sh $f
		fi
	done
done
