FROM rust:bullseye AS build

COPY Cargo.lock Cargo.lock
COPY Cargo.toml Cargo.toml
COPY src src

RUN cargo build --bin pond --release

FROM debian:bullseye-slim
EXPOSE 8001

COPY --from=build target/release/pond .

ENTRYPOINT ["./pond"]