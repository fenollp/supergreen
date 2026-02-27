#!/usr/bin/env -S bash -eu
set -o pipefail

stable=1.91.1 # Closest to latest stable, as official Rust images availability permits (TODO: use rustup when image isn't yet available)
fixed=1.90.0 # Some fixed rustc version


jobdef() {
    local name=$1; shift
    [[ $# -eq 0 ]]
cat <<EOF
  $name:
    runs-on: ubuntu-latest
    defaults:
      run:
        shell: bash -euo pipefail {0}
EOF
}


restore_bin() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - name: Retrieve saved bin
      uses: actions/download-artifact@v8
      with:
        name: cargo-green

    - name: Install saved bin
      shell: bash -eu {0}
      run: | # TODO: whence https://github.com/actions/download-artifact/issues/236
        chmod +x ./cargo-green
        { ! ./cargo-green --version ; } | grep cargo-green
        mv ./cargo-green ~/.cargo/bin/
EOF
}


rundeps_versions() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - run: docker info
    - run: docker buildx version
    - run: docker buildx build --help
    - run: podman version || true
    - run: cargo -Vv
EOF
}


cache_usage() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - run: sudo du -sh /var/lib/docker || true
    - run: docker system df
    - run: docker system df --verbose
    - run: BUILDX_BUILDER=supergreen docker buildx du | head || true
    - run: BUILDX_BUILDER=supergreen docker buildx du | tail || true
    - run: BUILDX_BUILDER=supergreen docker buildx du --verbose
EOF
}


