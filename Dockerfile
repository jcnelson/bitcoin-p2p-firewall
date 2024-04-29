FROM rust as builder
WORKDIR /src
COPY . .
RUN rustup target add x86_64-unknown-linux-musl

RUN apt-get update && apt-get install -y git musl-tools

RUN CC=musl-gcc \
    CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
    cargo build --release --workspace --target x86_64-unknown-linux-musl
RUN mkdir /out && cp -R /src/target/x86_64-unknown-linux-musl/release/. /out


FROM alpine
WORKDIR /
RUN apk --no-cache add --update
COPY --from=builder /out/bitcoin-p2p-firewalld /usr/local/bin/bitcoin-p2p-firewalld
CMD [ "/usr/local/bin/bitcoin-p2p-firewalld -n regtest -b block,cmpctblock 18444 bitcoin_regtest.bitcoind:28443" ]
