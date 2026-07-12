#!/usr/bin/env -S bash -eu
set -o pipefail

checked=2ceefa0090080354b80cc2f5415039bdb0d2bf0b
if [[ ! -f old ]]; then
  curl -#fsSLo old https://raw.githubusercontent.com/rust-lang/cargo/$checked/src/doc/src/reference/environment-variables.md
fi
if [[ ! -f new ]]; then
  curl -#fsSLo new https://raw.githubusercontent.com/rust-lang/cargo/refs/heads/master/src/doc/src/reference/environment-variables.md
fi
echo Comparing $checked vs https://github.com/rust-lang/cargo/blob/master/src/doc/src/reference/environment-variables.md
echo
diff -u old new
rm old new
