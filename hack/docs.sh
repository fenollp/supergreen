#!/usr/bin/env -S bash -eu
set -o pipefail

within=0
while read -r line; do
	[[ "${line:0:6}" = '### `$' ]] && [[ "$within" = 0 ]] && within=1
	[[ "${line:0:6}" = '### `$' ]] && [[ "$within" = 1 ]] && within=0 && title=${line:5}
	[[ "$line" = '---' ]] && within=0

	if [[ "$within" = 1 ]]; then
		echo ".$title."
	fi
done <README.md
