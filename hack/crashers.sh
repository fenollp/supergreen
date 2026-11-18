#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")

# The nightly props job (.github/workflows/props.yml) runs each property for a while. A property that
# fails uploads a `crasher-*` artifact: the minimized inputs proptest saved, and the parameters used.
# This script gathers those of the last days, dedupes them, and fails on any that is new.
#
# A crasher is known once its seed is in cargo-green/proptest-regressions/ (where proptest saves it,
# and replays it from): fix the bug, commit the seed, and it stops being new.
#
# Usage: $0 [--days N] [--repo owner/name] [--from <dir>]   #=> Fail if there are new crashers (needs curl, jq, unzip)
#        --days N      Artifacts of the last N days (default: 30)
#        --repo R      Defaults to $GITHUB_REPOSITORY, else the origin remote
#        --from DIR    Read DIR/**/crashers.tsv instead of downloading (and DIR/**/params.env)
#        $GITHUB_TOKEN (or $GH_TOKEN) with `actions: read`: optional. Without, artifacts come through
#                      https://nightly.link (the repo must have its app installed; listing is rate-limited to 60/h)
#
# Usage: $0 params                #=> PROPTEST_* env vars the props run with: how long to shrink, how many cases
# Usage: $0 filter                #=> The nextest filterset of the props
# Usage: $0 collect               #=> TSV of the crashers proptest saved here: file, test (from $TEST), seed, minimized input

shrink_time_ms=600000
shrink_iters=100000
cases=100000000 # Bounded by time, not cases

props_filter='test(/_for_all::/)' # Property tests live in modules named *_for_all

regressions=cargo-green/proptest-regressions

tab=$'\t'

collect() {
    local f seed input
    while IFS= read -r f; do
        # cc <seed> # shrinks to <minimized input>
        while IFS= read -r line; do
            seed=$(awk '{print $2}' <<<"$line")
            input=${line#*# shrinks to }
            printf '%s\t%s\t%s\t%s\n' "${f#"$regressions"/}" "${TEST:-?}" "$seed" "$input"
        done < <(grep -E '^cc [0-9a-f]+ ' "$f" || true)
    done < <(find "$regressions" -name '*.txt' 2>/dev/null | sort)
}

case "${1:-}" in
    params)
        echo "export PROPTEST_CASES=$cases"
        echo "export PROPTEST_MAX_SHRINK_TIME=$shrink_time_ms"
        echo "export PROPTEST_MAX_SHRINK_ITERS=$shrink_iters"
        exit 0 ;;
    filter)
        echo "$props_filter"
        exit 0 ;;
    collect)
        cd "$repo_root"
        collect
        exit 0 ;;
esac

days=30
repo=${GITHUB_REPOSITORY:-}
from=
while [[ $# -gt 0 ]]; do
    case "$1" in
        --days) days=${2:?}; shift 2 ;;
        --repo) repo=${2:?}; shift 2 ;;
        --from) from=${2:?}; shift 2 ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
done
[[ "$days" =~ ^[0-9]+$ ]] || { echo "--days wants a number, got: $days" >&2; exit 1; }

cd "$repo_root"
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT

if [[ -n "$from" ]]; then
    cp -R "$from"/. "$dir"/
else
    token=${GITHUB_TOKEN:-${GH_TOKEN:-}}
    if [[ -z "$repo" ]]; then
        repo=$(git remote get-url origin | sed -E 's#^(git@github.com:|https://github.com/)##; s#\.git$##')
    fi
    # Listing artifacts is open on public repos. Downloading one is not: that takes a token, or nightly.link.
    api() {
        local auth=()
        [[ -z "$token" ]] || auth=(-H "Authorization: Bearer $token")
        curl -fsS --retry 3 "${auth[@]}" -H 'Accept: application/vnd.github+json' "$@" || { echo "crashers.sh: GET failed: ${*: -1}" >&2; return 1; }
    }

    since=$(date -u -d "-$days days" +%s)
    page=1
    while :; do
        batch=$(api "https://api.github.com/repos/$repo/actions/artifacts?per_page=100&page=$page")
        [[ "$(jq '.artifacts | length' <<<"$batch")" -gt 0 ]] || break
        # id, download url, run id, commit, created: of unexpired crasher artifacts, newest first
        jq -r --argjson since "$since" '
            .artifacts[]
            | select((.name | startswith("crasher-")) and (.expired | not) and ((.created_at | fromdateiso8601) >= $since))
            | [.id, .archive_download_url, .workflow_run.id, .workflow_run.head_sha, .created_at] | @tsv
        ' <<<"$batch" >>"$dir/artifacts.tsv"
        # Newest first: done once a whole page is older than the window
        jq -e --argjson since "$since" '[.artifacts[] | select((.created_at | fromdateiso8601) >= $since)] | length > 0' <<<"$batch" >/dev/null || break
        ((page++))
    done
    touch "$dir/artifacts.tsv"

    while IFS=$'\t' read -r id url run sha created; do
        mkdir -p "$dir/$id"
        if [[ -n "$token" ]]; then
            api -L -o "$dir/$id.zip" "$url"
        else
            curl -fsSL --retry 3 -o "$dir/$id.zip" "https://nightly.link/$repo/actions/artifacts/$id.zip" ||
                { echo "crashers.sh: GET failed: nightly.link artifact $id" >&2; exit 1; }
        fi
        unzip -q -o "$dir/$id.zip" -d "$dir/$id"
        rm "$dir/$id.zip"
        printf '%s\t%s\t%s\n' "https://github.com/$repo/actions/runs/$run" "$sha" "$created" >"$dir/$id/run.tsv"
    done <"$dir/artifacts.tsv"
fi

# Every crasher found, then deduped: same property, same error (here: same minimized input, whatever the seed or the run).
# The first seen is the newest. Known seeds (already in the tree) are not new.
known=$(grep -rhoE '^cc [0-9a-f]+' "$regressions" 2>/dev/null | awk '{print $2}' | sort -u || true)

found=0
new=0
declare -A seen=()
while IFS= read -r tsv; do
    d=$(dirname "$tsv")
    run=$(cut -f1 "$d/run.tsv" 2>/dev/null || echo "?")
    sha=$(cut -f2 "$d/run.tsv" 2>/dev/null || echo "?")
    created=$(cut -f3 "$d/run.tsv" 2>/dev/null || echo "?")
    params=$(grep -h '^export ' "$d/params.env" 2>/dev/null | sed 's/^export //' | tr '\n' ' ' || true)
    while IFS=$'\t' read -r file test seed input; do
        [[ -n "$seed" ]] || continue
        ((found++)) || true
        key="$test$tab$input"
        [[ -z "${seen[$key]:-}" ]] || continue
        seen[$key]=1
        grep -qxF "$seed" <<<"$known" && continue
        ((new++)) || true
        {
            echo "NEW CRASHER: $test"
            echo "  minimized input: $input"
            echo "  seed:            cc $seed (add to cargo-green/proptest-regressions/$file once fixed)"
            echo "  found:           $created on $sha: $run"
            echo "  parameters:      ${params:-?}"
        } >&2
        [[ -z "${GITHUB_ACTIONS:-}" ]] || echo "::error title=New crasher::$test shrinks to: ${input:0:500}"
    done <"$tsv"
done < <(find "$dir" -name crashers.tsv | sort -rV)

echo "$found crashers over the last $days days, $new new (once deduped by property and minimized input)" >&2
[[ "$new" -eq 0 ]]
