# Build context: repo root
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY services ./services
RUN cargo build --release -p ingester

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/ingester /usr/local/bin/ingester
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/ingester"]
