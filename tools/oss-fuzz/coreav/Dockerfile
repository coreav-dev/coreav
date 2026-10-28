# OSS-Fuzz build image for CoreAV (tools/oss-fuzz/README.md). The base image
# has a nightly Rust and cargo-fuzz; build.sh needs python3 and cargo only.
FROM gcr.io/oss-fuzz-base/base-builder-rust
# The public repository, which exists from the public reset on.
RUN git clone --depth 1 https://github.com/coreav-dev/coreav.git coreav
WORKDIR $SRC/coreav
COPY build.sh $SRC/
