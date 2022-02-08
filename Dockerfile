FROM clux/muslrust as cargo-build

RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /usr/src/oggify

COPY Cargo.toml Cargo.toml

RUN mkdir src/

RUN echo "fn main() {println!(\"if you see this, the build broke\")}" > src/main.rs

RUN RUSTFLAGS=-Clinker=musl-gcc cargo build --release --target=x86_64-unknown-linux-musl

RUN rm -f target/x86_64-unknown-linux-musl/release/deps/oggify*

COPY . .

RUN RUSTFLAGS=-Clinker=musl-gcc cargo build --release --target=x86_64-unknown-linux-musl

FROM alfg/ffmpeg:latest

RUN apk update

RUN apk add --no-cache vorbis-tools xxd coreutils

COPY --from=cargo-build /usr/src/oggify/target/x86_64-unknown-linux-musl/release/oggify /usr/local/bin/oggify

ENV PATH_DIR=/data/

ENTRYPOINT ["oggify"]
# docker run --rm -i -v "$(pwd)":/data oggify elganzua124 Curuzucuatia21 < tracks_file