postcond_fresh() {
    local cargologs=$1; shift
    [[ $# -eq 0 ]]
cat <<EOF
    - name: ...doesn't recompile anything
      run: |
        err=0
        grep Finished  $cargologs
        grep Finished  $cargologs | grep -E [012]...s || ((err+=1))
        grep Dirty     $cargologs                     && ((err+=2))
        grep Compiling $cargologs                     && ((err+=4))
        exit \$err
EOF
}


postconds() {
    local cargologs=$1; shift
    [[ $# -eq 0 ]]
cat <<EOF
    - if: \${{ always() }}
      name: 🔴 =means=> it's again that cargo issue https://github.com/rust-lang/cargo/pull/14322
      run: |
        ! grep -C20 -F 'src/cargo/util/dependency_queue.rs:' $cargologs

    - if: \${{ always() }}
      name: 🔴 =means=> it's again that docker issue https://github.com/moby/buildkit/issues/5217
      run: |
        ! grep -C20 -F 'ResourceExhausted: grpc: received message larger than max' \$CARGOGREEN_LOG_PATH

    - if: \${{ always() }}
      name: 🔴 =means=> it's this HTTP/2 code = Unavailable desc = error reading from server-- connection error-- COMPRESSION_ERROR
      run: |
        ! grep -C20 -F 'connection error: COMPRESSION_ERROR' \$CARGOGREEN_LOG_PATH

    - if: \${{ always() }}
      name: 🔴 =means=> there's some panic!s
      run: |
        ! grep -C20 -F ' panicked at ' \$CARGOGREEN_LOG_PATH

    - if: \${{ always() }}
      name: 🔴 =means=> there's some BUGs
      run: |
        ! grep -C20 -F 'BUG: ' \$CARGOGREEN_LOG_PATH

    - if: \${{ always() }}
      name: 🔴 =means=> here's cargo's error text
      run: |
        ! grep -C20 -E '-[a-f0-9]{16} [eE]rror:' \$CARGOGREEN_LOG_PATH $cargologs

    - if: \${{ always() }}
      name: 🔴 =means=> 429 Too Many Requests
      run: |
        ! grep -C20 -F '429 Too Many Requests' \$CARGOGREEN_LOG_PATH $cargologs

    - if: \${{ always() }}
      name: 🔴 =means=> here's relevant logs
      run: |
        ! grep -C20 -F ' >>> ' \$CARGOGREEN_LOG_PATH

    - if: \${{ always() && env.CARGOGREEN_FINAL_PATH != '' }}
      name: 🌀 Maybe show final path diff
      run: |
        case "\$GITHUB_JOB" in
          cross*|ntpd*) exit 0 ;; # TODO: fix undeterministic final paths for git crates
          *) ;;
        esac
        final_diff=(git --no-pager diff)
        if [[ \${{ matrix.toolchain }} != $stable ]]; then
          final_diff+=(--exit-code)
        fi
        final_diff+=(--ignore-matching-lines='^#')
        final_diff+=(--ignore-matching-lines=' AS rust-base$')
        final_diff+=(--ignore-matching-lines=' NUM_JOBS=')
        final_diff+=(-- \$CARGOGREEN_FINAL_PATH)
        "\${final_diff[@]}"

    - if: \${{ failure() }}
      name: 🌀 cargo-green logs
      run: tail -n9999999 \$CARGOGREEN_LOG_PATH ; echo >\$CARGOGREEN_LOG_PATH
EOF
}


unset_action_envs() {
    [[ $# -eq 0 ]]
cat <<EOF
        unset CARGO_PROFILE_DEV_DEBUG
        unset CARGO_REGISTRIES_CRATES_IO_PROTOCOL
        unset CARGO_TERM_COLOR
        unset CARGO_UNSTABLE_SPARSE_REGISTRY
EOF
}


login_to_readonly_hub() {
    [[ $# -eq 0 ]]
cat <<EOF
    - uses: docker/login-action@v3
      if: \${{ ! startsWith(github.ref, 'refs/heads/dependabot/') }}
      with:
        username: \${{ vars.DOCKERHUB_USERNAME }}
        password: \${{ secrets.DOCKERHUB_TOKEN }}
EOF
}


# https://github.com/fenollp/supergreen/pkgs/container/supergreen
login_to_readwrite_ghcr() {
    [[ $# -eq 0 ]]
cat <<EOF
    - uses: docker/login-action@v3
      with:
        registry: ghcr.io
        username: \${{ github.actor }}
        password: \${{ secrets.GITHUB_TOKEN }}
EOF
}


bin_job() {
    [[ $# -eq 0 ]]
cat <<EOF
$(jobdef 'bin')
    steps:
    - uses: actions/checkout@v6

    - uses: actions-rust-lang/setup-rust-toolchain@v1
      with:
        toolchain: $stable
        cache-on-failure: true
$(rundeps_versions)

    - name: Cache \`cargo fetch\`
      uses: actions/cache@v5
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: \${{ github.job }}-\${{ runner.os }}-cargo-deps-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: \${{ github.job }}-\${{ runner.os }}-cargo-deps-

    - name: Cache \`cargo install\`
      uses: actions/cache@v5
      with:
        path: ~/instmp
        key: \${{ runner.os }}-cargo-install-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: |
          \${{ runner.os }}-cargo-install-

    - name: Compile HEAD cargo-green
      run: |
        CARGO_TARGET_DIR=~/instmp cargo install --locked --force --path=./cargo-green

    - uses: actions/upload-artifact@v7
      with:
        name: cargo-green
        path: /home/runner/.cargo/bin/cargo-green
        if-no-files-found: error

# \$(login_to_readonly_hub)
#     - run: cargo green supergreen sync
#     - uses: actions-rust-lang/setup-rust-toolchain@v1
#       with:
#         toolchain: $fixed
#         cache-on-failure: true
#     - run: cargo green supergreen sync
# \$(rundeps_versions)
#     - run: cargo green supergreen builder rm || true
#     - run: sudo du -sh \$(cargo green supergreen sync data 2>/dev/null) || true
#     - run: sudo cp -r \$(cargo green supergreen sync data 2>/dev/null) /home/runner/builder-cache || true
#     - run: sudo chown -R \$(id -u):\$(id -g) /home/runner/builder-cache
#     - run: du -sh /home/runner/builder-cache || true
#     - run: ls -lha /home/runner/builder-cache/ || true
#     - uses: actions/upload-artifact@v7
#       with:
#         name: builder-data
#         path: /home/runner/builder-cache
#         if-no-files-found: error
EOF
}


restore_builder_data() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - if: \${{ false }} # TODO: just-sync'd builder cache ends up >500MB (above artifacts free tier)
      name: Retrieve saved builder data
      uses: actions/download-artifact@v8
      with:
        name: builder-data
        path: /home/runner/builder-cache
    - if: \${{ false }} # TODO: just-sync'd builder cache ends up >500MB (above artifacts free tier)
      run: |
        set -x
        sudo mkdir -p \$(cargo green supergreen sync data 2>/dev/null)
        sudo mv -v /home/runner/builder-cache/* \$(cargo green supergreen sync data 2>/dev/null)/
        sudo chown -R root:root \$(cargo green supergreen sync data 2>/dev/null)

EOF
}
