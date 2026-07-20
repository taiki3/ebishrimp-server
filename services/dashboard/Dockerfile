# Build context: repository root.
#   docker build -f services/dashboard/Dockerfile .

FROM rust:1-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY services ./services
RUN cargo build --release -p dashboard

# Static assets (htmx / ECharts / CSS) are embedded in the binary via
# include_bytes!/include_str!, so the runtime image only needs the binary.
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/dashboard /usr/local/bin/dashboard
ENV LISTEN_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/dashboard"]
