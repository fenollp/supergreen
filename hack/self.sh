#!/usr/bin/env -S bash -eu
set -o pipefail

source $(realpath "$(dirname "$0")")/ck.sh

nightly=nightly-2025-08-06

# Usage:  $0                              #=> generate CI


postbin_steps() {
    local toolchain=${1:-stable}; shift
    [[ $# -eq 0 ]]
    cat <<EOF
    - uses: actions-rust-lang/setup-rust-toolchain@v1
      with:
        toolchain: $toolchain
        cache: false
        rustflags: ''

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
$(rundeps_versions)
    - uses: actions-rust-lang/setup-rust-toolchain@v1
      with:
        toolchain: stable

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

    - uses: actions/upload-artifact@v4
      with:
        name: cargo-green
        path: /home/runner/.cargo/bin/cargo-green


$(bin_jobdef 'installs')
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo install net=ON cache=OFF remote=OFF jobs=1
      run: |
$(unset_action_envs)
        cargo green -vv install --jobs=1 --locked --force --path=./cargo-green |& tee ../_
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'audits')
    steps:
$(rundeps_versions)
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
    - name: cargo +$nightly green udeps --all-targets --jobs=1 cache=OFF remote=OFF
      run: |
$(unset_action_envs)
        cargo +$nightly green udeps --all-targets --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Again, with +toolchain to cargo-green
      run: |
$(unset_action_envs)
        cargo green +$nightly udeps --all-targets --jobs=1 |& tee ../_
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'builds')
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
    - name: cargo build net=OFF cache=OFF remote=OFF jobs=1
      run: |
$(unset_action_envs)
        cargo green -vv build --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        cargo green -vv build --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command thrice without modifications (jobs>1)...
      run: |
$(unset_action_envs)
        cargo green -vv build --jobs=\$(nproc) --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'tests')
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
    - name: cargo test net=OFF cache=OFF remote=OFF jobs=1
      run: |
$(unset_action_envs)
        cargo green -vv test --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        cargo green -vv test --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command thrice without modifications (jobs>1)...
      run: |
$(unset_action_envs)
        cargo green -vv test --jobs=\$(nproc) --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'checks')
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
    - name: cargo check net=OFF cache=OFF remote=OFF jobs=\$(nproc)
      run: |
$(unset_action_envs)
        cargo green -vv check --jobs=\$(nproc) --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        cargo green -vv check --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'packages')
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch |& tee ../_
$(postconds ../_)
    - name: cargo package net=OFF cache=OFF remote=OFF jobs=1
      run: |
$(unset_action_envs)
        cargo green -vv package --jobs=1 --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_)
$(cache_usage)


$(bin_jobdef 'clippy')
    steps:
$(rundeps_versions)
$(postbin_steps)
    - run: rustup component add clippy
$(cache_usage)
    - name: cargo fetch
      run: |
$(unset_action_envs)
        cargo green -vv fetch
$(postconds ../_)
    - name: cargo clippy net=OFF cache=OFF remote=OFF jobs=1
      run: |
$(unset_action_envs)
        cargo green -vv clippy --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
$(unset_action_envs)
        cargo green -vv clippy --jobs=\$(nproc) --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_)
$(cache_usage)
EOF
