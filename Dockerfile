FROM clux/muslrust:1.83.0-stable AS chef
USER root
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY ./Cargo.toml ./
COPY ./src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl --bin oggify

FROM alpine:3.21.0 AS runtime
RUN apk update
RUN apk add --no-cache vorbis-tools xxd coreutils
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/oggify /usr/local/bin/
COPY tag_ogg.sh /usr/local/bin/tag_ogg.sh
RUN chmod +x /usr/local/bin/tag_ogg.sh
ENTRYPOINT ["oggify"]