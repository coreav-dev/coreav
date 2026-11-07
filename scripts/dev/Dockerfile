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
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh \
    && CARGO_HOME=/opt/cargo sh /tmp/rustup.sh -y --profile minimal --default-toolchain none \
    && cd /opt/dev && CARGO_HOME=/opt/cargo rustup show active-toolchain \
    && rm /tmp/rustup.sh

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
