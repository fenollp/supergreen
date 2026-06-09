#!/bin/sh
# rustup-init.sh @ tag 1.29.0 — only the parts that run when invoked as:
#   rustup-init.sh --verbose -y --no-modify-path --profile minimal \
#                  --default-toolchain 1.95 --default-host x86_64-unknown-linux-gnu
# on x86_64 GNU/Linux with curl(OpenSSL).
#
# Lines tagged   # NOT TAKEN   describe branches whose condition is false here.
# Functions defined-but-never-called are omitted entirely.

# ─── Top-level prologue ──────────────────────────────────────────────────────

has_local() {
    local _has_local
}
has_local 2>/dev/null || alias local=typeset   # has_local succeeds → alias NOT installed

# is_zsh() defined; called once from downloader (returns false on dash/bash).

set -u

RUSTUP_UPDATE_ROOT="${RUSTUP_UPDATE_ROOT:-https://static.rust-lang.org/rustup}"
RUSTUP_QUIET=no

# usage(): defined, never called (no --help / -h in args).

# ─── main() — called from the bottom of the file ─────────────────────────────

main() {
    downloader --check                            # first call into downloader (see below)
    need_cmd uname
    need_cmd mktemp
    need_cmd chmod
    need_cmd mkdir
    need_cmd rm
    need_cmd rmdir

    get_architecture || return 1                  # || branch NOT TAKEN
    local _arch="$RETVAL"                         # → x86_64-unknown-linux-gnu
    assert_nz "$_arch" "arch"

    local _default_host_override=""
    # case _arch in *windows*) … ;;               # NOT TAKEN

    local _ext=""
    # case _arch in *windows*) _ext=".exe" ;;     # NOT TAKEN

    local _url
    # if [ "${RUSTUP_VERSION+set}" = 'set' ]      # NOT TAKEN (unset)
    _url="${RUSTUP_UPDATE_ROOT}/dist"
    _url="${_url}/${_arch}/rustup-init${_ext}"
    # → https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init

    local _dir
    _dir="$(ensure mktemp -d)"                    # the `if !` failure branch NOT TAKEN

    local _file="${_dir}/rustup-init${_ext}"

    local _ansi_escapes_are_valid=false
    if [ -t 2 ]; then
        if [ "${TERM+set}" = 'set' ]; then
            case "$TERM" in
                xterm*|rxvt*|urxvt*|linux*|vt*)
                    _ansi_escapes_are_valid=true  # taken iff terminal matches
                    ;;
            esac
        fi
    fi

    # ── Arg sweep #1: only -y / -q / --help / --quiet matter to the script ──
    local need_tty=yes
    for arg in "$@"; do
        case "$arg" in
            # --help)  NOT TAKEN
            # --quiet) NOT TAKEN
            *)
                OPTIND=1
                # ${arg%%--*} is empty for any arg starting with `--`, so these
                # five args take the `continue`:
                #   --verbose, --no-modify-path, --profile,
                #   --default-toolchain, --default-host
                if [ "${arg%%--*}" = "" ]; then
                    continue
                fi
                # Remaining args reach getopts:  -y, minimal, 1.95,
                # x86_64-unknown-linux-gnu.  Only `-y` is a recognised flag.
                while getopts :hqy sub_arg "$arg"; do
                    case "$sub_arg" in
                        # h) NOT TAKEN
                        # q) NOT TAKEN
                        y)
                            need_tty=no           # taken when arg == -y
                            ;;
                    esac
                done
                ;;
        esac
    done

    say 'downloading installer'
    ensure mkdir -p "$_dir"
    ensure downloader "$_url" "$_file" "$_arch"   # second call into downloader
    ensure chmod u+x "$_file"
    # if [ ! -x "$_file" ]                        # NOT TAKEN (chmod just succeeded)

    # ── Arg sweep #2: was --default-host given? ──
    for arg in "$@"; do
        case "$arg" in
            --default-host|--default-host=*)
                _default_host_override=           # taken (we passed --default-host)
                break
                ;;
        esac
    done

    # if [ "$need_tty" = "yes" ] && [ ! -t 0 ]    # NOT TAKEN (need_tty=no)
    # else branch:
    ignore "$_file" ${_default_host_override:+"$_default_host_override"} "$@"
    # _default_host_override is empty, so ${var:+word} expands to nothing.
    # Effective exec:
    #   "$_file" --verbose -y --no-modify-path --profile minimal \
    #            --default-toolchain 1.95 --default-host x86_64-unknown-linux-gnu

    local _retval=$?
    ignore rm "$_file"
    ignore rmdir "$_dir"
    return "$_retval"
}

