/*
 copyright: (c) 2013-2020 by Blockstack PBC, a public benefit corporation.

 This file is part of Blockstack.

 Blockstack is free software. You may redistribute or modify
 it under the terms of the GNU General Public License as published by
 the Free Software Foundation, either version 3 of the License or
 (at your option) any later version.

 Blockstack is distributed in the hope that it will be useful,
 but WITHOUT ANY WARRANTY, including without the implied warranty of
 MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 GNU General Public License for more details.

 You should have received a copy of the GNU General Public License
 along with Blockstack. If not, see <http://www.gnu.org/licenses/>.
*/

#![allow(unused_macros)]

extern crate mio;

use std::io;
use std::io::{Read, Write};
use std::fmt;
use std::error;
use std::mem;

use mio::net as mio_net;
use mio::Ready;
use mio::Token;
use mio::PollOpt;

use std::process;
use std::env;
use std::collections::HashMap;
use std::net;
use std::net::SocketAddr;
use std::net::Shutdown;
use std::time::Duration;
use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOG_DEBUG : u8 = 4;
pub const LOG_INFO : u8 = 3;
pub const LOG_WARN : u8 = 2;
pub const LOG_ERROR : u8 = 1;

#[cfg(test)] pub const BUF_SIZE : usize = 16;
#[cfg(not(test))] pub const BUF_SIZE : usize = 65536;

// per-thread log level and log format
thread_local!(static LOGLEVEL: RefCell<u8> = RefCell::new(LOG_DEBUG));

pub fn set_loglevel(ll: u8) -> Result<(), String> {
    LOGLEVEL.with(move |level| {
        match ll {
            LOG_ERROR..=LOG_DEBUG => {
                *level.borrow_mut() = ll;
                Ok(())
            },
            _ => {
                Err("Invalid log level".to_string())
            }
        }
    })
}

pub fn get_loglevel() -> u8 {
    let mut res = 0;
    LOGLEVEL.with(|lvl| {
        res = *lvl.borrow();
    });
    res
}

pub const IDLE_TIMEOUT : u128 = 120000;       // 2 minutes idle without any I/O? kill the socket

macro_rules! debug {
    ($($arg:tt)*) => ({
        if get_loglevel() >= LOG_DEBUG {
            use std::time::SystemTime;
            let (ts_sec, ts_msec) = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                Ok(n) => (n.as_secs(), n.subsec_nanos() / 1_000_000),
                Err(_) => (0, 0)
            };
            eprintln!("DEBUG [{}.{:03}] [{}:{}] {}", ts_sec, ts_msec, file!(), line!(), format!($($arg)*));
        }
    })
}

macro_rules! info {
    ($($arg:tt)*) => ({
        if get_loglevel() >= LOG_INFO {
            use std::time::SystemTime;
            let (ts_sec, ts_msec) = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                Ok(n) => (n.as_secs(), n.subsec_nanos() / 1_000_000),
                Err(_) => (0, 0)
            };
            eprintln!("INFO [{}.{:03}] [{}:{}] {}", ts_sec, ts_msec, file!(), line!(), format!($($arg)*));
        }
    })
}

macro_rules! warn {
    ($($arg:tt)*) => ({
        if get_loglevel() >= LOG_WARN {
            use std::time::SystemTime;
            let (ts_sec, ts_msec) = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                Ok(n) => (n.as_secs(), n.subsec_nanos() / 1_000_000),
                Err(_) => (0, 0)
            };
            eprintln!("WARN [{}.{:03}] [{}:{}] {}", ts_sec, ts_msec, file!(), line!(), format!($($arg)*));
        }
    })
}

macro_rules! error {
    ($($arg:tt)*) => ({
        if get_loglevel() >= LOG_ERROR {
            use std::time::SystemTime;
            let (ts_sec, ts_msec) = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                Ok(n) => (n.as_secs(), n.subsec_nanos() / 1_000_000),
                Err(_) => (0, 0)
            };
            eprintln!("ERROR [{}.{:03}] [{}:{}] {}", ts_sec, ts_msec, file!(), line!(), format!($($arg)*));
        }
    })
}

const SERVER : Token = mio::Token(0);

pub fn get_epoch_time_ms() -> u128 {
    let start = SystemTime::now();
    let since_the_epoch = start.duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    return since_the_epoch.as_millis();
}

#[derive(Debug)]
pub enum Error {
    /// Failed to read
    ReadError(io::Error),
    /// Failed to write
    WriteError(io::Error),
    /// Blocked
    Blocked,
    /// Receive timed out 
    RecvTimeout,
    /// Not connected to peer
    ConnectionBroken,
    /// Connection could not be (re-)established
    ConnectionError,
    /// Failed to bind
    BindError,
    /// Failed to poll 
    PollError,
    /// Failed to accept 
    AcceptError,
    /// Failed to register socket with poller 
    RegisterError,
    /// Failed to query socket metadata 
    SocketError,
    /// server is not bound to a socket
    NotConnected,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::ReadError(ref io) => fmt::Display::fmt(io, f),
            Error::WriteError(ref io) => fmt::Display::fmt(io, f),
            Error::RecvTimeout => write!(f, "Packet receive timeout"),
            Error::ConnectionBroken => write!(f, "connection to peer node is broken"),
            Error::ConnectionError => write!(f, "connection to peer could not be (re-)established"),
            Error::BindError => write!(f, "Failed to bind to the given address"),
            Error::PollError => write!(f, "Failed to poll"),
            Error::AcceptError => write!(f, "Failed to accept connection"),
            Error::RegisterError => write!(f, "Failed to register socket with poller"),
            Error::SocketError => write!(f, "Socket error"),
            Error::NotConnected => write!(f, "Not connected to peer network"),
            Error::Blocked => write!(f, "EWOULDBLOCK"),
        }
    }
}

