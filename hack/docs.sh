#!/usr/bin/env -S bash -eu
set -o pipefail
# set -x

declare -A sections=()
sections['configuration']=1
sections['usage']=1

envs='CARGOGREEN' # Explicitly undocumented

within=0
title=''
dox=''
while IFS= read -r line; do
	if [[ "${line:0:2}" = '##' ]] && [[ "$within" = 0 ]]; then
		within=1
		title=${line#*'# '}
		# shellcheck disable=SC2199,SC2076
		if [[ "${title:0:2}" = '`$' ]]; then
			# The env settings
			title=${title#*$} && title=${title%*\`}
			envs="$envs $title"
		elif [[ " ${!sections[@]} " =~ " ${title,,} " ]]; then
			# The sections
			title=${title,,}
			(( sections["$title"]++ ))
		else
			within=0 && title='' && continue
		fi
		dox=./cargo-green/docs/$title.md
		echo "╭─ $dox"
		printf -- '' >"$dox"
		continue
	fi
	[[ "${line:0:2}" != '##' ]] && [[ "$within" != 0 ]] && within=0 && continue
	[[ "$line" = '---' ]] && within=0 && title='' && continue
	[[ "$title" == '' ]] && continue

	echo "  $line"
	printf '%s\n' "$line" >>"$dox"

done <README.md

for section in "${!sections[@]}"; do
	if [[ "${sections["$section"]}" = 2 ]]; then continue; fi
	echo "Wrong section '$section' (${sections["$section"]})!" && exit 1
done

for dox in ./cargo-green/docs/*.md; do
	section=$(basename "$dox" .md)
	# shellcheck disable=SC2199,SC2076
	if [[ " ${!sections[@]} " =~ " $section " ]]; then continue; fi
	if [[ " $envs " =~ " $section " ]]; then continue; fi
	echo "Unused $dox!" && exit 1
done

for env in $envs; do
	if [[ "$env" = 'CARGOGREEN' ]]; then continue; fi
	dox="./cargo-green/docs/$env.md"
	if ! grep -F "$env" "$dox" >/dev/null 2>&1; then
		echo "Weird docs for $env:"
		cat "$dox"
		exit 1
	fi
done

ordered_envs() {
	[[ $# -eq 0 ]]
	grep -E '^ +var!' cargo-green/src/supergreen.rs | cut -d! -f2 | sed 's%(%%;s%^ENV_%CARGOGREEN_%'
}

diff -y \
	<(echo "${envs:11}" | sed 's% %\n%g') \
	<(ordered_envs)

diff -y \
	<(grep -E '^  - \[`\$' README.md | cut -d '`' -f2 | cut -d '$' -f2) \
	<(ordered_envs)
