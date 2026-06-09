# Reproduces, by hand, what
#
#   rustup-init --verbose -y --no-modify-path --profile minimal \
#               --default-toolchain 1.95 --default-host x86_64-unknown-linux-gnu
#
# does on a fresh x86_64-linux-gnu system, using only the URLs traced earlier:
#
#   • channel-rust-1.95.toml (+ .sha256)
#   • rustc-1.95.0-x86_64-unknown-linux-gnu.tar.xz
#   • rust-std-1.95.0-x86_64-unknown-linux-gnu.tar.xz
#   • cargo-1.95.0-x86_64-unknown-linux-gnu.tar.xz
#
# All three archives use rust-installer layout, so a single shell `install.sh`
# inside each tarball does the file moves into the toolchain prefix.
#
# This Dockerfile deliberately does NOT run the real rustup-init binary; it
# performs each step that maybe_install_rust → Manifestation::update would
# perform, with the same final directory layout.

FROM debian:latest

ARG RUST_VERSION=1.95.0
ARG HOST_TRIPLE=x86_64-unknown-linux-gnu
ARG DIST_ROOT=https://static.rust-lang.org/dist

# Same env vars rustup itself would honour. We're running as root in the
# container, so $HOME is /root.
ENV RUSTUP_HOME=/root/.rustup \
    CARGO_HOME=/root/.cargo \
    PATH=/root/.cargo/bin:${PATH}

# Tools needed to fetch and verify the archives. xz-utils for .tar.xz,
# ca-certificates for the TLS handshake against static.rust-lang.org,
# coreutils for sha256sum. gcc is the "default linker" rustup warns about
# when missing — included so `cargo build` works out of the box.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
        ca-certificates curl xz-utils gcc libc6-dev \
 && rm -rf /var/lib/apt/lists/*

# Toolchain prefix that rustup would create at
# $RUSTUP_HOME/toolchains/<channel>-<host>/
ENV TOOLCHAIN_DIR=${RUSTUP_HOME}/toolchains/1.95-${HOST_TRIPLE}

RUN set -eux; \
    mkdir -p \
        "${RUSTUP_HOME}/downloads" \
        "${RUSTUP_HOME}/tmp" \
        "${RUSTUP_HOME}/update-hashes" \
        "${TOOLCHAIN_DIR}/lib/rustlib" \
        "${CARGO_HOME}/bin"; \
    \
    cd "${RUSTUP_HOME}/tmp"; \
    \
    # ── 1. Download channel manifest and its sha256 sidecar ──────────────── \
    curl -fsSL -o channel.toml        "${DIST_ROOT}/channel-rust-1.95.toml"; \
    curl -fsSL -o channel.toml.sha256 "${DIST_ROOT}/channel-rust-1.95.toml.sha256"; \
    \
    # Verify the manifest. The .sha256 file is `<hash>  <filename>`, but the \
    # filename column doesn't match our local name, so sed it before checking. \
    sed "s|  .*$|  channel.toml|" channel.toml.sha256 | sha256sum -c -; \
    \
    # Record the manifest hash exactly where Manifestation::update would \
    # (`opts.update_hash`), so a future `rustup update` here would short-circuit. \
    awk '{print $1}' channel.toml.sha256 \
        > "${RUSTUP_HOME}/update-hashes/1.95-${HOST_TRIPLE}"; \
    \
    # Final manifest copy lives inside the toolchain. \
    install -m 0644 channel.toml \
        "${TOOLCHAIN_DIR}/lib/rustlib/multirust-channel-manifest.toml"; \
    \
    # ── 2. Pull the [pkg.<name>.target.<triple>] {url,hash} pairs for the \
    #      three components in the `minimal` profile. We need a tiny TOML \
    #      reader here; Debian's python3 is too heavy to install just for \
    #      this, so we use a portable awk state machine instead. \
    extract_url_hash() { \
        local pkg="$1"; \
        awk -v pkg="${pkg}" -v triple="${HOST_TRIPLE}" ' \
            $0 == "[pkg." pkg ".target." triple "]" { in_block=1; next } \
            /^\[/ { in_block=0 } \
            in_block && $1 == "xz_url"  { gsub(/"/,"",$3); url=$3 } \
            in_block && $1 == "xz_hash" { gsub(/"/,"",$3); hash=$3 } \
            END { print url " " hash } \
        ' channel.toml; \
    }; \
    \
    # ── 3. For each component: download, verify, extract, run install.sh ── \
    for pkg in rustc rust-std cargo; do \
        read -r url hash <<EOF \
$(extract_url_hash "${pkg}") \
EOF \
        : "${url:?manifest missing ${pkg} url}" "${hash:?manifest missing ${pkg} hash}"; \
        \
        # Download into the hash-keyed cache. \
        archive="${RUSTUP_HOME}/downloads/${hash}"; \
        curl -fsSL -o "${archive}" "${url}"; \
        \
        # Verify against the hash quoted in the manifest. \
        echo "${hash}  ${archive}" | sha256sum -c -; \
        \
        # Unpack into a temp dir, run the rust-installer install.sh which \
        # walks the component's manifest.in and copies files into the \
        # toolchain prefix. --disable-ldconfig because rustup also skips it. \
        stage="${RUSTUP_HOME}/tmp/${pkg}-stage"; \
        mkdir -p "${stage}"; \
        tar -xJf "${archive}" -C "${stage}" --strip-components=1; \
        "${stage}/install.sh" \
            --prefix="${TOOLCHAIN_DIR}" \
            --disable-ldconfig; \
        rm -rf "${stage}"; \
        \
        # Drop the cached archive, matching download_cfg.clean(). \
        rm -f "${archive}"; \
    done; \
    \
    # ── 4. multirust-config.toml: the component/target inventory ────────── \
    cat > "${TOOLCHAIN_DIR}/lib/rustlib/multirust-config.toml" <<EOF; \
config_version = "1"\n\
\n\
[[components]]\n\
pkg = "rustc"\n\
target = "${HOST_TRIPLE}"\n\
\n\
[[components]]\n\
pkg = "rust-std"\n\
target = "${HOST_TRIPLE}"\n\
\n\
[[components]]\n\
pkg = "cargo"\n\
target = "${HOST_TRIPLE}"\n\
EOF \
    \
    # ── 5. settings.toml: rustup's global state, in $RUSTUP_HOME root. ─── \
    cat > "${RUSTUP_HOME}/settings.toml" <<EOF; \
default_host_triple = "${HOST_TRIPLE}"\n\
default_toolchain = "1.95-${HOST_TRIPLE}"\n\
profile = "minimal"\n\
version = "12"\n\
\n\
[overrides]\n\
EOF \
    \
    # ── 6. Clean up the tmp dir we used as a scratch space. ─────────────── \
    rm -rf "${RUSTUP_HOME}/tmp"/*; \
    \
    # ── 7. Sanity check, mirroring check_proxy_sanity (cargo, rustc only). ─ \
    "${TOOLCHAIN_DIR}/bin/rustc" --version; \
    "${TOOLCHAIN_DIR}/bin/cargo" --version

# The proxies under $CARGO_HOME/bin would normally be the rustup multiplexer
# itself. We don't have a rustup binary here (we skipped install_bins by
# choice), so symlink straight to the toolchain. This is good enough for
# `cargo`, `rustc`, `rustdoc`; it's not a real rustup install.
RUN set -eux; \
    for tool in cargo rustc rustdoc; do \
        ln -sf "${TOOLCHAIN_DIR}/bin/${tool}" "${CARGO_HOME}/bin/${tool}"; \
    done

# Smoke test in the final image.
RUN rustc --version && cargo --version
