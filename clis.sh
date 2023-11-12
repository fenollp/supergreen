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

  local job=$(sed 's%@%_%g;s%\.%-%g' <<<"$name_at_version")
  case "$name_at_version" in
    buildxargs@*) name_at_version='' ;;
    cross@*) name_at_version='' ;;
    *) ;;
  esac

	cat <<EOF
  $job:
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

    - name: Buildx version
      run: docker buildx version

    - name: Pre-pull images
      run: ./rustcbuildx pull

    - name: Show defaults
      run: ./rustcbuildx env

    - name: Buildx disk usage
      run: docker buildx du | tail -n-1

    - name: cargo install net=ON cache=OFF remote=OFF
      run: |
        RUSTCBUILDX_LOG=debug \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
        RUSTC_WRAPPER="\$PWD"/rustcbuildx \\
          CARGO_TARGET_DIR=~/instst cargo -vv install --jobs=1 --locked --force $name_at_version $@

    - if: \${{ failure() || success() }}
      run: cat logs.txt && echo >logs.txt

    - name: Buildx disk usage
      if: \${{ failure() || success() }}
      run: docker buildx du | tail -n-1

    - name: Target dir disk usage
      if: \${{ failure() || success() }}
      run: du -sh ~/instst

    - name: cargo install net=ON cache=ON remote=OFF
      run: |
        RUSTCBUILDX_LOG=debug \\
        RUSTCBUILDX_LOG_PATH="\$PWD"/logs.txt \\
        RUSTC_WRAPPER="\$PWD"/rustcbuildx \\
          CARGO_TARGET_DIR=~/instst cargo -vv install --jobs=1 --locked --force $name_at_version $@ 2>&1 | tee _

    - if: \${{ failure() || success() }}
      run: cat logs.txt

    - name: Buildx disk usage
      if: \${{ failure() || success() }}
      run: docker buildx du | tail -n-1

    - name: Target dir disk usage
      if: \${{ failure() || success() }}
      run: du -sh ~/instst

    - if: \${{ failure() || success() }}
      run: |
        grep Finished _ | grep -E [01]...s

    - if: \${{ failure() || success() }}
      run: |
        grep Fresh _

    - if: \${{ failure() || success() }}
      run: |
        ! grep Compiling _

    - if: \${{ failure() || success() }}
      run: |
        ! grep 'DEBUG|INFO|WARN|ERROR' _

    - if: \${{ failure() || success() }}
      run: |
        ! grep 'Falling back' _

    - if: \${{ failure() || success() }}
      run: |
        ! grep BUG _

    - if: \${{ failure() || success() }}
      run: cat _ || true

EOF
}



header

# TODO: https://crates.io/categories/command-line-utilities?sort=recent-updates
cli buildxargs@master       --git https://github.com/fenollp/buildxargs.git
cli cargo-audit@0.18.3      --features=fix
cli cargo-deny@0.14.3
cli cargo-llvm-cov@0.5.36
cli cargo-nextest@0.9.61
cli cross@0.2.5             --git https://github.com/cross-rs/cross.git --tag=v0.2.5 cross
cli diesel_cli@2.1.1        --no-default-features --features=postgres
cli hickory-dns@0.24.0      --features=dns-over-rustls

#TODO: not a cli but try users of https://github.com/dtolnay/watt
#TODO: play with cargo flags: lto (embeds bitcode)
#TODO: allowlist non-busting rustc flags => se about this cache key
#TODO: test cargo -vv build -> test -> build and look for "Dirty", expect none