impl error::Error for Error {
    fn cause(&self) -> Option<&dyn error::Error> {
        match *self {
            Error::ReadError(ref io) => Some(io),
            Error::WriteError(ref io) => Some(io),
            Error::RecvTimeout => None,
            Error::ConnectionBroken => None,
            Error::ConnectionError => None,
            Error::BindError => None,
            Error::PollError => None,
            Error::AcceptError => None,
            Error::RegisterError => None,
            Error::SocketError => None,
            Error::NotConnected => None,
            Error::Blocked => None,
        }
    }
}

/// Polling state from the network.
/// Binds event IDs to newly-accepted sockets.
/// Lists event IDs with I/O readiness.
pub struct NetworkPollState {
    pub new: HashMap<usize, mio_net::TcpStream>,
    pub ready: Vec<usize>
}

impl NetworkPollState {
    pub fn new() -> NetworkPollState {
        NetworkPollState {
            new: HashMap::new(),
            ready: vec![]
        }
    }
}

/// State of the network.
pub struct NetworkState {
    poll: mio::Poll,
    server: mio_net::TcpListener,
    events: mio::Events,
    count: usize,
}

/// The "message type" field in a Bitcoin message header.  Always 12 bytes; contains a
/// null-terminated ASCII string with the request.
#[derive(Debug, Clone, PartialEq)]
pub struct BitcoinCommand(pub [u8; 12]);

impl BitcoinCommand {
    pub fn from_str(s: &str) -> Option<BitcoinCommand> {
        if s.len() > 12 {
            None
        }
        else {
            let mut bytes = [0u8; 12];
            let strbytes = s.as_bytes();
            for i in 0..strbytes.len() {
                bytes[i] = strbytes[i];
            }
            Some(BitcoinCommand(bytes))
        }
    }

    pub fn to_string(&self) -> String {
        let mut printable = vec![];
        for i in 0..12 {
            if self.0[i] != 0 {
                printable.push(self.0[i]);
            }
            else {
                break;
            }
        }
        String::from_utf8(printable).unwrap_or("#UNKNOWN".to_string())
    }
}

/// Firewall forwarding state.
/// Used both for relaying messages from downstream clients to the upstream bitcoind,
/// and from the upstream bitcoind back down to a downstream client.
pub struct ForwardState {
    /// Buffer to store an incoming Bitcoin message header ("preamble")
    preamble_buf: Vec<u8>,
    /// Buffer to store outgoing bytes to be sent
    write_buf: Vec<u8>,
    /// Pointer into write_buf as to where to start writing again, in case of EWOULDBLOCK
    write_buf_ptr: usize,
    /// Parsed Bitcoin message header.  Instantiated only if we're reading the payload.
    preamble: Option<BitcoinPreamble>,
    /// Number of bytes consumed from the message payload.
    num_read: u64,
    /// Whether or not the message will be dropped.
    filtered: bool,
    /// Timestamp, in milliseconds since the Epoch, when we last read or wrote at least 1 byte
    /// using this forwarding state.
    last_io_at: u128,
    /// Forwarding state is associated with *two* sockets -- the downstream client socket, and the
    /// upstream bitcoind socket.  This is the event ID of the _other_ socket this forwarding state
    /// is bound to in the BitcoinProxy struct below.
    pair_event_id: usize,
}

pub struct BitcoinProxy {
    /// What are the expected magic bytes in each Bitcoin message?
    magic: u32,
    /// Network I/O state
    network: NetworkState,
    /// Address of the upstream bitcoind node
    backend_addr: SocketAddr,
    /// List of Bitcoin commands that are prohibited.  All else are allowed.  If this has items,
    /// then whitelist will be empty.
    blacklist: Vec<BitcoinCommand>,
    /// List of Bitcoin commands that are allowed.  All else are pohibited.  If this has items,
    /// then blacklist will be empty.
    whitelist: Vec<BitcoinCommand>,

    /// Map event IDs to their downstream sockets and their forwarding states.
    downstream: HashMap<usize, mio_net::TcpStream>,
    messages_downstream: HashMap<usize, ForwardState>,
    
    /// Map event IDs to their upstream sockets and their forwarding states.
    upstream: HashMap<usize, mio_net::TcpStream>,
    messages_upstream: HashMap<usize, ForwardState>,
}

/// Bitcoin message header.  Note that it's fixed-sized.
#[derive(Debug, PartialEq)]
pub struct BitcoinPreamble {
    magic: u32,
    command: BitcoinCommand,
    length: u32,
    checksum: u32
}

impl BitcoinPreamble {
    #[cfg(test)]
    pub fn new(magic: u32, command: &str, length: u32, checksum: u32) -> BitcoinPreamble {
        BitcoinPreamble {
            magic: magic,
            command: BitcoinCommand::from_str(command).unwrap(),
            length: length, 
            checksum: checksum
        }
    }
}

impl NetworkState {
    /// Start listening on the given address.
    fn bind_address(addr: &SocketAddr) -> Result<mio_net::TcpListener, Error> {
        mio_net::TcpListener::bind(addr)
            .map_err(|e| {
                error!("Failed to bind to {:?}: {:?}", addr, e);
                Error::BindError
            })
    }

    /// Allocate the next unused network ID.  Note that there is no roll-over logic (but on 64-bit
    /// systems, usize is a 64-bit integer, so this really shouldn't be a problem).
    pub fn next_event_id(&mut self) -> usize {
        let cnt = self.count;
        self.count += 1;
        cnt
    }

    /// Instantiate the network state.  Bind to the given address, and accept at most `capacity`
    /// sockets.
    pub fn bind(addr: &SocketAddr, capacity: usize) -> Result<NetworkState, Error> {
        let server = NetworkState::bind_address(addr)?;
        let poll = mio::Poll::new()
            .map_err(|e| {
                error!("Failed to initialize poller: {:?}", e);
                Error::BindError
            })?;

        let events = mio::Events::with_capacity(capacity);

        poll.register(&server, SERVER, mio::Ready::all(), mio::PollOpt::edge())
            .map_err(|e| {
                error!("Failed to register server socket: {:?}", &e);
                Error::BindError
            })?;

        Ok(NetworkState {
            poll: poll,
            server: server,
            events: events,
            count: 1
        })
    }

