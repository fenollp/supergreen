#!/usr/bin/env -S bash -eu
set -o pipefail

[[ $# -eq 0 ]] && echo "
Usage:
sort=? keyword=? category=?	$0 find [ <crate> ]
sort=?				$0 rev <crate>

Modifiers:
	sort=recent-updates | alpha | downloads | recent-downloads | new

	keyword=	cli | async | api | parser | web | wasm | database | http | crypto | blockchain

# 57 on 2025/11/24 from 'https://crates.io/api/v1/categories?page=1&per_page=100&sort=crates'
	category=	command-line-utilities | development-tools | no-std | web-programming
			| api-bindings | network-programming | data-structures | cryptography
			| asynchronous | science | embedded | algorithms | encoding | parsing
			| multimedia | rust-patterns | hardware-support | parser-implementations
			| wasm | text-processing | os | mathematics | game-development | database
			| concurrency | command-line-interface | gui | filesystem | external-ffi-bindings
			| graphics | rendering | config | compilers | simulation | authentication
			| memory-management | games | game-engines | visualization | compression
			| database-implementations | caching | text-editors | finance | value-formatting
			| date-and-time | template-engine | internationalization | emulators | accessibility
			| email | localization | computer-vision | aerospace | virtualization | security | automotive
" && exit 1
verb=$1; shift
sort=${sort:-recent-updates}
pause=1
ua=https://github.com/fenollp/supergreen

rev() {
	local crate=$1; shift
	[[ $# -eq 0 ]]
	for i in {1..100}; do
		curl -fsSL "https://crates.io/api/v1/crates/$crate/reverse_dependencies?page=$i&per_page=100&sort=$sort" \
		  --user-agent "$ua" \
		  --compressed -H 'Accept: */*' -H 'Accept-Encoding: gzip, deflate' \
		  | jq -r '.versions[] | select(.yanked == false) | select(.bin_names != []) | .updated_at + "  https://crates.io/crates/" + .crate + "/" + .num + "/dependencies  " + (.bin_names|join(",")) + "  " + .description'
		sleep 1
	done
}

search() {
	[[ $# -le 1 ]]
	local query="sort=$sort"
	if [[ "${1:-}" != '' ]]; then query="$query&q=$1"; fi
	if [[ "${keyword:-}" != '' ]]; then query="$query&keyword=$keyword"; fi
	if [[ "${category:-}" != '' ]]; then query="$query&category=$category"; fi
	echo "$query"
	for i in {1..100}; do
		curl -fsSL "https://crates.io/api/v1/crates?$query&page=$i&per_page=100" \
		  --user-agent "$ua" \
		  --compressed -H 'Accept: */*' -H 'Accept-Encoding: gzip, deflate' \
		| jq -r '.crates[] | select(.yanked == false) | .updated_at + "  https://crates.io/crates/" + .id + "/" + .max_version + "/dependencies  " + "  " + .description'
		sleep 1
	done
}

case "$verb" in
rev) rev $1 ;;
find) search ${1:-} ;;
*) echo "Unexpected argument '$verb'!" && exit 1
esac
