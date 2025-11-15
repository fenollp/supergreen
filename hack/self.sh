#!/usr/bin/env -S bash -eu
set -o pipefail

repo_root=$(realpath "$(dirname "$(dirname "$0")")")
source "$repo_root"/hack/ck.sh

nightly=nightly-2025-08-06

# Usage:  $0                              #=> generate CI


# TODO: all of `cargo --list` (including shortcuts) except for cinstall-ed plugins
# Currently missing (non-exhaustive)
# cargo green add
# cargo green bench
# cargo green clean
# cargo green doc
# cargo green init
# cargo green install
# cargo green new
# cargo green publish
# cargo green remove
# cargo green run
# cargo green search
# cargo green uninstall
# cargo green update


postbin_steps() {
    local toolchain=${1:-stable}; shift
    [[ $# -eq 0 ]]
    cat <<EOF
$(login_to_readonly_hub)
    - uses: actions-rust-lang/setup-rust-toolchain@v1
      with:
        toolchain: $toolchain
        rustflags: ''
        cache-on-failure: true
$(rundeps_versions)

$(restore_bin)

    - uses: actions/checkout@v5

    - name: Cache \`cargo fetch\`
      uses: actions/cache@v4
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: \${{ github.job }}-\${{ runner.os }}-cargo-deps-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: \${{ github.job }}-\${{ runner.os }}-cargo-deps-

    - run: cargo fetch
    - run: cargo green supergreen env
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
name: self
jobs:


$(jobdef 'bin')
    steps:
    - uses: actions-rust-lang/setup-rust-toolchain@v1
      with:
        toolchain: stable
        cache-on-failure: true
$(rundeps_versions)

    - uses: actions/checkout@v5

    - name: Cache \`cargo fetch\`
      uses: actions/cache@v4
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: \${{ github.job }}-\${{ runner.os }}-cargo-deps-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: \${{ github.job }}-\${{ runner.os }}-cargo-deps-

    - name: Cache \`cargo install\`
      uses: actions/cache@v4
      with:
        path: ~/instmp
        key: \${{ runner.os }}-cargo-install-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: |
          \${{ runner.os }}-cargo-install-

    - name: Compile HEAD cargo-green
      run: |
        CARGO_TARGET_DIR=~/instmp cargo install --locked --force --path=./cargo-green

    - uses: actions/upload-artifact@v5
      with:
        name: cargo-green
        path: /home/runner/.cargo/bin/cargo-green


$(bin_jobdef 'installs')
    steps:
$(postbin_steps)
$(cache_usage)
    - name: 🔵 cargo green install --locked --force --path=./cargo-green
      run: |
$(unset_action_envs)
        cargo green -vv install --locked --force --path=./cargo-green |& tee ../_
    - name: cargo green install --locked --force --path=./cargo-green --jobs=1
      if: \${{ failure() }}
      run: |
$(unset_action_envs)
        cargo green -vv install --locked --force --path=./cargo-green --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'audits')
    steps:
$(postbin_steps)
    - uses: taiki-e/install-action@v2
      with:
        tool: cargo-audit
$(cache_usage)
    - name: cargo audit net=ON cache=OFF remote=OFF
      run: |
$(unset_action_envs)
        cargo green -vv audit |& tee ../_
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'udeps')
    steps:
$(rundeps_versions)
$(postbin_steps $nightly)
    - uses: taiki-e/install-action@v2
      with:
        tool: cargo-udeps
$(cache_usage)
    - name: 🔵 cargo +$nightly green udeps --all-targets
      run: |
$(unset_action_envs)
        cargo +$nightly green udeps --all-targets |& tee ../_
    - name: cargo +$nightly green udeps --all-targets --jobs=1
      if: \${{ failure() }}
      run: |
$(unset_action_envs)
        cargo +$nightly green udeps --all-targets --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: 🔵 cargo green +$nightly udeps --all-targets
      run: |
$(unset_action_envs)
        cargo green +$nightly udeps --all-targets |& tee ../_
    - name: cargo green +$nightly udeps --all-targets --jobs=1
      if: \${{ failure() }}
      run: |
$(unset_action_envs)
        cargo green +$nightly udeps --all-targets --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'builds')
    steps:
$(postbin_steps)
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
    - name: 🔵 cargo green build --all-targets --all-features --locked --frozen --offline
      run: |
$(unset_action_envs)
        cargo green -vv build --all-targets --all-features --locked --frozen --offline |& tee ../_
    - name: cargo green build --all-targets --all-features --locked --frozen --offline --jobs=1
      if: \${{ failure() }}
      run: |
$(unset_action_envs)
        cargo green -vv build --all-targets --all-features --locked --frozen --offline --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command thrice without modifications...
      run: |
$(unset_action_envs)
        cargo green -vv build --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'tests')
    steps:
$(postbin_steps)
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
    - name: 🔵 cargo green test --all-targets --all-features --locked --frozen --offline
      run: |
$(unset_action_envs)
        cargo green -vv test --all-targets --all-features --locked --frozen --offline |& tee ../_
    - name: cargo green test --all-targets --all-features --locked --frozen --offline --jobs=1
      if: \${{ failure() }}
      run: |
$(unset_action_envs)
        cargo green -vv test --all-targets --all-features --locked --frozen --offline --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        cargo green -vv test --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'checks')
    steps:
$(postbin_steps)
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
    - name: 🔵 cargo green check --all-targets --all-features --locked --frozen --offline
      run: |
$(unset_action_envs)
        cargo green -vv check --all-targets --all-features --locked --frozen --offline |& tee ../_
    - name: cargo green check --all-targets --all-features --locked --frozen --offline --jobs=1
      if: \${{ failure() }}
      run: |
$(unset_action_envs)
        cargo green -vv check --all-targets --all-features --locked --frozen --offline --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        cargo green -vv check --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'packages')
    steps:
$(postbin_steps)
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
    - name: 🔵 cargo green package --all-features --locked --frozen --offline
      run: |
$(unset_action_envs)
        cargo green -vv package --all-features --locked --frozen --offline |& tee ../_
    - name: cargo green package --all-features --locked --frozen --offline --jobs=1
      if: \${{ failure() }}
      run: |
$(unset_action_envs)
        cargo green -vv package --all-features --locked --frozen --offline --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'clippy')
    steps:
$(postbin_steps)
    - run: rustup component add clippy
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch
$(postconds ../_)
    - name: 🔵 cargo green clippy --all-targets --all-features --locked --frozen --offline
      run: |
$(unset_action_envs)
        cargo green -vv clippy --all-targets --all-features --locked --frozen --offline |& tee ../_
    - name: cargo green clippy --all-targets --all-features --locked --frozen --offline --jobs=1
      if: \${{ failure() }}
      run: |
$(unset_action_envs)
        cargo green -vv clippy --all-targets --all-features --locked --frozen --offline --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        cargo green -vv clippy --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)
EOF