    /// Register a socket for read/write notifications with this poller.  Poll events will be keyed
    /// to the given event ID.  Use edge triggers instead of level triggers, since that's the only
    /// portable way to get events in mio.
    pub fn register(&mut self, event_id: usize, sock: &mio_net::TcpStream) -> Result<(), Error> {
        debug!("Register socket {:?} as event {}", sock, event_id);
        self.poll.register(sock, mio::Token(event_id), Ready::all(), PollOpt::edge())
            .map_err(|e| {
                error!("Failed to register socket: {:?}", &e);
                Error::RegisterError
            })
    }

    /// Deregister a socket event.  Future events on the socket will be ignored, and the socket
    /// will be explicitly shutdown.
    pub fn deregister(&mut self, sock: &mio_net::TcpStream) -> Result<(), Error> {
        self.poll.deregister(sock)
            .map_err(|e| {
                error!("Failed to deregister socket: {:?}", &e);
                Error::RegisterError
            })?;

        sock.shutdown(Shutdown::Both)
            .map_err(|_e| Error::SocketError)?;

        debug!("Socket disconnected: {:?}", sock);
        Ok(())
    }

    /// Poll socket states.  Create a new NetworkPollState struct to store newly-accepted sockets,
    /// and list ready sockets.
    pub fn poll(&mut self, timeout: u64) -> Result<NetworkPollState, Error> {
        self.poll.poll(&mut self.events, Some(Duration::from_millis(timeout)))
            .map_err(|e| {
                error!("Failed to poll: {:?}", &e);
                Error::PollError
            })?;
       
        let mut poll_state = NetworkPollState::new();
        for event in &self.events {
            match event.token() {
                SERVER => {
                    // new inbound connection(s)
                    loop {
                        let (client_sock, _client_addr) = match self.server.accept() {
                            Ok((client_sock, client_addr)) => (client_sock, client_addr),
                            Err(e) => {
                                match e.kind() {
                                    io::ErrorKind::WouldBlock => {
                                        break;
                                    },
                                    _ => {
                                        return Err(Error::AcceptError);
                                    }
                                }
                            }
                        };

                        debug!("New socket accepted from {:?} (event {}): {:?}", &_client_addr, self.count, &client_sock);
                        poll_state.new.insert(self.count, client_sock);
                        self.count += 1;
                    }
                },
                mio::Token(event_id) => {
                    // I/O available 
                    poll_state.ready.push(event_id);
                }
            }
        }

        Ok(poll_state)
    }
}

impl ForwardState {
    pub fn new() -> ForwardState {
        ForwardState {
            preamble_buf: vec![],
            write_buf: vec![],
            write_buf_ptr: 0,
            preamble: None,
            num_read: 0,
            filtered: false,
            last_io_at: get_epoch_time_ms(),
            pair_event_id: 0
        }
    }

    /// Reset the forwarding state.  Called each time we process a whole message.
    pub fn reset(&mut self) -> () {
        self.preamble_buf.clear();
        self.preamble = None;
        self.num_read = 0;
        self.filtered = false;
        self.write_buf.clear();
        self.write_buf_ptr = 0;
    }

    /// Given the expected magic and byte buffer encoding a Bitcoin message header, parse it out.
    /// Returns None if the bytes do not encode a well-formed header.
    /// The checksum field will be ignored.
    fn parse_preamble(magic: u32, buf: &[u8]) -> Option<BitcoinPreamble> {
        if buf.len() < mem::size_of::<BitcoinPreamble>() {
            debug!("Invalid preamble: buf is {}, expected {}", buf.len(), mem::size_of::<BitcoinPreamble>());
            return None;
        }

        let mut msg_magic_bytes = [0u8; 4];
        msg_magic_bytes.copy_from_slice(&buf[0..4]);

        let msg_magic = u32::from_le_bytes(msg_magic_bytes);
        if msg_magic != magic {
            debug!("Invalid magic: expected {:x}, got {:x}", magic, msg_magic);
            return None;
        }

        let mut command_bytes = [0u8; 12];
        command_bytes.copy_from_slice(&buf[4..16]);
        let command = BitcoinCommand(command_bytes);

        let mut msg_len_bytes = [0u8; 4];
        let mut msg_checksum_bytes = [0u8; 4];

        msg_len_bytes.copy_from_slice(&buf[16..20]);
        msg_checksum_bytes.copy_from_slice(&buf[20..24]);

        let msglen = u32::from_le_bytes(msg_len_bytes);
        let checksum = u32::from_le_bytes(msg_checksum_bytes);

        debug!("Parsed preamble: '{}'. Payload length: {}", command.to_string(), msglen);

        Some(BitcoinPreamble {
            magic: magic,
            command: command,
            length: msglen,
            checksum: checksum
        })
    }

    /// How many bytes are left to write out?
    pub fn buf_len(&self) -> usize {
        self.write_buf[self.write_buf_ptr..].len()
    }

    /// Attempt to drain the write buffer to the given writeable upstream.  Returns Ok(true) if we
    /// drain the buffer, Ok(false) if there are bytes remaining, or Err(...) on network error.
    pub fn try_flush<W: Write>(&mut self, upstream: &mut W) -> Result<bool, Error> {
        if self.filtered {
            debug!("Dropping {} bytes", self.write_buf.len() - self.write_buf_ptr);
            self.write_buf.clear();
            self.write_buf_ptr = 0;
            return Ok(true);
        }

        if self.write_buf_ptr >= self.write_buf.len() {
            debug!("Write buffer is empty");
            return Ok(true);
        }

        let to_flush = &self.write_buf[self.write_buf_ptr..];
        let nw = upstream.write(to_flush)
            .map_err(|e| {
                match e.kind() {
                    io::ErrorKind::WouldBlock => Error::Blocked,
                    _ => Error::WriteError(e)
                }
            })?;

        if nw > 0 {
            self.last_io_at = get_epoch_time_ms();
        }

        debug!("Forwarded {} bytes upstream out of {}", nw, self.write_buf[self.write_buf_ptr..].len());
        self.write_buf_ptr += nw;

        if self.write_buf_ptr >= self.write_buf.len() {
            self.write_buf.clear();
            self.write_buf_ptr = 0;
            return Ok(true);
        }
        else {
            return Ok(false);
        }
    }
    
