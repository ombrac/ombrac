FROM rust:latest AS builder

RUN apt update && \
    apt install -y \
      musl-tools \
      cmake \
      clang \
      gcc-x86-64-linux-gnu \
      gcc-aarch64-linux-gnu && \
    rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

WORKDIR /usr/src/ombrac
COPY . .

ARG TARGETARCH

RUN case ${TARGETARCH} in \
      "amd64") export RUST_TARGET="x86_64-unknown-linux-musl" ;; \
      "arm64") export RUST_TARGET="aarch64-unknown-linux-musl" ;; \
      *) echo "Unsupported TARGETARCH: ${TARGETARCH}"; exit 1 ;; \
    esac && \
    echo "Building for RUST_TARGET: ${RUST_TARGET}" && \
    cargo build --release --target ${RUST_TARGET} --bin ombrac-server && \
    cargo build --release --target ${RUST_TARGET} --bin ombrac-client && \
    mkdir -p /usr/src/ombrac/binaries && \
    mv /usr/src/ombrac/target/${RUST_TARGET}/release/ombrac-server /usr/src/ombrac/binaries/ombrac-server && \
    mv /usr/src/ombrac/target/${RUST_TARGET}/release/ombrac-client /usr/src/ombrac/binaries/ombrac-client


FROM alpine:latest AS ombrac-server
ARG TARGETARCH
COPY --from=builder /usr/src/ombrac/binaries/ombrac-server /usr/local/bin/
RUN chmod +x /usr/local/bin/ombrac-server
ENTRYPOINT ["/usr/local/bin/ombrac-server"]


FROM alpine:latest AS ombrac-client
ARG TARGETARCH
COPY --from=builder /usr/src/ombrac/binaries/ombrac-client /usr/local/bin/
RUN chmod +x /usr/local/bin/ombrac-client
ENTRYPOINT ["/usr/local/bin/ombrac-client"]