# ─── get_current_exe()  — called once from get_architecture ──────────────────

get_current_exe() {
    local _current_exe
    if test -L /proc/self/exe ; then
        _current_exe=/proc/self/exe                # taken
    # else branch NOT TAKEN
    fi
    echo "$_current_exe"
}

# ─── get_bitness() — called once from get_architecture ───────────────────────

get_bitness() {
    need_cmd head
    local _current_exe=$1
    local _current_exe_head
    _current_exe_head=$(head -c 5 "$_current_exe")
    # if 32-bit ELF                                # NOT TAKEN
    if [ "$_current_exe_head" = "$(printf '\177ELF\002')" ]; then
        echo 64                                    # taken
    # else err+exit                                # NOT TAKEN
    fi
}

# is_host_amd64_elf, get_endianness, check_loongarch_uapi, ensure_loongarch_uapi:
# defined but never called on x86_64-linux-gnu — omitted.

# ─── get_architecture() — called once from main ──────────────────────────────

get_architecture() {
    local _ostype _cputype _bitness _arch _clibtype
    _ostype="$(uname -s)"                          # → Linux
    _cputype="$(uname -m)"                         # → x86_64
    _clibtype="gnu"

    if [ "$_ostype" = Linux ]; then
        # if [ "$(uname -o)" = Android ]           # NOT TAKEN
        # if ldd --version | grep -q 'musl'        # NOT TAKEN (glibc)
        :
    fi
    # if [ "$_ostype" = Darwin ]                   # NOT TAKEN
    # if [ "$_ostype" = SunOS ]                    # NOT TAKEN

    local _current_exe
    case "$_ostype" in
        Linux)
            _current_exe=$(get_current_exe)
            _ostype=unknown-linux-$_clibtype       # → unknown-linux-gnu
            _bitness=$(get_bitness "$_current_exe")  # → 64
            ;;
    esac

    case "$_cputype" in
        x86_64 | x86-64 | x64 | amd64)
            _cputype=x86_64
            ;;
    esac

    # if unknown-linux-gnu + bitness==32           # NOT TAKEN (bitness=64)
    # if unknown-linux-gnueabihf + armv7           # NOT TAKEN

    _arch="${_cputype}-${_ostype}"                 # → x86_64-unknown-linux-gnu
    RETVAL="$_arch"
}

# ─── say/__print/need_cmd/check_cmd/assert_nz/ensure/ignore — the helpers ────

__print() {
    if $_ansi_escapes_are_valid; then
        printf '\33[1m%s:\33[0m %s\n' "$1" "$2" >&2
    else
        printf '%s: %s\n' "$1" "$2" >&2
    fi
}

# warn() and err() defined; not reached on the happy path.

say() {
    if [ "$RUSTUP_QUIET" = "no" ]; then            # taken (quiet stayed at "no")
        __print 'info' "$1" >&2
    fi
}

need_cmd() {
    if ! check_cmd "$1"; then                      # NOT TAKEN (every command exists)
        :
    fi
}

check_cmd() {
    command -v "$1" > /dev/null 2>&1
}

assert_nz() {
    # if [ -z "$1" ]                               # NOT TAKEN (_arch is non-empty)
    :
}

ensure() {
    if ! "$@"; then                                # NOT TAKEN on the success path
        :
    fi
}

ignore() {
    "$@"
}

# ─── downloader() — called TWICE (with --check, then with the real URL) ──────