    /// Read a Bitcoin message header from the readable fd, with the given magic bytes.
    /// Reads up to, but not over, the number of bytes in a Bitcoin message header.
    /// If we get enough bytes to form a Bitcoin message header, and if we successfully parse it
    /// into a BitcoinPreamble, then return Ok(true) so we can proceed to read the payload.
    /// Return Ok(false) if we don't have enough bytes to form a BitcoinPreamble.
    /// Return Err(...) on network error, or if we can't parse a BitcoinPreamble (indicates to the
    /// network poll loop to disconnect this socket).
    pub fn read_preamble<R: Read>(&mut self, magic: u32, fd: &mut R) -> Result<bool, Error> {
        assert!(self.preamble.is_none());
        if self.preamble_buf.len() >= mem::size_of::<BitcoinPreamble>() {
            // should be unreachable
            return Err(Error::ConnectionBroken);
        }

        let to_read = mem::size_of::<BitcoinPreamble>() - self.preamble_buf.len();
        let mut buf = vec![0u8; to_read];

        debug!("Read up to {} preamble bytes", to_read);
        let nr = match fd.read(&mut buf) {
            Ok(0) => {
                return Err(Error::ConnectionBroken);
            },
            Ok(nr) => {
                debug!("Read {} preamble bytes out of {}", nr, to_read);
                self.last_io_at = get_epoch_time_ms();
                nr
            },
            Err(e) => match e.kind() {
                io::ErrorKind::WouldBlock => {
                    return Err(Error::Blocked);
                }
                _ => {
                    return Err(Error::ReadError(e));
                }
            }
        };

        self.preamble_buf.extend_from_slice(&buf[0..nr]);
        if self.preamble_buf.len() == mem::size_of::<BitcoinPreamble>() {
            let preamble = ForwardState::parse_preamble(magic, &self.preamble_buf);
            if preamble.is_none() {
                return Err(Error::ConnectionBroken);
            }

            // forward premable upstream
            self.write_buf.extend_from_slice(&self.preamble_buf);

            self.preamble = preamble;
            self.preamble_buf.clear();
            Ok(true)
        }
        else {
            Ok(false)
        }
    }

    /// Consume a Bitcoin message payload from the given readable input.  
    /// Buffers up the bytes into the write buffer, so a subsequent call to .try_flush() can be
    /// used to send them to the paired socket.
    /// Returns the number of bytes read on success.  Returns Err(...) on network error, in which case, the underlying
    /// socket will be closed.
    pub fn read_payload<R: Read>(&mut self, input: &mut R) -> Result<usize, Error> {
        assert!(self.preamble.is_some());

        let preamble = self.preamble.as_ref().unwrap();
        assert!(self.num_read <= (preamble.length as u64));
        
        let to_read = (preamble.length as u64) - self.num_read;
        let max_read = 
            if to_read > (BUF_SIZE + mem::size_of::<BitcoinPreamble>() - self.buf_len()) as u64 {
                BUF_SIZE + mem::size_of::<BitcoinPreamble>() - self.buf_len()
            }
            else {
                to_read as usize
            };

        let mut buf = [0u8; BUF_SIZE + mem::size_of::<BitcoinPreamble>()];
        let nr = match input.read(&mut buf[0..max_read]) {
            Ok(0) => {
                if max_read > 0 {
                    debug!("EOF encountered");
                    return Err(Error::ConnectionBroken);
                }
                else {
                    // zero-length message was expected.
                    // Can happen with `verack`, for example.
                    debug!("Read zero-length payload (so-far: {}, total: {})", self.num_read, preamble.length);
                    self.last_io_at = get_epoch_time_ms();
                    0
                }
            }
            Ok(nr) => {
                debug!("Read {} payload bytes out of {} (so-far: {}, total: {})", nr, max_read, self.num_read, preamble.length);
                self.last_io_at = get_epoch_time_ms();
                nr
            },
            Err(e) => match e.kind() {
                io::ErrorKind::WouldBlock => {
                    return Err(Error::Blocked);
                }
                _ => {
                    return Err(Error::ReadError(e));
                }
            }
        };

        self.write_buf.extend_from_slice(&buf[0..nr]);
        self.num_read += nr as u64;
        Ok(nr)
    }

    /// Reset this forwarding state if we finished consuming a message.
    pub fn try_reset(&mut self) -> () {
        match self.preamble {
            Some(ref preamble) => {
                if self.num_read >= preamble.length as u64 && self.buf_len() == 0 {
                    debug!("Reset!");
                    self.reset();
                }
            },
            None => {}
        }
    }

    /// Has the socket gone more than IDLE_TIMEOUT milliseconds without reading or writing
    /// anything?  Used to disconnect idle sockets.
    pub fn is_idle_timeout(&self) -> bool {
        if self.last_io_at + IDLE_TIMEOUT < get_epoch_time_ms() {
            return true;
        }
        else {
            return false;
        }
    }
}

impl BitcoinProxy {
    /// Create a new BitcoinProxy, bound to the given port, and forwarding packets to the given
    /// backend socket address.  Binds a server socket on success.
    pub fn new(magic: u32, port: u16, backend_addr: SocketAddr) -> Result<BitcoinProxy, Error> {
        let net = NetworkState::bind(&format!("0.0.0.0:{}", port).parse::<SocketAddr>().unwrap(), 1000)?;
        Ok(BitcoinProxy {
            network: net,
            magic: magic,
            backend_addr: backend_addr,
            blacklist: vec![],
            whitelist: vec![],
            messages_downstream: HashMap::new(),
            downstream: HashMap::new(),
            upstream: HashMap::new(),
            messages_upstream: HashMap::new(),
        })
    }

