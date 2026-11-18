# syntax=docker/dockerfile:1
# Kitsunarr — multi-stage build. The runtime image contains one static-ish binary plus CA
# certificates; no Node.js, Python or .NET.

ARG RUST_VERSION=1.99.1
ARG DX_VERSION=0.7.10
# SHA-256 of dx-x86_64-unknown-linux-gnu.tar.gz from the Dioxus v0.7.10 release.
ARG DX_SHA256=4363e4ed2a3f1eb7f4d38d2d59aed59ce43271c44c16b425e92c89a64761fbe7

# The build stage runs on the build machine's own architecture and cross-compiles the server
# for the image's (`TARGETARCH`): a multi-architecture build (linux/amd64 and linux/arm64)
# emulates only the runtime stage's package installation, never the Rust compiler.
FROM --platform=$BUILDPLATFORM docker.io/library/rust:${RUST_VERSION}-trixie AS build
ARG DX_VERSION
ARG DX_SHA256
ARG TARGETARCH
# dx probes `cargo +nightly` and falls back to the pinned toolchain when it is missing; without
# this, rustup downloads a whole nightly toolchain first, which can stall the build.
ENV RUSTUP_AUTO_INSTALL=0
WORKDIR /src
RUN case "$TARGETARCH" in \
      amd64|"") echo x86_64-unknown-linux-gnu > /rust-target ;; \
      arm64) echo aarch64-unknown-linux-gnu > /rust-target \
        && apt-get update \
        && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
        && rm -rf /var/lib/apt/lists/* ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
 && rustup target add "$(cat /rust-target)" \
 && rustup target add wasm32-unknown-unknown \
 && curl -fsSL -o /tmp/dx.tar.gz "https://github.com/DioxusLabs/dioxus/releases/download/v${DX_VERSION}/dx-x86_64-unknown-linux-gnu.tar.gz" \
 && echo "${DX_SHA256}  /tmp/dx.tar.gz" | sha256sum -c - \
 && tar -xzf /tmp/dx.tar.gz -C /usr/local/bin dx \
 && rm /tmp/dx.tar.gz
COPY . .
ARG KITSUNARR_GIT_REVISION=
ENV KITSUNARR_GIT_REVISION=${KITSUNARR_GIT_REVISION} \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    dx build -p kitsunarr-web --release --profile web-release \
 && cargo build --release --locked -p kitsunarr-server --target "$(cat /rust-target)" \
 && install -m 0755 "target/$(cat /rust-target)/release/kitsunarr" /kitsunarr

FROM docker.io/library/debian:trixie-slim
# ffprobe (from ffmpeg) reads media information from video files, as Sonarr's bundled one.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tzdata ffmpeg \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /config /usr/share/doc/kitsunarr \
 && chown 1000:1000 /config
COPY --from=build /kitsunarr /usr/local/bin/kitsunarr
COPY LICENSE COPYRIGHT.md /usr/share/doc/kitsunarr/
ENV KITSUNARR_CONFIG_DIR=/config \
    KITSUNARR_DATA_DIR=/config \
    KITSUNARR_BIND_ADDRESS=:: \
    KITSUNARR_PORT=8990 \
    TZ=Etc/UTC
USER 1000:1000
VOLUME ["/config"]
EXPOSE 8990
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 CMD ["/usr/local/bin/kitsunarr", "healthcheck"]
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/kitsunarr"]
CMD ["serve"]
LABEL org.opencontainers.image.title="Kitsunarr" \
      org.opencontainers.image.description="A modern Rust-powered media manager for TV, anime and movies." \
      org.opencontainers.image.source="https://github.com/KvasFan/Kitsunarr" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"
