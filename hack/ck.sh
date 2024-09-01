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


restore_bin-artifacts() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - name: Retrieve saved bin
      uses: actions/download-artifact@v4
      with:
        name: bin-artifacts

    - name: Install saved bin
      run: | # TODO: whence https://github.com/actions/download-artifact/issues/236
        chmod +x ./cargo-green
        ./cargo-green --version | grep cargo-green
        mv ./cargo-green ~/.cargo/bin/
        cargo green --version
EOF
}


rundeps_versions() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - run: docker info
    - run: docker buildx version
    - run: docker buildx build --help
    - run: podman version || true
    - run: rustc -Vv
EOF
}


cache_usage() {
    [[ $# -eq 0 ]]
    cat <<EOF
    - run: sudo du -sh /var/lib/docker
    - run: docker system df
    - run: docker system df --verbose
    - run: |
        docker buildx du | head || true
        docker buildx du | tail || true
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
        grep Finished  $cargologs | grep -E [01]...s || err=1
        grep Dirty     $cargologs                    && err=1
        grep Compiling $cargologs                    && err=1
        exit \$err
EOF
}


postconds() {
    local cargologs=$1; shift
    local greenlogs=$1; shift
    [[ $# -eq 0 ]]
cat <<EOF
    - if: \${{ failure() || success() }}
      name: 🔴 =means=> it's again that cargo issue https://github.com/rust-lang/cargo/pull/14322
      run: |
        ! grep -C20 -F "thread 'main' panicked at src/cargo/util/dependency_queue.rs:" $cargologs

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> it's again that docker issue https://github.com/moby/buildkit/issues/5217
      run: |
        ! grep -C20 -F 'ResourceExhausted: grpc: received message larger than max' $greenlogs

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> there's some panic!s
      run: |
        ! grep -C20 -F ' panicked at ' $greenlogs

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> there's some BUGs
      run: |
        ! grep -C20 -F 'BUG: ' $greenlogs

    - if: \${{ failure() || success() }}
      name: 🔴 =means=> here's relevant logs
      run: |
        ! grep -C20 -F ' >>> ' $greenlogs

    - if: \${{ failure() || success() }}
      run: tail -n9999999 $greenlogs ; echo >$greenlogs
EOF
}
