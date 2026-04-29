#!/usr/bin/env -S bash -eu
set -o pipefail

stable=1.94.0 # Closest to latest stable, as official Rust images availability permits (TODO: use rustup when image isn't yet available)
fixed=1.93.1 # Some fixed rustc version


action__cache='actions/cache@27d5ce7f107fe9357f9df03efb73ab90386fccae # v5.0.5'
action__cache_restore='actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae # v5.0.5'
action__checkout='actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2'
action__docker_login='docker/login-action@4907a6ddec9925e35a0a9e82d7399ccc52663121 # v4.1.0'
action__download_artifact='actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1'
action__install_action='taiki-e/install-action@1329c298aa20c3257846c9b2e0e55967df3e3c37 # v2.75.25'
action__setup_rust_toolchain='actions-rust-lang/setup-rust-toolchain@2b1f5e9b395427c92ee4e3331786ca3c37afe2d7 # v1.16.0'
action__upload_artifact='actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1'

jobdef() {
    local name=$1; shift
    [[ $# -eq 0 ]]
cat <<EOF
  $name:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    defaults:
      run:
        shell: bash -euo pipefail {0}
EOF
}


restore_bin() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - name: Retrieve saved bin
      uses: $action__download_artifact
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
    - run: which cargo && cargo -Vv
    - run: which rustc && rustc -Vv
    - run: rustup show
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
        final_diff=(git --no-pager diff)
        if [[ \${{ matrix.toolchain }} != $stable ]]; then
          final_diff+=(--exit-code)
        fi
        final_diff+=(--ignore-matching-lines='^#')
        final_diff+=(--ignore-matching-lines=' AS rust-base$')
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
    - uses: $action__docker_login
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
    - uses: $action__docker_login
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
    - uses: $action__checkout
      with:
        persist-credentials: false

    - uses: $action__setup_rust_toolchain
      with:
        toolchain: $stable
        cache-on-failure: true
$(rundeps_versions)

    - name: Cache \`cargo fetch\`
      uses: $action__cache
      with:
        path: |
          ~/.cargo/registry/index/
          ~/.cargo/registry/cache/
          ~/.cargo/git/db/
        key: \${{ github.job }}-\${{ runner.os }}-cargo-deps-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: \${{ github.job }}-\${{ runner.os }}-cargo-deps-

    - name: Cache \`cargo install\`
      uses: $action__cache
      with:
        path: ~/instmp
        key: \${{ runner.os }}-cargo-install-\${{ hashFiles('**/Cargo.lock') }}
        restore-keys: |
          \${{ runner.os }}-cargo-install-

    - name: Compile HEAD cargo-green
      run: |
        CARGO_TARGET_DIR=~/instmp cargo install --locked --force --path=./cargo-green

    - uses: $action__upload_artifact
      with:
        name: cargo-green
        path: /home/runner/.cargo/bin/cargo-green
        if-no-files-found: error

# \$(login_to_readonly_hub)
#     - run: cargo green supergreen sync
#     - uses: $action__setup_rust_toolchain
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
#     - uses: $action__upload_artifact
#       with:
#         name: builder-data
#         path: /home/runner/builder-cache
#         if-no-files-found: error
EOF
}


restore_builder_data() {
    [[ $# -eq 0 ]]
    cat <<EOF
  # - if: \${{ false }} # TODO: just-sync'd builder cache ends up >500MB (above artifacts free tier)
  #   name: Retrieve saved builder data
  #   uses: $action__download_artifact
  #   with:
  #     name: builder-data
  #     path: /home/runner/builder-cache
  # - if: \${{ false }} # TODO: just-sync'd builder cache ends up >500MB (above artifacts free tier)
  #   run: |
  #     set -x
  #     sudo mkdir -p \$(cargo green supergreen sync data 2>/dev/null)
  #     sudo mv -v /home/runner/builder-cache/* \$(cargo green supergreen sync data 2>/dev/null)/
  #     sudo chown -R root:root \$(cargo green supergreen sync data 2>/dev/null)

EOF
}
