#!/usr/bin/env -S bash -eu
set -o pipefail

declare -A portables=()
files=(recipes/*.Dockerfile)

for file in "${!files[@]}"; do
    file=${files[$file]}
    portable=
    if grep -F 'Pipe this file to:' "$file" >/dev/null 2>&1; then
        portable=➤
        ((portables["$file"]++)) || true
    fi
    printf '%s\t%s\n' "$portable" "$file"
done

echo

echo "Total recipes: ${#files[@]}"
echo "Portable: ${#portables[@]}"
echo "$(bc <<<"scale=2; (${#portables[@]}*100)/${#files[@]}")%"
