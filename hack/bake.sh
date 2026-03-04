#!/usr/bin/env -S bash -eu
set -o pipefail

declare -A portables=()
declare -A sizes=()
files=(recipes/*.Dockerfile)

for file in "${!files[@]}"; do
    file=${files[$file]}
    [[ "$file" = 'recipes/buildxargs@1.4.0.Dockerfile' ]] && continue
    portable=$(tail -n1 "$file" | cut -d/ -f3)
    portables["$portable"]=$(basename "$file")
    sizes["$portable"]=$(grep -cE '^FROM' "$file")
done

sorted() {
    for portable in "${!portables[@]}"; do
        echo "${sizes[$portable]} $portable ${portables[$portable]}"
    done | sort -k1n
}

echo 'group "default" {'
echo '  targets = ['
sorted | while read -r _ bin _; do
    printf '    "%s",\n' "$bin"
done
echo '  ]'
echo '}'
echo
sorted | while read -r _ bin file; do
    printf 'target "%s" {\n' "$bin"
    echo   '  context = "recipes"'
    printf '  dockerfile = "%s"\n' "$file"
    echo   '  output = [{ type = "local", dest = "." }]'
    echo   '}'
done
