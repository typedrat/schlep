ARG UBUNTU_RELEASE=24.04
ARG CHISEL_RELEASE=1.1.0

FROM ubuntu:$UBUNTU_RELEASE AS os_builder
ARG UBUNTU_RELEASE
ARG CHISEL_RELEASE
ARG TARGETARCH

ADD "https://github.com/canonical/chisel/releases/download/v${CHISEL_RELEASE}/chisel_v${CHISEL_RELEASE}_linux_${TARGETARCH}.tar.gz" chisel.tar.gz
RUN tar xvf chisel.tar.gz -C /usr/bin
RUN \
    apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y ca-certificates
WORKDIR /rootfs
RUN \
  chisel cut --release ubuntu-$UBUNTU_RELEASE --root /rootfs \
      base-files_base \
      base-files_release-info \
      ca-certificates_data \
      libgcc-s1_libs \
      libc6_libs

FROM ghcr.io/rust-lang/rust:nightly AS chef
RUN cargo install cargo-chef
WORKDIR app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin schlep

FROM scratch
LABEL org.opencontainers.image.description="Schlep is the less-is-more SFTP server that is designed to integrate with your existing systems, not replace them."
LABEL org.opencontainers.image.source=https://github.com/typedrat/schlep

COPY --from=os_builder /rootfs /
WORKDIR app
COPY --from=builder /app/target/release/schlep /usr/local/bin/schlep
ENTRYPOINT ["/usr/local/bin/schlep"]
