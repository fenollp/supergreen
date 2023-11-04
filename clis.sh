#!/bin/bash -eu
set -o pipefail

header() {
	cat <<EOF
on: [push]
name: CLIs
jobs:


  meta-check:
    runs-on: ubuntu-latest
    steps:
    - uses: actions/checkout@v4
    - run: ./clis.sh | tee .github/workflows/clis.yml
    - run: git --no-pager diff --exit-code
    - name: Run shellcheck
      uses: ludeeus/action-shellcheck@2.0.0
      with:
        check_together: 'yes'
        severity: error

  bin:
    runs-on: ubuntu-latest
    steps:
    - uses: actions-rs/toolchain@v1
      with:
        profile: minimal
        toolchain: stable

    - uses: actions/checkout@v4

    # Actually, the whole archives
    - name: Cache \`cargo fetch\`
      uses: actions/cache@v3
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: cargo-deps-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: cargo-deps-

    - name: Cache \`cargo install\`
      uses: actions/cache@v3
      with:
        path: ~/instmp
        key: \${{ runner.os }}-cargo-install-\${{ hashFiles('**/Cargo.lock') }}-and-\${{ hashFiles('src/**') }}
        restore-keys: |
          \${{ runner.os }}-cargo-install-\${{ hashFiles('**/Cargo.lock') }}-and-
          \${{ runner.os }}-cargo-install-

    - name: Compile HEAD
      run: |
        CARGO_TARGET_DIR=~/instmp cargo install --force --path=\$PWD

    - uses: actions/upload-artifact@v3
      with:
        name: bin-artifact
        path: ~/instmp/release/rustcbuildx

EOF
}

cli() {
	local name_at_version=$1; shift

	cat <<EOF
  $(sed 's%@%_%g;s%\.%-%g' <<<"$name_at_version"):
    runs-on: ubuntu-latest
    needs: bin
    steps:
    - uses: actions/checkout@v4

    - uses: actions-rs/toolchain@v1
      with:
        profile: minimal
        toolchain: stable
$(
	case "$name_at_version" in
		cargo-llvm-cov@*) printf '    - run: rustup component add llvm-tools-preview\n' ;;
		*) ;;
	esac
)

    - name: Retrieve saved bin
      uses: actions/download-artifact@v3
      with:
        name: bin-artifact
    - run: | # TODO: whence https://github.com/actions/download-artifact/issues/236
        chmod +x ./rustcbuildx
        ./rustcbuildx --version | grep rustcbuildx

    - name: cargo install net=ON cache=OFF remote=OFF
      run: |
        RUSTCBUILDX_DEBUG=1 \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
        RUSTC_WRAPPER="\$PWD"/rustcbuildx \\
          CARGO_TARGET_DIR=~/instst cargo -vv install --force $name_at_version $@ || \\
            cat logs.txt

    - name: cargo install net=ON cache=ON remote=OFF
      run: |
        RUSTCBUILDX_DEBUG=1 \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
        RUSTC_WRAPPER="\$PWD"/rustcbuildx \\
          CARGO_TARGET_DIR=~/instst cargo -vv install --force $name_at_version $@ 2>&1 | tee _ || \\
            cat logs.txt
        cat _ | grep Finished | grep 0...s
        cat _ | grep Fresh
        ! cat _ | grep Compiling
        ! cat _ | grep 'DEBUG|INFO|WARN|ERROR'

EOF
}



header

cli cargo-audit@0.18.3 	    --features=fix
cli cargo-deny@0.14.3
cli diesel_cli@2.1.1        --no-default-features --features=postgres
cli cargo-llvm-cov@0.5.36

# TODO: more
# https://github.com/cross-rs/cross/releases/tag/v0.2.5
# https://crates.io/categories/command-line-utilities?sort=recent-updates
# https://crates.io/crates/cargo-nextest
