#!/bin/bash -eu
set -o pipefail

source $(realpath "$(dirname "$0")")/ck.sh

nightly=nightly-2025-02-09

# Usage:  $0                              #=> generate CI


postbin_steps() {
    local toolchain=${1:-stable}; shift
    [[ $# -eq 0 ]]
cat <<EOF
    - uses: actions-rs/toolchain@v1
      with:
        profile: minimal
        toolchain: $toolchain

$(restore_bin-artifacts)

    - uses: actions/checkout@v4

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
EOF
}


cat <<EOF
on: [push]
name: self
jobs:


$(jobdef 'bin')
    steps:
$(rundeps_versions)
    - uses: actions-rs/toolchain@v1
      with:
        profile: minimal
        toolchain: stable

    - uses: actions/checkout@v4

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
        name: bin-artifacts
        path: /home/runner/.cargo/bin/cargo-green


$(jobdef 'installs')
    needs: bin
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo install net=ON cache=OFF remote=OFF jobs=1
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          CARGO_TARGET_DIR=~/instst cargo green -vv install --jobs=1 --locked --force --path=./cargo-green |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)


$(jobdef 'audits')
    needs: bin
    steps:
$(rundeps_versions)
$(postbin_steps)
    - uses: taiki-e/install-action@v2
      with:
        tool: cargo-audit
$(cache_usage)
    - name: cargo audit net=ON cache=OFF remote=OFF
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv audit |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)


$(jobdef 'udeps')
    needs: bin
    steps:
$(rundeps_versions)
$(postbin_steps $nightly)
    - uses: taiki-e/install-action@v2
      with:
        tool: cargo-udeps
$(cache_usage)
    - name: cargo +$nightly green udeps --all-targets --jobs=1 cache=OFF remote=OFF
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo +$nightly green udeps --all-targets --jobs=1 |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)
    - name: Again, with +toolchain to cargo-green
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green +$nightly udeps --all-targets --jobs=1 |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)


$(jobdef 'builds')
    needs: bin
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo build net=OFF cache=OFF remote=OFF jobs=1
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv build --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv build --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_ ../logs.txt)
$(cache_usage)
    - name: Ensure running the same command thrice without modifications (jobs>1)...
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv build --jobs=\$(nproc) --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_ ../logs.txt)
$(cache_usage)


$(jobdef 'tests')
    needs: bin
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo test net=OFF cache=OFF remote=OFF jobs=1
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv test --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv test --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_ ../logs.txt)
$(cache_usage)
    - name: Ensure running the same command thrice without modifications (jobs>1)...
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv test --jobs=\$(nproc) --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_ ../logs.txt)
$(cache_usage)


$(jobdef 'checks')
    needs: bin
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo check net=OFF cache=OFF remote=OFF jobs=\$(nproc)
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv check --jobs=\$(nproc) --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv check --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_ ../logs.txt)
$(cache_usage)


$(jobdef 'packages')
    needs: bin
    steps:
$(rundeps_versions)
$(postbin_steps)
$(cache_usage)
    - name: cargo package net=OFF cache=OFF remote=OFF jobs=1
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          CARGO_TARGET_DIR=~/cargo-package cargo green -vv package --jobs=1 --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)


$(jobdef 'clippy')
    needs: bin
    steps:
$(rundeps_versions)
$(postbin_steps)
    - run: rustup component add clippy
$(cache_usage)
    - name: cargo clippy net=OFF cache=OFF remote=OFF jobs=1
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv clippy --jobs=1 --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postconds ../_ ../logs.txt)
$(cache_usage)
    - name: Ensure running the same command twice without modifications...
      run: |
        CARGOGREEN_LOG=trace \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/../logs.txt \\
          cargo green -vv clippy --jobs=\$(nproc) --all-targets --all-features --locked --frozen --offline |& tee ../_
$(postcond_fresh ../_)
$(postconds ../_ ../logs.txt)
$(cache_usage)
EOF
