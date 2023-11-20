#!/bin/bash -eu
set -o pipefail

# TODO: https://crates.io/categories/command-line-utilities?sort=recent-updates
declare -a nvs nvs_args
   i=0  ; nvs[i]=buildxargs@master;     nvs_args[i]='--git https://github.com/fenollp/buildxargs.git'
((i+=1)); nvs[i]=cargo-audit@0.18.3;    nvs_args[i]='--features=fix'
((i+=1)); nvs[i]=cargo-deny@0.14.3;     nvs_args[i]=''
((i+=1)); nvs[i]=cargo-llvm-cov@0.5.36; nvs_args[i]=''
((i+=1)); nvs[i]=vixargs@0.1.0;         nvs_args[i]=''
((i+=1)); nvs[i]=cargo-nextest@0.9.61;  nvs_args[i]=''
((i+=1)); nvs[i]=cross@0.2.5;           nvs_args[i]='--git https://github.com/cross-rs/cross.git --tag=v0.2.5 cross'
((i+=1)); nvs[i]=diesel_cli@2.1.1;      nvs_args[i]='--no-default-features --features=postgres'
((i+=1)); nvs[i]=hickory-dns@0.24.0;    nvs_args[i]='--features=dns-over-rustls'
((i+=1)); nvs[i]=rustcbuildx@main;      nvs_args[i]='--git https://github.com/fenollp/rustcbuildx.git --branch=main rustcbuildx'

#TODO: not a cli but try users of https://github.com/dtolnay/watt
#TODO: play with cargo flags: lto (embeds bitcode)
#TODO: allowlist non-busting rustc flags => se about this cache key
#TODO: test cargo -vv build -> test -> build and look for "Dirty", expect none
#TODO: merge buildx.yml in here
#TODO: test cargo miri usage
#TODO: test cargo lambda build --release --arm64 usage
#TODO: test https://github.com/facebookexperimental/MIRAI


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

as_install() {
  local name_at_version=$1; shift
  case "$name_at_version" in
    buildxargs@*) echo ;;
    cross@*) echo ;;
    rustcbuildx@*) echo ;;
    *) echo "$name_at_version" ;;
  esac
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
          CARGO_TARGET_DIR=~/instst cargo -vv install --jobs=1 --locked --force $(as_install "$name_at_version") $@

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
          CARGO_TARGET_DIR=~/instst cargo -vv install --jobs=1 --locked --force $(as_install "$name_at_version") $@ 2>&1 | tee _

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

if [[ $# = 0 ]]; then
  header

  for i in "${!nvs[@]}"; do
    name_at_version=${nvs["$i"]}
    case "$name_at_version" in
      rustcbuildx@*) continue ;;
    esac
    cli "$name_at_version" "${nvs_args["$i"]}"
  done

  exit
fi


name_at_version=$1; shift
cleanup=${1:-0}

picked=-1
for i in "${!nvs[@]}"; do
  case "${nvs[$i]}" in
    *"$name_at_version"*) picked=$i; break ;;
  esac
done
if [[ "$picked" = -1 ]]; then
  echo "Could not match '$name_at_version' among:"
  for i in "${!nvs[@]}"; do
    echo "${nvs[$i]}" "${nvs_args[$i]}"
  done
  exit 1
fi
name_at_version=${nvs[$i]}
args=${nvs_args[$i]}

session_name=$(sed 's%@%_%g;s%\.%-%g' <<<"$name_at_version")
tmptrgt=/tmp/clis-$session_name
tmplogs=/tmp/clis-$session_name.logs.txt
tmpgooo=/tmp/clis-$session_name.ready


tmux new-session -d -s "$session_name"
tmux select-window -t "$session_name:0"

send() {
  tmux send-keys "$* && exit" C-m
}


gitdir=$(realpath "$(dirname "$0")")
send "CARGO_TARGET_DIR=/tmp/rstcbldx cargo install --locked --force --path=$gitdir"
tmux split-window
if [[ "$cleanup" = '1' ]]; then
  send rm -rf "$tmptrgt"
  tmux select-layout even-vertical
  tmux split-window
  send docker buildx prune -af '&&' touch "$tmpgooo"
  tmux select-layout even-vertical
  tmux split-window
else
  touch "$tmpgooo"
fi

send rustcbuildx pull
tmux select-layout even-vertical
tmux split-window


send "rm $tmplogs; touch $tmplogs; tail -f $tmplogs; :"
tmux select-layout even-vertical
tmux split-window

send \
  'until' '[[' -f "$tmpgooo" ']];' 'do' sleep '1;' 'done' '&&' rm "$tmpgooo" '&&' \
  RUSTCBUILDX_LOG=debug \
  RUSTCBUILDX_LOG_PATH="$tmplogs" \
  RUSTC_WRAPPER=rustcbuildx \
    CARGO_TARGET_DIR="$tmptrgt" cargo -vv install --jobs=1 --locked --force "$(as_install "$name_at_version")" "$args" \
  '&&' tmux kill-session -t "$session_name"
tmux select-layout even-vertical

tmux attach-session -t "$session_name"
