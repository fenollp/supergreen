#!/usr/bin/env -S bash -eu
set -o pipefail
# set -x

declare -A sections=()
sections['configuration']=1

envs=()

within=0
title=''
dox=''
while read -r line; do
	if [[ "${line:0:2}" = '##' ]] && [[ "$within" = 0 ]]; then
		within=1
		title=${line#*'# '}
		# shellcheck disable=SC2199,SC2076
		if [[ "${title:0:2}" = '`$' ]]; then
			# The env settings
			title=${title#*$} && title=${title%*\`}
			envs+=("$title")
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
	# shellcheck disable=SC2199,SC2076
	if [[ " ${envs[@]} " =~ " $section " ]]; then continue; fi
	echo "Unused $dox!" && exit 1
done

for env in "${envs[@]}"; do
	dox="./cargo-green/docs/$env.md"
	if ! grep -F "$env" "$dox" >/dev/null 2>&1; then
		echo "Weird docs for $env:"
		cat "$dox"
		exit 1
	fi
done
