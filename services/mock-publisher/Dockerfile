# Build context: repo root
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY services ./services
RUN cargo build --release -p mock-publisher

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/mock-publisher /usr/local/bin/mock-publisher
ENTRYPOINT ["/usr/local/bin/mock-publisher"]