    /// Add a command to the firewall whitelist.
    /// Duplicates are not checked.
    pub fn add_whitelist(&mut self, cmd: BitcoinCommand) -> () {
        self.whitelist.push(cmd);
    }

    /// Adds a command to the firewall blacklist.
    /// Duplicates are not checked.
    pub fn add_blacklist(&mut self, cmd: BitcoinCommand) -> () {
        self.blacklist.push(cmd);
    }

    /// Given a preamble, a whitelist, and a blacklist, determine whether or not to filter the
    /// Bitcoin message (returns true to filter; false not to).
    /// While this is not checked, either whitelist, blacklist, or both must have zero entries for
    /// this to work as expected.  If both lists are non-empty, the whitelist is preferred.
    pub fn filter(preamble: &BitcoinPreamble, whitelist: &Vec<BitcoinCommand>, blacklist: &Vec<BitcoinCommand>) -> bool {
        if whitelist.len() > 0 {
            for cmd in whitelist.iter() {
                if *cmd == preamble.command {
                    return false;
                }
            }
            debug!("Filtered non-whitelisted command '{:?}'", preamble.command.to_string());
            return true;
        }

        if blacklist.len() > 0 {
            for cmd in blacklist.iter() {
                if *cmd == preamble.command {
                    debug!("Filtered blacklisted command '{:?}'", preamble.command.to_string());
                    return true;
                }
            }

            return false;
        }

        return false;
    }

    /// Register a downstream client connection.
    /// This is called to handle a recently-accepted socket.
    /// Opens an associated socket to the upstream bitcoind, so that messages from this socket are
    /// sent out to bitcoind if they are not filtered.
    /// Registers both sockets with the network poller, and creates forwarding state for both.
    /// Each socket's forwarding state contains is the other's paired event ID, which will be
    /// necessary to find the right socket to ferry bytes to when processing socket I/O.
    fn register_socket(&mut self, event_id: usize, socket: mio_net::TcpStream) -> Result<(), Error> {
        self.network.register(event_id, &socket)?;
        
        let upstream_sync = net::TcpStream::connect_timeout(&self.backend_addr, Duration::new(1, 0)).map_err(|_e| Error::ConnectionError)?;
        let upstream = mio_net::TcpStream::from_stream(upstream_sync).map_err(|_e| Error::ConnectionError)?;
        upstream.set_nodelay(true).map_err(|_e| Error::ConnectionError)?;
        
        let upstream_event_id = self.network.next_event_id();
        self.network.register(upstream_event_id, &upstream)?;

        let mut forward = ForwardState::new();
        let mut backward = ForwardState::new();

        forward.pair_event_id = upstream_event_id;
        backward.pair_event_id = event_id;
        
        self.messages_downstream.insert(event_id, forward);
        self.downstream.insert(event_id, socket);

        self.messages_upstream.insert(upstream_event_id, backward);
        self.upstream.insert(upstream_event_id, upstream);

        debug!("Registered event pair {},{}", event_id, upstream_event_id);
        Ok(())
    }

    /// Disconnect an event's socket from the poller, and free up its forwarding state.
    /// Does not touch the event's paired socket.
    fn deregister_socket(&mut self, event_id: usize) -> () {
        if self.messages_downstream.get(&event_id).is_some() {
            match self.downstream.get(&event_id) {
                Some(ref sock) => {
                    let _ = self.network.deregister(sock);
                },
                None => {}
            }

            self.messages_downstream.remove(&event_id);
        };
        
        if self.messages_upstream.get(&event_id).is_some() {
            match self.upstream.get(&event_id) {
                Some(ref sock) => {
                    let _ = self.network.deregister(sock);
                },
                None => {}
            }

            self.messages_upstream.remove(&event_id);
        };

        debug!("De-egistered event {}", event_id);
    }

    /// Register all new downstream clients, and open an upstream socket to bitcoind for each one.
    pub fn process_new_sockets(&mut self, pollstate: &mut NetworkPollState) -> () {
        for (event_id, sock) in pollstate.new.drain() {
            let _ = self.register_socket(event_id, sock);
        }
    }

    /// Handle socket I/O from one socket (inbound) to another (outbound), filtering messages as
    /// needed.  Handles as many bytes as possible, bufferring them up into the inbound socket's
    /// forwarding state as needed.  Runs until encountering EWOULDBLOCK.
    pub fn process_socket_io<R: Read, W: Write>(magic: u32, whitelist: &Vec<BitcoinCommand>, blacklist: &Vec<BitcoinCommand>, inbound: &mut R, forward: &mut ForwardState, outbound: &mut W) -> bool {
        let blocked;
        loop {
            if forward.preamble.is_none() {
                match forward.read_preamble(magic, inbound) {
                    Ok(true) => {},
                    Ok(false) => {
                        return true;
                    },
                    Err(Error::Blocked) => {
                        debug!("read_preamble blocked");
                        blocked = true;
                        break;
                    },
                    Err(_e) => {
                        return false;
                    }
                }
            }

            if forward.preamble.is_some() {
                if !forward.filtered && BitcoinProxy::filter(&forward.preamble.as_ref().unwrap(), whitelist, blacklist) {
                    forward.filtered = true;
                }

                match forward.read_payload(inbound) {
                    Ok(_) => {},
                    Err(Error::Blocked) => {
                        debug!("read_payload blocked");
                        blocked = true;
                        break;
                    }
                    Err(_e) => {
                        return false;
                    }
                }
            }

            match forward.try_flush(outbound) {
                Ok(true) => {},
                Ok(false) => {
                    return true;
                },
                Err(Error::Blocked) => {
                    debug!("try_flush blocked");
                    blocked = true;
                    break;
                }
                Err(_e) => {
                    return false;
                }
            }

            forward.try_reset();
        }
        if blocked {
            forward.try_reset();

            // try to make space in the write buffer
            match forward.try_flush(outbound) {
                Ok(_) => {
                    return true;
                },
                Err(Error::Blocked) => {
                    debug!("try_flush blocked");
                    return true;
                },
                Err(_e) => {
                    return false;
                }
            }
        }

        return true;
    }

