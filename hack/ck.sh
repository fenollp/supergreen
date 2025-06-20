#!/bin/bash -eu
set -o pipefail


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
      uses: actions/download-artifact@v4
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
    - run: docker buildx prune --all --force
EOF
}


cache_usage() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - run: sudo du -sh /var/lib/docker
    - run: docker system df
    - run: docker system df --verbose
    - run: docker buildx du | head || true
    - run: docker buildx du | tail || true
    - run: docker buildx du --verbose
EOF
}


postcond_fresh() {
    local cargologs=$1; shift
    [[ $# -eq 0 ]]
cat <<EOF
    - name: ...doesn't recompile anything
      run: |
        err=0
        grep Finished  $cargologs | grep -E [012]...s || err=1
        grep Dirty     $cargologs                     && err=1
        grep Compiling $cargologs                     && err=1
        exit \$err
EOF
}


postconds() {
    local cargologs=$1; shift
    [[ $# -eq 0 ]]
cat <<EOF
    - if: \${{ failure() || success() }}
      name: 🔴 =means=> it's again that cargo issue https://github.com/rust-lang/cargo/pull/14322
      run: |
        ! grep -C20 -F 'src/cargo/util/dependency_queue.rs:' $cargologs

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> it's again that docker issue https://github.com/moby/buildkit/issues/5217
      run: |
        ! grep -C20 -F 'ResourceExhausted: grpc: received message larger than max' \$CARGOGREEN_LOG_PATH

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> there's some panic!s
      run: |
        ! grep -C20 -F ' panicked at ' \$CARGOGREEN_LOG_PATH

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> there's some BUGs
      run: |
        ! grep -C20 -F 'BUG: ' \$CARGOGREEN_LOG_PATH

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> here's relevant logs
      run: |
        ! grep -C20 -F ' >>> ' \$CARGOGREEN_LOG_PATH

    - if: \${{ failure() || success() }}
      name: cargo-green logs
      run: tail -n9999999 \$CARGOGREEN_LOG_PATH ; echo >\$CARGOGREEN_LOG_PATH

    - if: \${{ ( failure() || success() ) && env.CARGOGREEN_FINAL_PATH != '' && matrix.toolchain != 'stable' }}
      name: Maybe show final path diff
      run: |
        case "\$GITHUB_JOB" in
          cross*|ntpd*) exit 0 ;; # TODO: fix undeterministic final paths for git crates
          *) ;;
        esac
        git --no-pager diff --exit-code -I '^# Generated by' -I ' AS rust-base$' -I '^# syntax=docker.io/docker/dockerfile:1@' -- \$CARGOGREEN_FINAL_PATH
EOF
}

unset_action_envs() {
    [[ $# -eq 0 ]]
cat <<EOF
        unset CARGO_INCREMENTAL
        unset CARGO_PROFILE_DEV_DEBUG
        unset CARGO_REGISTRIES_CRATES_IO_PROTOCOL
        unset CARGO_TERM_COLOR
        unset CARGO_UNSTABLE_SPARSE_REGISTRY
EOF
}
