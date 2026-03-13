# ── Stage 1: Build ───────────────────────────────────────────────────────────
FROM rust:1.82-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependency compilation (re-runs only when Cargo.toml / Cargo.lock changes)
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release 2>&1
RUN rm -rf src

# Build actual source
COPY src ./src
COPY migrations ./migrations
RUN touch src/main.rs && cargo build --release

# ── Stage 2: Runtime ─────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/nexusgate ./nexusgate
COPY migrations ./migrations
COPY dashboard ./dashboard

# Non-root user for security
RUN useradd -m -u 1001 nexusgate && \
    mkdir -p /app/data && \
    chown -R nexusgate:nexusgate /app

USER nexusgate

EXPOSE 8080

HEALTHCHECK --interval=15s --timeout=5s --start-period=10s \
  CMD wget -qO- http://localhost:8080/health || exit 1

CMD ["./nexusgate"]