    /// Handle I/O on all ready sockets.  Read bytes from downstream clients and write them to
    /// their upstream sockets, and read bytes from upstream sockets and write them to downstream
    /// clients.  The filter will only be applied to messages coming from downstream clients.
    pub fn process_ready_sockets(&mut self, pollstate: &mut NetworkPollState) -> () {
        let mut broken = vec![];
        for event_id in pollstate.ready.iter() {
            if self.downstream.get(event_id).is_some() {
                debug!("Event {} (a downstream socket) is ready for I/O", event_id);
                let pair_event_id = match self.messages_downstream.get(event_id) {
                    Some(ref fwd) => fwd.pair_event_id,
                    None => {
                        continue;
                    }
                };

                match (self.messages_downstream.get_mut(event_id), self.downstream.get_mut(event_id), self.upstream.get_mut(&pair_event_id)) {
                    (Some(ref mut forward), Some(ref mut downstream), Some(ref mut upstream)) => {
                        debug!("Process I/O from downstream event {} ({:?})", *event_id, downstream);
                        let alive = BitcoinProxy::process_socket_io(self.magic, &self.whitelist, &self.blacklist, downstream, forward, upstream);
                        if !alive {
                            broken.push(*event_id);
                        }
                    }
                    (_, _, _) => {
                        continue;
                    }
                }
            }
            else if self.upstream.get(event_id).is_some() {
                debug!("Event {} (an upstream socket) is ready for I/O", event_id);
                let pair_event_id = match self.messages_upstream.get(event_id) {
                    Some(ref fwd) => fwd.pair_event_id,
                    None => {
                        continue;
                    }
                };

                match (self.messages_upstream.get_mut(event_id), self.downstream.get_mut(&pair_event_id), self.upstream.get_mut(event_id)) {
                    (Some(ref mut forward), Some(ref mut downstream), Some(ref mut upstream)) => {
                        debug!("Process I/O from upstream event {} ({:?})", *event_id, downstream);
                        let alive = BitcoinProxy::process_socket_io(self.magic, &vec![], &vec![], upstream, forward, downstream);
                        if !alive {
                            broken.push(*event_id);
                        }
                    }
                    (_, _, _) => {
                        continue;
                    }
                }
            }
        }

        for event_id in broken {
            self.deregister_socket(event_id);
        }
    }

    /// Find idle sockets and disconnect them.
    pub fn process_dead_sockets(&mut self) -> () {
        let mut timedout : Vec<usize> = vec![];
        for event_id in self.messages_downstream.keys() {
            match self.messages_downstream.get(event_id) {
                Some(ref fwd) => {
                    if fwd.is_idle_timeout() {
                        debug!("Event {} (downstream) has idled too long", event_id);
                        timedout.push(*event_id);
                    }
                },
                None => {}
            }
        }
        for event_id in self.messages_upstream.keys() {
            match self.messages_upstream.get(event_id) {
                Some(ref fwd) => {
                    if fwd.is_idle_timeout() {
                        debug!("Event {} (upstream) has idled too long", event_id);
                        timedout.push(*event_id);
                    }
                },
                None => {}
            }
        }
        for event_id in timedout {
            self.deregister_socket(event_id);
        }
    }

    /// Do one poll pass.
    pub fn poll(&mut self, timeout: u64) -> Result<(), Error> {
        let mut poll_state = self.network.poll(timeout)?;
        self.process_new_sockets(&mut poll_state);
        self.process_ready_sockets(&mut poll_state);
        self.process_dead_sockets();
        Ok(())
    }
}

/// Given a mutable list of arguments, and a switch, find the first instance of the switch and its
/// associated argument, remove them from argv, and return the argument itself.  Returns None if
/// not found.
fn find_arg_value(arg: &str, argv: &mut Vec<String>) -> Option<String> {
    debug!("Find '{}'. Argv is {:?}", arg, argv);
    let mut idx : i64 = -1;
    for i in 0..argv.len() - 1 {
        if argv[i] == arg {
            idx = (i+1) as i64;
            break;
        }
    }
    if idx > 0 {
        let value = argv[idx as usize].to_string();
        debug!("Argument {} is '{}'", idx, &value);

        argv.remove((idx - 1) as usize);
        argv.remove((idx - 1) as usize);
        Some(value)
    }
    else {
        None
    }
}

/// Print usage and exit
fn usage(prog_name: String) -> () {
    eprintln!("Usage: {} [-w|-b MSG1,MSG2,MSG3,...] [-v VERBOSITY] [-n NETWORK-NAME] port upstream_addr", prog_name);
    process::exit(1);
}

