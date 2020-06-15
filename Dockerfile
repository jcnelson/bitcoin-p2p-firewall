FROM rust as builder
WORKDIR /src/bitcoin-p2p-firewall
COPY . .
RUN cargo install --target x86_64-unknown-linux-gnu --path /src/bitcoin-p2p-firewall


FROM alpine
WORKDIR /
RUN apk --no-cache add --update 
COPY --from=builder /usr/local/cargo/bin/bitcoin-p2p-firewalld /usr/local/bin/bitcoin-p2p-firewalld
CMD [ "/usr/local/bin/bitcoin-p2p-firewalld -n regtest -b block,cmpctblock 18444 bitcoin_regtest.bitcoind:28443" ]
