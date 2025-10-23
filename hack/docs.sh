#!/usr/bin/env -S bash -eu
set -o pipefail
# set -x

within=0
while read -r line; do
	[[ "${line:0:6}" = '### `$' ]] && [[ "$within" = 0 ]] && within=1 && title=${line:5}
	# [[ "${line:0:6}" = '### `$' ]] && [[ "$within" = 1 ]] && within=0
	[[ "$line" = '---' ]] && within=0

	if [[ "$within" = 1 ]]; then
		echo ".$title.$line"
	fi
done <README.md