downloader() {
    is_zsh && setopt local_options shwordsplit     # is_zsh false → setopt skipped
    local _dld _ciphersuites _err _status _retry

    if check_cmd curl; then
        _curl_path=$(command -v curl)
        # snap-curl branch                         # NOT TAKEN
        _dld=curl
    # elif wget                                    # NOT TAKEN
    fi

    if [ "$1" = --check ]; then                    # ── taken on call #1 ──
        need_cmd "$_dld"                           # need_cmd curl
    elif [ "$_dld" = curl ]; then                  # ── taken on call #2 ──
        check_curl_for_retry_support
        _retry="$RETVAL"                           # → "--retry 3 -C -"
        get_ciphersuites_for_curl
        _ciphersuites="$RETVAL"                    # → non-empty OpenSSL suite list

        if [ -n "$_ciphersuites" ]; then
            # shellcheck disable=SC2086
            _err=$(curl $_retry --proto '=https' --tlsv1.2 \
                        --ciphers "$_ciphersuites" \
                        --silent --show-error --fail --location "$1" \
                        --output "$2" 2>&1)
            _status=$?
        fi
        # else-with-fallback warnings              # NOT TAKEN

        # if [ -n "$_err" ]                        # NOT TAKEN on a clean download
        return $_status
    fi
    # wget branch and final "Unknown downloader"   # NOT TAKEN
}

# ─── check_help_for() — called by the two helpers below ──────────────────────

check_help_for() {
    local _arch _cmd _arg
    _arch="$1"; shift
    _cmd="$1"; shift
    local _category
    if "$_cmd" --help | grep -q '"--help all"'; then
        _category="all"                            # one or the other depending on curl ver.
    else
        _category=""
    fi
    case "$_arch" in
        # *darwin*) … ;;                           # NOT TAKEN (arch is "notspecified")
        *) ;;
    esac
    for _arg in "$@"; do
        if ! "$_cmd" --help "$_category" | grep -q -- "$_arg"; then
            return 1                               # NOT TAKEN on a modern curl
        fi
    done
    true
}

check_curl_for_retry_support() {
    local _retry_supported=""
    if check_help_for "notspecified" "curl" "--retry"; then
        _retry_supported="--retry 3"
        if check_help_for "notspecified" "curl" "--continue-at"; then
            _retry_supported="--retry 3 -C -"
        fi
    fi
    RETVAL="$_retry_supported"
}

get_ciphersuites_for_curl() {
    # if [ -n "${RUSTUP_TLS_CIPHERSUITES-}" ]      # NOT TAKEN (unset)
    local _openssl_syntax="no"
    local _gnutls_syntax="no"
    local _backend_supported="yes"
    if curl -V | grep -q ' OpenSSL/'; then
        _openssl_syntax="yes"
    # libressl/boringssl/gnutls branches           # NOT TAKEN
    fi

    local _args_supported="no"
    if [ "$_backend_supported" = "yes" ]; then
        if check_help_for "notspecified" "curl" "--tlsv1.2" "--ciphers" "--proto"; then
            _args_supported="yes"
        fi
    fi

    local _cs=""
    if [ "$_args_supported" = "yes" ]; then
        if [ "$_openssl_syntax" = "yes" ]; then
            _cs=$(get_strong_ciphersuites_for "openssl")
        # elif gnutls                              # NOT TAKEN
        fi
    fi
    RETVAL="$_cs"
}

# get_ciphersuites_for_wget(): defined, never called.

get_strong_ciphersuites_for() {
    if [ "$1" = "openssl" ]; then
        echo "TLS_AES_128_GCM_SHA256:TLS_CHACHA20_POLY1305_SHA256:TLS_AES_256_GCM_SHA384:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384"
    # elif gnutls                                  # NOT TAKEN
    fi
}

# ─── Bottom of the file: the actual entry point ──────────────────────────────

set +u

case "$RUSTUP_INIT_SH_PRINT" in
    # arch | architecture)  …                     # NOT TAKEN (env var unset/empty)
    *)
        main "$@" || exit 1                       # `|| exit 1` only fires if main fails
        ;;
esac
