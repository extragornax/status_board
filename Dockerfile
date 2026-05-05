FROM rust:1-slim-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src/ src/
COPY templates/ templates/

RUN cargo build --release --locked || cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates wget && rm -rf /var/lib/apt/lists/*

RUN useradd -u 1000 -m app
USER app
WORKDIR /app

COPY --from=builder /app/target/release/status_board /app/status_board
COPY services.json /app/services.json

ENV PORT=3000
ENV RUST_LOG=info,status_board=info
ENV SERVICES_CONFIG=services.json

EXPOSE 3000

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD wget -qO- http://localhost:3000/health || exit 1

CMD ["./status_board"]