// Entry point
fn main() {
    set_loglevel(LOG_DEBUG).unwrap();

    let mut argv : Vec<String> = env::args().map(|s| s.to_string()).collect();
    let prog_name = argv[0].clone();

    if argv.len() < 2 {
        usage(prog_name.clone());
    }

    // find -v
    let verbosity_arg = find_arg_value("-v", &mut argv).unwrap_or(format!("{}", LOG_ERROR));
    let verbosity = verbosity_arg.parse::<u8>().map_err(|_e| usage(prog_name.clone())).unwrap();

    let _ = set_loglevel(verbosity);

    // find -w
    let whitelist_filter_csv = find_arg_value("-w", &mut argv).unwrap_or("".to_string());
    let mut whitelist_filter : Vec<String> = whitelist_filter_csv.split(",").filter(|s| s.len() > 0 ).map(|s| s.to_string()).collect();
    
    // find -b
    let blacklist_filter_csv = find_arg_value("-b", &mut argv).unwrap_or("".to_string());
    let mut blacklist_filter : Vec<String> = blacklist_filter_csv.split(",").filter(|s| s.len() > 0).map(|s| s.to_string()).collect();

    if whitelist_filter.len() != 0 && blacklist_filter.len() != 0 {
        debug!("Whitelist filter: {:?}", &whitelist_filter);
        debug!("Blacklist filter: {:?}", &blacklist_filter);
        usage(prog_name.clone());
    }

    // find -n
    let network_name = find_arg_value("-n", &mut argv).unwrap_or("mainnet".to_string());
    let magic = match network_name.as_str() {
        "mainnet" => 0xD9B4BEF9,
        "testnet" => 0x0709110B,
        "regtest" => 0xDAB5BFFA,
        _ => {
            usage(prog_name.clone());
            unreachable!();
        }
    };

    debug!("Final Argv is {:?}", &argv);

    // find port 
    if argv.len() != 3 {
        usage(prog_name.clone());
    }
    let port = argv[1].parse::<u16>().map_err(|_e| usage(prog_name.clone())).unwrap();
    let backend = argv[2].parse::<SocketAddr>().map_err(|_e| usage(prog_name.clone())).unwrap();

    debug!("Bind on port {}", port);
    let mut proxy = BitcoinProxy::new(magic, port, backend).unwrap();
    debug!("Bound to 0.0.0.0:{}", port);

    for message_name in whitelist_filter.drain(..) {
        if message_name.len() > 12 {
            eprintln!("Invalid Bitcoin message type: '{}'", message_name);
            usage(prog_name.clone());
        }
        info!("Whitelist '{}'", message_name);
        let bitcoin_cmd = BitcoinCommand::from_str(&message_name).unwrap();
        proxy.add_whitelist(bitcoin_cmd);
    }
    
    for message_name in blacklist_filter.drain(..) {
        if message_name.len() > 12 {
            eprintln!("Invalid Bitcoin message type: '{}'", message_name);
            usage(prog_name.clone());
        }
        info!("Blacklist '{}'", message_name);
        let bitcoin_cmd = BitcoinCommand::from_str(&message_name).unwrap();
        proxy.add_blacklist(bitcoin_cmd);
    }

    loop {
        match proxy.poll(100) {
            Ok(_) => {},
            Err(_e) => {
                break;
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::io::Cursor;
    use std::io::{Read, Write};

    /// A Cursor container that will return EWOULDBLOCK after reading a given number of bytes.
    /// Used to test EWOULDBLOCK handling without actually setting up sockets.
    pub struct BlockingCursor<T> {
        inner: Cursor<T>,
        block_window: usize,
        block: bool,
    }

    impl<T: AsRef<[u8]>> BlockingCursor<T> {
        pub fn new(cursor: Cursor<T>, block_window: usize) -> BlockingCursor<T> {
            BlockingCursor {
                inner: cursor,
                block_window: block_window,
                block: false,
            }
        }

        pub fn destruct(self) -> Cursor<T> {
            self.inner
        }
    }

    impl<T: AsRef<[u8]>> Read for BlockingCursor<T> {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.block {
                self.block = false;
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            self.block = true;
            if bytes.len() > self.block_window {
                self.inner.read(&mut bytes[0..self.block_window])
            }
            else {
                self.inner.read(bytes)
            }
        }
    }

    impl Write for BlockingCursor<&mut Vec<u8>> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.block {
                self.block = false;
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            self.block = true;
            if bytes.len() > self.block_window {
                self.inner.write(&bytes[0..self.block_window])
            }
            else {
                self.inner.write(bytes)
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    #[test]
    fn test_parse_preamble() {
        let message_bytes = vec![
            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (verack)
            0x76, 0x65, 0x72, 0x61, 0x63, 0x6b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (1234)
            0x0d2, 0x04, 0x00, 0x00,
            // checksum (0x12345678)
            0x78, 0x56, 0x34, 0x12
        ];
        let preamble = ForwardState::parse_preamble(0xf9beb4d9, &message_bytes).unwrap();
        assert_eq!(preamble, BitcoinPreamble::new(0xf9beb4d9, "verack", 1234, 0x12345678));

        // bad magic
        assert!(ForwardState::parse_preamble(0xD9B4BEF8, &message_bytes).is_none());
    }

    #[test]
    fn test_read_message() {
        let message_bytes = vec![
            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (verack)
            0x76, 0x65, 0x72, 0x61, 0x63, 0x6b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (16)
            0x10, 0x00, 0x00, 0x00,
            // checksum (0x78563412)
            0x12, 0x34, 0x56, 0x78,
            // payload
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f
        ];

        let mut f = ForwardState::new();
        let mut fd = io::Cursor::new(&message_bytes);
        let mut output = vec![];

        let res = f.read_preamble(0xf9beb4d9, &mut fd).unwrap();
        assert!(res, "Failed to read preamble");

        assert!(f.preamble.is_some());
        assert_eq!(f.preamble, Some(BitcoinPreamble::new(0xf9beb4d9, "verack", 16, 0x78563412)));
        assert_eq!(f.num_read, 0);

        let res = f.try_flush(&mut output).unwrap();
        assert!(res);

        let res = f.read_payload(&mut fd).unwrap();
        assert_eq!(res, 16);
        
        let res = f.try_flush(&mut output).unwrap();
        assert!(res);

        f.try_reset();
        assert_eq!(output, message_bytes);
        assert!(f.preamble.is_none());
        assert_eq!(f.preamble_buf.len(), 0);
        assert_eq!(f.num_read, 0);
    }

    #[test]
    fn test_socket_io() {
        let stream_bytes = vec![
            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (version)
            0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (8)
            0x08, 0x00, 0x00, 0x00,
            // checksum (0x12345678)
            0x78, 0x56, 0x34, 0x12,
            // payload
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,

            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (verack)
            0x76, 0x65, 0x72, 0x61, 0x63, 0x6b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (16)
            0x10, 0x00, 0x00, 0x00,
            // checksum (0x22334455)
            0x55, 0x44, 0x33, 0x22,
            // payload
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,

            // magic 
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (ping)
            0x70, 0x69, 0x6e, 0x67, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (4)
            0x04, 0x00, 0x00, 0x00,
            // checksum (0x33445566)
            0x33, 0x44, 0x55, 0x66,
            // payload
            0x20, 0x21, 0x22, 0x23
        ];

        let mut f = ForwardState::new();
        let inner_fd = io::Cursor::new(&stream_bytes);
        let mut inner_output = vec![];

        let mut fd = BlockingCursor::new(inner_fd, 10);
        let mut output = BlockingCursor::new(io::Cursor::new(&mut inner_output), 10);

        for _ in 0..stream_bytes.len() {
            BitcoinProxy::process_socket_io(0xf9beb4d9, &vec![], &vec![], &mut fd, &mut f, &mut output);
        }

        let inner_fd = fd.destruct(); 
        let inner_output = output.destruct();

        assert_eq!(inner_fd.position(), stream_bytes.len() as u64);
        assert_eq!(**inner_output.get_ref(), stream_bytes);
    }
    
    #[test]
    fn test_socket_io_whitelist() {
        let stream_bytes = vec![
            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (version)
            0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (8)
            0x08, 0x00, 0x00, 0x00,
            // checksum (0x12345678)
            0x78, 0x56, 0x34, 0x12,
            // payload
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,

            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (verack)
            0x76, 0x65, 0x72, 0x61, 0x63, 0x6b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (16)
            0x10, 0x00, 0x00, 0x00,
            // checksum (0x22334455)
            0x55, 0x44, 0x33, 0x22,
            // payload
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,

            // magic 
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (ping)
            0x70, 0x69, 0x6e, 0x67, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (4)
            0x04, 0x00, 0x00, 0x00,
            // checksum (0x33445566)
            0x66, 0x55, 0x44, 0x33,
            // payload
            0x20, 0x21, 0x22, 0x23
        ];
        
        let filtered_stream_bytes = vec![
            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (version)
            0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (8)
            0x08, 0x00, 0x00, 0x00,
            // checksum (0x12345678)
            0x78, 0x56, 0x34, 0x12,
            // payload
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,

            // magic 
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (ping)
            0x70, 0x69, 0x6e, 0x67, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (4)
            0x04, 0x00, 0x00, 0x00,
            // checksum (0x33445566)
            0x66, 0x55, 0x44, 0x33,
            // payload
            0x20, 0x21, 0x22, 0x23
        ];
        
        let mut f = ForwardState::new();
        let inner_fd = io::Cursor::new(&stream_bytes);
        let mut inner_output = vec![];

        let mut fd = BlockingCursor::new(inner_fd, 10);
        let mut output = BlockingCursor::new(io::Cursor::new(&mut inner_output), 10);

        for _ in 0..stream_bytes.len() {
            BitcoinProxy::process_socket_io(0xf9beb4d9, &vec![BitcoinCommand::from_str("version").unwrap(), BitcoinCommand::from_str("ping").unwrap()], &vec![], &mut fd, &mut f, &mut output);
        }

        let inner_fd = fd.destruct(); 
        let inner_output = output.destruct();

        assert_eq!(inner_fd.position(), stream_bytes.len() as u64);
        assert_eq!(**inner_output.get_ref(), filtered_stream_bytes);
    }
    
    #[test]
    fn test_socket_io_blacklist() {
        let stream_bytes = vec![
            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (version)
            0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (8)
            0x08, 0x00, 0x00, 0x00,
            // checksum (0x12345678)
            0x78, 0x56, 0x34, 0x12,
            // payload
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,

            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (verack)
            0x76, 0x65, 0x72, 0x61, 0x63, 0x6b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (16)
            0x10, 0x00, 0x00, 0x00,
            // checksum (0x22334455)
            0x55, 0x44, 0x33, 0x22,
            // payload
            0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,

            // magic 
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (ping)
            0x70, 0x69, 0x6e, 0x67, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (4)
            0x04, 0x00, 0x00, 0x00,
            // checksum (0x33445566)
            0x66, 0x55, 0x44, 0x33,
            // payload
            0x20, 0x21, 0x22, 0x23
        ];
        
        let filtered_stream_bytes = vec![
            // magic
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (version)
            0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (8)
            0x08, 0x00, 0x00, 0x00,
            // checksum (0x12345678)
            0x78, 0x56, 0x34, 0x12,
            // payload
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,

            // magic 
            0xd9, 0xb4, 0xbe, 0xf9,
            // command (ping)
            0x70, 0x69, 0x6e, 0x67, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // length (4)
            0x04, 0x00, 0x00, 0x00,
            // checksum (0x33445566)
            0x66, 0x55, 0x44, 0x33,
            // payload
            0x20, 0x21, 0x22, 0x23
        ];

        let mut f = ForwardState::new();
        let inner_fd = io::Cursor::new(&stream_bytes);
        let mut inner_output = vec![];

        let mut fd = BlockingCursor::new(inner_fd, 10);
        let mut output = BlockingCursor::new(io::Cursor::new(&mut inner_output), 10);

        for _ in 0..stream_bytes.len() {
            BitcoinProxy::process_socket_io(0xf9beb4d9, &vec![], &vec![BitcoinCommand::from_str("verack").unwrap()], &mut fd, &mut f, &mut output);
        }

        let inner_fd = fd.destruct(); 
        let inner_output = output.destruct();

        assert_eq!(inner_fd.position(), stream_bytes.len() as u64);
        assert_eq!(**inner_output.get_ref(), filtered_stream_bytes);
    }
}
