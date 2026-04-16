FROM rust:bullseye AS builder
ARG VERSION
RUN cargo install go-fish-game-server --version $VERSION --locked

FROM debian:12-slim AS runtime

COPY --from=builder /usr/local/cargo/bin/go-fish-game-server go-fish-game-server
COPY build/config.toml config.toml

# Run as non-root (optional but recommended)
USER 1001:1001

ENTRYPOINT ["./go-fish-game-server", "--config", "config.toml"]