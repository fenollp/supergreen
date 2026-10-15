#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")
source "$repo_root"/hack/ck.sh

nightly=nightly-2026-04-09

# Usage:  $0                              #=> generate CI



postbin_steps() {
    local toolchain=${1:-stable}; shift
    [[ $# -eq 0 ]]
    cat <<EOF
$(login_to_readonly_hub)
    - uses: $action__setup_rust_toolchain
      with:
        toolchain: $toolchain
        rustflags: ''
        cache-on-failure: true
$(restore_bin)
$(restore_builder_data)
    - uses: $action__checkout
      with:
        persist-credentials: false

    - name: Cache \`cargo fetch\`
      uses: $action__cache
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: \${{ github.job }}-\${{ runner.os }}-cargo-deps-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: \${{ github.job }}-\${{ runner.os }}-cargo-deps-

$(cargo_green_setup)
    - run: cargo fetch
    - run: cargo green supergreen env
EOF
}

cargo_green_fetch() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green fetch |& tee ../_
$(postconds ../_)
EOF
}

try_then_fallback_single_threaded() {
    cat <<EOF
    - name: 🔵 $*
      id: do-try
      timeout-minutes: 11
      run: |
$(unset_action_envs)
        $* |& tee ../_
    - name: $* --jobs=1
      id: do-try-jobs1
      timeout-minutes: 11
      if: \${{ job.steps.do-try.outcome == 'failure' }}
      run: |
$(unset_action_envs)
        $* --jobs=1 |& tee ../_
$(postconds ../_)
EOF
}

run_again_ensuring_freshly_result() {
    cat <<EOF
    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        $* |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
EOF
}

bin_jobdef() {
    local name=$1; shift
    [[ $# -eq 0 ]]
    cat <<EOF
$(jobdef "$name")
    needs: bin
    env:
      RUST_BACKTRACE: 1
      CARGOGREEN_LOG: trace
      CARGOGREEN_LOG_PATH: logs.txt
EOF
}


cat <<EOF
on: [push]
name: Self
permissions: {}
jobs:


$(bin_job)


$(bin_jobdef 'naked')
    steps:
$(postbin_steps)
$(cache_usage)
    - name: 🔵 cargo vs cargo green (NAKED)
      run: |
$(unset_action_envs)
        diff <(cargo) <(cargo green |& tee ../_)
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'installs')
    steps:
$(postbin_steps)
$(cache_usage)
$(try_then_fallback_single_threaded cargo green install --locked --force --path=./cargo-green)
$(cache_usage)


$(bin_jobdef 'audits')
    steps:
$(postbin_steps)
    - uses: $action__install_action
      with:
        tool: cargo-audit
$(cache_usage)
    - name: 🔵 cargo audit net=ON cache=OFF remote=OFF
      run: |
$(unset_action_envs)
        cargo green audit |& tee ../_
    - run: grep Scanning ../_
$(postconds ../_)
$(cache_usage)

# When audits fail on main: try updating vulnerable dependencies and open (or update) a PR.
# A separate job so that write permissions never reach a job building (thus running) dependencies' code.
# NOTE: requires the repository setting "Allow GitHub Actions to create and approve pull requests".
$(jobdef 'audits-fix')
    needs: audits
    if: \${{ always() && needs.audits.result == 'failure' && github.ref == 'refs/heads/main' }}
    permissions:
      contents: write
      pull-requests: write
    steps:
    - uses: $action__checkout
      with:
        persist-credentials: false
    - uses: $action__setup_rust_toolchain
      with:
        toolchain: stable
        rustflags: ''
        cache: false
    - uses: $action__install_action
      with:
        tool: cargo-audit
    - name: Update vulnerable dependencies
      id: fix
      run: |
        cargo audit --json >../audit.json || true
        jq -r '.vulnerabilities.list[] | .package | "\(.name)@\(.version)"' ../audit.json | sort -u >../vulnerable.txt
        if ! [[ -s ../vulnerable.txt ]]; then
          echo "No vulnerable dependency: audits failed for another reason"
          exit
        fi
        while read -r pkg; do
          cargo update --package "\$pkg" || true
        done <../vulnerable.txt
        cargo audit || true
        if git diff --quiet Cargo.lock; then
          echo "No patched versions available (yet) for:" && cat ../vulnerable.txt
          exit
        fi
        {
          echo 'Found by \`cargo audit\` then updated with \`cargo update --package\`:'
          echo
          jq -r '.vulnerabilities.list[] | "- [\(.advisory.id)](\(.advisory.url // "https://rustsec.org/advisories/\(.advisory.id)")) \(.package.name)@\(.package.version): \(.advisory.title)"' ../audit.json
        } >../body.md
        echo changed=true >>"\$GITHUB_OUTPUT"
    - name: Open or update PR
      if: steps.fix.outputs.changed == 'true'
      env:
        GH_TOKEN: \${{ github.token }}
      run: |
        branch=audit-fix
        git config user.name 'github-actions[bot]'
        git config user.email 'action@github.com'
        git switch -c "\$branch"
        git commit -m 'chore(deps): update vulnerable dependencies' -- Cargo.lock
        git push --force "https://x-access-token:\$GH_TOKEN@github.com/\$GITHUB_REPOSITORY" "HEAD:refs/heads/\$branch"
        if [[ "\$(gh pr view "\$branch" --json state --jq .state 2>/dev/null)" == OPEN ]]; then
          gh pr edit "\$branch" --body-file ../body.md
        else
          gh pr create --base main --head "\$branch" --title 'chore(deps): update vulnerable dependencies' --body-file ../body.md
        fi


$(bin_jobdef 'udeps')
    steps:
$(postbin_steps $nightly)
    - uses: $action__install_action
      with:
        tool: cargo-udeps
$(cache_usage)
$(try_then_fallback_single_threaded cargo +$nightly green udeps --all-targets)
$(cache_usage)

$(bin_jobdef 'udeps-bis')
    steps:
$(postbin_steps $nightly)
    - uses: $action__install_action
      with:
        tool: cargo-udeps
$(cache_usage)
$(try_then_fallback_single_threaded cargo green +$nightly udeps --all-targets)
$(cache_usage)


$(bin_jobdef 'builds')
    steps:
$(postbin_steps)
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green build --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
$(run_again_ensuring_freshly_result cargo green build --all-targets --all-features --locked --frozen --offline)
$(cache_usage)


$(bin_jobdef 'tests')
    steps:
$(postbin_steps)
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green test --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
$(run_again_ensuring_freshly_result cargo green test --all-targets --all-features --locked --frozen --offline)
$(cache_usage)


$(bin_jobdef 'checks')
    steps:
$(postbin_steps)
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green check --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
$(run_again_ensuring_freshly_result cargo green check --all-targets --all-features --locked --frozen --offline)
$(cache_usage)


$(bin_jobdef 'docs')
    steps:
$(postbin_steps)
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded "RUSTDOCFLAGS='-D warnings'" cargo doc --locked --frozen --offline --all-features --no-deps)
$(cache_usage)
$(run_again_ensuring_freshly_result "RUSTDOCFLAGS='-D warnings'" cargo doc --locked --frozen --offline --all-features --no-deps)
$(cache_usage)


$(bin_jobdef 'packages')
    steps:
$(postbin_steps)
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green package --all-features --locked --frozen --offline)
$(cache_usage)
# FIXME: 'Dirty [..]: the file `src/[..]` has changed ([..], 19s after last build at [..])'
# \$(run_again_ensuring_freshly_result cargo green package --all-features --locked --frozen --offline)
# \$(cache_usage)


$(bin_jobdef 'clippy')
    steps:
$(postbin_steps)
    - run: rustup component add clippy
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green clippy --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
$(run_again_ensuring_freshly_result cargo green clippy --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
EOF
