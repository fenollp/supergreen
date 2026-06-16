#!/usr/bin/env -S bash -eu
set -o pipefail

declare -A portables=()
declare -A sizes=()
files=(recipes/*.Dockerfile)

for file in "${!files[@]}"; do
    file=${files[$file]}
    [[ "$file" = 'recipes/buildxargs@1.4.0.Dockerfile' ]] && continue
    portable=$(tail -n1 "$file" | cut -d/ -f3)
    [[ "$portable" = '' ]] && continue # That recipe does not (yet) produce a binary
    portables["$portable"]=$(basename "$file")
    sizes["$portable"]=$(grep -cE '^FROM' "$file")
done

sorted() {
    for portable in "${!portables[@]}"; do
        echo "${sizes[$portable]} $portable ${portables[$portable]}"
    done | sort -k1n
}

echo '{'
echo '  "group": { "default": {'
first=1
while read -r _ bin _; do
    if [[ $first = 1 ]]; then
        first=0
        printf '    "targets": [ "%s"\n' "$bin"
    else
        printf '               , "%s"\n' "$bin"
    fi
done < <(sorted)
echo '               ]}},'
echo
echo '  "target":'
x='{ '
while read -r _ bin file; do
    printf '  %s"%s": {\n' "$x" "$bin"
    echo   '      "context": "recipes",'
    printf '      "dockerfile": "%s",\n' "$file"
    echo   '      "output": ["."]'
    echo   '    }'
    x=', '
done < <(sorted)
echo '  }'
echo '}'
