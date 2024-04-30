FROM rust:bookworm as build
WORKDIR /src
COPY . .


FROM rust as builder
WORKDIR /src
COPY . .
RUN cargo build --release 
RUN mkdir /out && cp -R /src/target/release/bitcoin-p2p-firewalld /out


FROM debian:bookworm
COPY --from=builder /out/bitcoin-p2p-firewalld /usr/local/bin/bitcoin-p2p-firewalld
CMD [ "/usr/local/bin/bitcoin-p2p-firewalld", "-v", "3", "-n", "regtest", "-d", "block,cmpctblock", "18444", "127.0.0.1:28444" ]
