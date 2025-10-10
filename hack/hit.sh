#!/usr/bin/env -S bash -eu
set -o pipefail

declare -A stages=()
files=(recipes/*.Dockerfile)

for file in "${!files[@]}"; do
    file=${files[$file]}
    echo $file
    while read -r h; do
        ((stages["$h"]++)) || true
    done < <(grep -E ' AS [^ ]+[-][a-f0-9]{16}' $file | grep -vF scratch | grep -vF '##' | awk '{print $4}')
done
total=${#stages[@]}

for stage in "${!stages[@]}"; do
    if [[ "${stages[$stage]}" = 1 ]]; then
        unset stages["$stage"]
    fi
done
hits=${#stages[@]}

echo

for stage in "${!stages[@]}"; do
    echo "${stages[$stage]}: $stage"
done | sort -k1,2

echo

echo "Total recipes: ${#files[@]}"
echo "Total stages: $total"
echo "Stages in common: $hits"
echo "$(bc <<<"scale=2; ($hits*100)/$total")%"
