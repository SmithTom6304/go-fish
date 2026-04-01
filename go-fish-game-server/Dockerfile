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

FROM debian:12-slim as runtime

# Copy binary from builder
COPY --from=builder target/release/go-fish-game-server go-fish-game-server

# Run as non-root (optional but recommended)
USER nonroot:nonroot

ENTRYPOINT ["go-fish-game-server"]

ENTRYPOINT ["top", "-b"]