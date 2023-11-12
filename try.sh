#!/bin/bash -eux
set -o pipefail

[[ "$#" = 0 ]] && echo "Usage: $0  <bincrate@version> [-f]"
name_at_version=$1; shift


session_name=$(sed 's%@%_%g;s%\.%-%g' <<<"$name_at_version")
tmptrgt=/tmp/$session_name
tmplogs=/tmp/$session_name.logs.txt
tmpgooo=/tmp/$session_name.ready


tmux new-session -d -s "$session_name"
tmux select-window -t "$session_name:0"

send() {
	tmux send-keys "$* && exit" C-m
}


gitdir=$(realpath "$(dirname "$0")")
send "CARGO_TARGET_DIR=/tmp/rstcbldx cargo install --locked --force --path=$gitdir"
tmux split-window
if [[ "${ANEW:-0}" = '1' ]]; then
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

send 'until' '[[' -f "$tmpgooo" ']];' 'do' sleep '1;' 'done' '&&' rm "$tmpgooo" '&&' RUSTCBUILDX_LOG=debug RUSTCBUILDX_LOG_PATH="$tmplogs" RUSTC_WRAPPER=rustcbuildx CARGO_TARGET_DIR="$tmptrgt" cargo -vv install --jobs=1 --locked --force "$name_at_version" "$@" '&&' tmux kill-session -t "$session_name"
tmux select-layout even-vertical

tmux attach-session -t "$session_name"
