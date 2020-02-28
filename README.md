# Bitcoin P2P Firewall

This small program creates a "firewall" for Bitcoin p2p messages, allowing you
to filter out p2p messages from downstream clients.  It's meant to allow you to
run a Bitcoin regtest node in "read-only" mode, so that clients can download
blocks and ping the node but not reorg the chain.

## Installation

You will need Rust and Cargo.

```
$ cargo build
$ cargo install
```

This package only needs the `mio` crate, which works on most major operating systems.

## Usage

```
$ ./bitcoin-p2p-firewalld [-v DEBUG_VERBOSITY] 
                          [-n NETWORK_NAME]
                          [-b BLACKLISTED_MESSAGES]
                          [-w WHITELISTED_MESSAGES]
                          port
                          upstream_bitcoind_address
```

You can increase verbosity by passing `-v 2`, `-v 3`, or `-v 4`.  The highest
verbosity is `-v 4`.

The valid options for `-n` are `mainnet`, `testnet`, or `regtest`.  If
unspecified, the default is `mainnet`.

Either `-b` or `-w` can be specified but not both.  Both arguments take a
comma-separated list of Bitcoin commands.  Any command string will be accepted,
but see [the list of Bitcoin
commands](https://en.bitcoin.it/wiki/Protocol_documentation#Message_types).

The `port` argument is the _public_ port to bind on.  Use the port your
`bitcoind` would otherwise have bound on.

The `upstream_bitcoind_address` is the `host:port` of your upstream `bitcoind`
node's p2p address.

### Example

```
$ ./bitcoin-p2p-firewalld -v 4 -n regtest -b block,cmpctblock 18444 127.0.0.1:18445
```

Here, the firewall will bind on port 18444, and will send filtered messages to
127.0.0.1:18445.  Blocks and compact blocks will be filtered from remote
clients, thereby ensuring that the `bitcoind` node running on 127.0.0.1:18445
will never see them
