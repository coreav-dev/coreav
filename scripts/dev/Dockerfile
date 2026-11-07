FROM debian:trixie-slim

ARG TARGETARCH
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    bash build-essential ca-certificates clang cmake curl git libssl-dev \
    nodejs npm pkg-config python3 python3-venv xz-utils \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rustup
ENV PATH=/opt/cargo/bin:/opt/oracle/bin:$PATH
COPY rust-toolchain.toml /opt/dev/rust-toolchain.toml
RUN case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-gnu; sha=20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c ;; \
      arm64) target=aarch64-unknown-linux-gnu; sha=e3853c5a252fca15252d07cb23a1bdd9377a8c6f3efa01531109281ae47f841c ;; \
      *) exit 1 ;; \
    esac \
    && curl --proto '=https' --tlsv1.2 -fsSL "https://static.rust-lang.org/rustup/archive/1.28.2/$target/rustup-init" -o /tmp/rustup-init \
    && echo "$sha  /tmp/rustup-init" | sha256sum --check --strict \
    && chmod 755 /tmp/rustup-init \
    && CARGO_HOME=/opt/cargo /tmp/rustup-init -y --profile minimal --default-toolchain none \
    && cd /opt/dev && CARGO_HOME=/opt/cargo rustup show active-toolchain \
    && rm /tmp/rustup-init

COPY scripts/ /opt/dev/scripts/
RUN case "$TARGETARCH" in \
      amd64) bash /opt/dev/scripts/install-oracle-ffmpeg.sh /opt/oracle/bin ;; \
      arm64) ;; \
      *) exit 1 ;; \
    esac

RUN useradd --create-home --uid 1000 dev \
    && mkdir -p /home/dev/.cargo/registry /target \
    && chown -R dev:dev /home/dev/.cargo /target
ENV HOME=/home/dev
ENV CARGO_HOME=/home/dev/.cargo
ENV CARGO_TARGET_DIR=/target
ENV CARGO_BUILD_JOBS=2
USER dev
WORKDIR /work
CMD ["bash"]
