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

    - run: |
        cargo green supergreen setup || true
        { cargo green supergreen setup 2>/dev/null || true; } | sudo /bin/sh -xe
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
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
EOF
}

try_then_fallback_single_threaded() {
    cat <<EOF
    - name: 🔵 $*
      run: |
$(unset_action_envs)
        $* |& tee ../_
    - name: $* --jobs=1
      if: \${{ failure() }}
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
$(try_then_fallback_single_threaded cargo green -vv install --locked --force --path=./cargo-green)
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
        cargo green -vv audit |& tee ../_
    - run: grep Scanning ../_
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'udeps')
    steps:
$(postbin_steps $nightly)
    - uses: $action__install_action
      with:
        tool: cargo-udeps
$(cache_usage)
$(try_then_fallback_single_threaded cargo +$nightly green udeps --all-targets)
$(cache_usage)
$(try_then_fallback_single_threaded cargo green +$nightly udeps --all-targets)
$(cache_usage)


$(bin_jobdef 'builds')
    steps:
$(postbin_steps)
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green -vv build --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
$(run_again_ensuring_freshly_result cargo green -vv build --all-targets --all-features --locked --frozen --offline)
$(cache_usage)


$(bin_jobdef 'tests')
    steps:
$(postbin_steps)
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green -vv test --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
$(run_again_ensuring_freshly_result cargo green -vv test --all-targets --all-features --locked --frozen --offline)
$(cache_usage)


$(bin_jobdef 'checks')
    steps:
$(postbin_steps)
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green -vv check --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
$(run_again_ensuring_freshly_result cargo green -vv check --all-targets --all-features --locked --frozen --offline)
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
$(try_then_fallback_single_threaded cargo green -vv package --all-features --locked --frozen --offline)
$(cache_usage)
# FIXME: 'Dirty [..]: the file `src/[..]` has changed ([..], 19s after last build at [..])'
# \$(run_again_ensuring_freshly_result cargo green -vv package --all-features --locked --frozen --offline)
# \$(cache_usage)


$(bin_jobdef 'clippy')
    steps:
$(postbin_steps)
    - run: rustup component add clippy
$(cache_usage)
$(cargo_green_fetch)
$(try_then_fallback_single_threaded cargo green -vv clippy --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
$(run_again_ensuring_freshly_result cargo green -vv clippy --all-targets --all-features --locked --frozen --offline)
$(cache_usage)
EOF
