#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

[[ $# -ne 0 ]] && echo "Usage: $0" && exit 1


branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
runID=$(gh run list --branch "$branch" --limit 1 --workflow CLIs --json databaseId --jq '.[].databaseId')
gh run download "$runID" --pattern '*.Dockerfile'

for f in *.Dockerfile/*.Dockerfile; do
	echo $f
	mv $f recipes/
	rmdir $(dirname $f)
	f=recipes/$(basename $f)

	# When diffing, ignore changes that:
	#  mention the crate version number
	#  mention the rustc version number
	#  mention BuildKit syntax version
	#  and changes to cargo JSON stderr messages. (TODO: drop) Turns out these are flaky though multiple builds...
	if git --no-pager diff --exit-code \
		--ignore-matching-lines='^#' \
		--ignore-matching-lines=' AS rust-base$' \
		--ignore-matching-lines=' NUM_JOBS=' \
		-- $f; then
		git checkout -- $f 2>/dev/null || true
	# else
	# 	"$repo_root"/hack/graph.sh $f
	fi
done
