# Build from the go-fish-tui-client directory:
#   docker build -f go-fish-tui-client/Dockerfile go-fish-tui-client
FROM rust:1.94.1-bookworm AS builder

# Install the wasm32 target
RUN rustup target add wasm32-unknown-unknown

# Grab cargo binstall
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash

# Install trunk; it will download the correct wasm-bindgen-cli version automatically.
RUN cargo binstall trunk

# The getrandom wasm_js rustflag from the workspace .cargo/config.toml is not
# in scope when building from this crate's directory, so mirror it here.
ENV CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS='--cfg getrandom_backend="wasm_js"'

WORKDIR /app

# --- Dependency caching layer ---
# COPY Cargo.toml ./
# RUN mkdir src && echo "fn main() {}" > src/main.rs
# RUN cargo build --target wasm32-unknown-unknown --release
# RUN rm -rf src

# --- Build ---
COPY Trunk.toml index.html ./
COPY Cargo.toml ./
COPY src ./src

RUN trunk build --release --locked
# TRUNK_TOOLS_WASM_BINDGEN="0.2.114"

# --- Runtime ---
FROM nginx:alpine AS runtime
COPY --from=builder /app/dist /usr/share/nginx/html
