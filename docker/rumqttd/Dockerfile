# rumqttd built from source. The Docker Hub image is outdated and panics on
# modern config files (known issue), so we build our own from the upstream tag.
# Rust pinned to the toolchain contemporary with the rumqttd tag: newer rustc
# rejects a borrow pattern in the old `metrics` dependency (rust#141402).
FROM rust:1.81-bookworm AS builder
ARG RUMQTTD_TAG=rumqttd-0.20.0
RUN git clone --depth 1 --branch ${RUMQTTD_TAG} https://github.com/bytebeamio/rumqtt /src
WORKDIR /src
RUN cargo build --release -p rumqttd

FROM debian:bookworm-slim
COPY --from=builder /src/target/release/rumqttd /usr/local/bin/rumqttd
EXPOSE 1883 3030 9042
ENTRYPOINT ["/usr/local/bin/rumqttd"]
CMD ["--config", "/etc/rumqttd/rumqttd.toml"]
