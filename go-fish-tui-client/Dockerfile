# Build from the workspace root:
#   docker build -f go-fish-tui-client/Dockerfile .
FROM rust:1.94.1-bookworm AS builder

# Install the wasm32 target
RUN rustup target add wasm32-unknown-unknown

# Grab cargo binstall
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash

# Install trunk; it will download the correct wasm-bindgen-cli version automatically.
RUN cargo binstall trunk

WORKDIR /app

# Copy workspace manifests and all crates needed for the WASM build
COPY Cargo.toml Cargo.lock ./
COPY go-fish ./go-fish
COPY go-fish-web ./go-fish-web
COPY go-fish-game-server ./go-fish-game-server
COPY go-fish-tui-client ./go-fish-tui-client

# --- Build ---
WORKDIR /app/go-fish-tui-client
RUN CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS='--cfg getrandom_backend="wasm_js"' trunk build --release --locked
# TRUNK_TOOLS_WASM_BINDGEN="0.2.114"

# --- Runtime ---
FROM nginx:alpine AS runtime
COPY --from=builder /app/go-fish-tui-client/dist /usr/share/nginx/html
