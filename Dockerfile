# Using the `rust-musl-builder` as base image, instead of 
# the official Rust toolchain
FROM clux/muslrust:stable AS chef
USER root
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY ./Cargo.toml ./
COPY ./src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
COPY --from=planner /app/recipe.json recipe.json
# Notice that we are specifying the --target flag!
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl --bin oggify

FROM alfg/ffmpeg:latest AS runtime

RUN apk update
RUN apk add --no-cache vorbis-tools xxd coreutils
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/oggify /usr/local/bin/app
ENV PATH_DIR=/data/
ENTRYPOINT ["/usr/local/bin/app"]