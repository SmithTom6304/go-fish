FROM rust:bullseye AS builder
LABEL authors="tom"

# Cache dependencies first
COPY Cargo.toml ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

# Build actual app
COPY ./src ./src
RUN cargo build --release

FROM debian:12-slim AS runtime

# Copy binary from builder
COPY --from=builder target/release/go-fish-game-server go-fish-game-server
COPY build/config.toml config.toml

# Run as non-root (optional but recommended)
USER 1001:1001

ENTRYPOINT ["./go-fish-game-server", "--config", "config.toml"]