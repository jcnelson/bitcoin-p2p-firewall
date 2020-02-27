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

#![allow(unused_imports)]
#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(unused_macros)]

extern crate mio;

use std::io::prelude::*;
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

pub const LOG_DEBUG : u8 = 4;
pub const LOG_INFO : u8 = 3;
pub const LOG_WARN : u8 = 2;
pub const LOG_ERROR : u8 = 1;

// per-thread log level and log format
thread_local!(static loglevel: RefCell<u8> = RefCell::new(LOG_DEBUG));

pub fn set_loglevel(ll: u8) -> Result<(), String> {
    loglevel.with(move |level| {
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
    loglevel.with(|lvl| {
        res = *lvl.borrow();
    });
    res
}

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

pub struct NetworkState {
    addr: SocketAddr,
    poll: mio::Poll,
    server: mio_net::TcpListener,
    events: mio::Events,
    count: usize,
}

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

pub struct ForwardState {
    preamble_buf: Vec<u8>,
    preamble: Option<BitcoinPreamble>,
    num_read: u64
}

pub struct BitcoinProxy {
    magic: u32,
    network: NetworkState,
    backend_addr: SocketAddr,
    blacklist: Vec<BitcoinCommand>,
    whitelist: Vec<BitcoinCommand>,

    messages: HashMap<usize, ForwardState>,
    downstream: HashMap<usize, mio_net::TcpStream>,
    upstream: HashMap<usize, mio_net::TcpStream>
}

pub struct BitcoinPreamble {
    magic: u32,
    command: BitcoinCommand,
    length: u32,
    checksum: u32
}

impl NetworkState {
    fn bind_address(addr: &SocketAddr) -> Result<mio_net::TcpListener, Error> {
        mio_net::TcpListener::bind(addr)
            .map_err(|e| {
                error!("Failed to bind to {:?}: {:?}", addr, e);
                Error::BindError
            })
    }

    pub fn bind(addr: &SocketAddr, capacity: usize) -> Result<NetworkState, Error> {
        let server = NetworkState::bind_address(addr)?;
        let poll = mio::Poll::new()
            .map_err(|e| {
                error!("Failed to initialize poller: {:?}", e);
                Error::BindError
            })?;

        let events = mio::Events::with_capacity(capacity);

        poll.register(&server, SERVER, mio::Ready::readable(), mio::PollOpt::edge())
            .map_err(|e| {
                error!("Failed to register server socket: {:?}", &e);
                Error::BindError
            })?;

        Ok(NetworkState {
            addr: addr.clone(),
            poll: poll,
            server: server,
            events: events,
            count: 1
        })
    }

    /// next event ID
    pub fn next_event_id(&mut self) -> usize {
        let ret = self.count;
        self.count += 1;
        ret
    }

    /// Register a socket for read/write notifications with this poller
    pub fn register(&mut self, event_id: usize, sock: &mio_net::TcpStream) -> Result<(), Error> {
        debug!("Register socket {:?} as event {}", sock, event_id);
        self.poll.register(sock, mio::Token(event_id), Ready::all(), PollOpt::edge())
            .map_err(|e| {
                error!("Failed to register socket: {:?}", &e);
                Error::RegisterError
            })
    }

    /// Deregister a socket event
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

    /// Poll socket states
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
            preamble: None,
            num_read: 0
        }
    }

    pub fn reset(&mut self) -> () {
        self.preamble_buf.clear();
        self.preamble = None;
        self.num_read = 0;
    }

    fn parse_preamble(magic: u32, buf: &[u8]) -> Option<BitcoinPreamble> {
        if buf.len() != mem::size_of::<BitcoinPreamble>() {
            return None;
        }

        let mut msg_magic_bytes = [0u8; 4];
        msg_magic_bytes.copy_from_slice(&buf[0..4]);

        let msg_magic = u32::from_be_bytes(msg_magic_bytes);
        if msg_magic != magic {
            return None;
        }

        let mut command_bytes = [0u8; 12];
        command_bytes.copy_from_slice(&buf[4..16]);
        let command = BitcoinCommand(command_bytes);

        let mut msg_len_bytes = [0u8; 4];
        let mut msg_checksum_bytes = [0u8; 4];

        msg_len_bytes.copy_from_slice(&buf[16..20]);
        msg_checksum_bytes.copy_from_slice(&buf[20..24]);

        let msglen = u32::from_be_bytes(msg_len_bytes);
        let checksum = u32::from_be_bytes(msg_checksum_bytes);

        Some(BitcoinPreamble {
            magic: magic,
            command: command,
            length: msglen,
            checksum: checksum
        })
    }
    
    pub fn read_preamble<R: Read>(&mut self, magic: u32, fd: &mut R) -> Result<bool, Error> {
        assert!(self.preamble.is_none());
        if self.preamble_buf.len() >= mem::size_of::<BitcoinPreamble>() {
            return Err(Error::ConnectionBroken);
        }

        let to_read = mem::size_of::<BitcoinPreamble>() - self.preamble_buf.len();
        let mut buf = vec![0u8; to_read];

        let nr = match fd.read(&mut buf) {
            Ok(0) => {
                return Err(Error::ConnectionBroken);
            },
            Ok(nr) => nr,
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

            self.preamble = preamble;
            self.preamble_buf.clear();
            Ok(true)
        }
        else {
            Ok(false)
        }
    }

    pub fn read_payload<R: Read, W: Write>(&mut self, input: &mut R, output: &mut W) -> Result<usize, Error> {
        assert!(self.preamble.is_some());

        let preamble = self.preamble.as_ref().unwrap();
        assert!((preamble.length as u64) < self.num_read);
        
        let to_read = self.num_read - (preamble.length as u64);
        let max_read = 
            if to_read > 65536 {
                65536
            }
            else {
                to_read as usize
            };

        let mut buf = [0u8; 65536];
        let nr = match input.read(&mut buf[0..max_read]) {
            Ok(nr) => nr,
            Err(e) => match e.kind() {
                io::ErrorKind::WouldBlock => {
                    return Err(Error::Blocked);
                }
                _ => {
                    return Err(Error::ReadError(e));
                }
            }
        };

        output.write_all(&buf[0..nr])
            .map_err(|e| {
                match e.kind() {
                    io::ErrorKind::WouldBlock => Error::Blocked,
                    _ => Error::WriteError(e)
                }
            })?;

        self.num_read += nr as u64;
        if self.num_read >= preamble.length as u64 {
            self.reset();
        }

        Ok(nr)
    }
}

impl BitcoinProxy {
    pub fn new(magic: u32, port: u16, backend_addr: SocketAddr) -> Result<BitcoinProxy, Error> {
        let net = NetworkState::bind(&format!("0.0.0.0:{}", port).parse::<SocketAddr>().unwrap(), 1000)?;
        Ok(BitcoinProxy {
            network: net,
            magic: magic,
            backend_addr: backend_addr,
            blacklist: vec![],
            whitelist: vec![],
            messages: HashMap::new(),
            downstream: HashMap::new(),
            upstream: HashMap::new(),
        })
    }

    pub fn add_whitelist(&mut self, cmd: BitcoinCommand) -> () {
        self.whitelist.push(cmd);
    }

    pub fn add_blacklist(&mut self, cmd: BitcoinCommand) -> () {
        self.blacklist.push(cmd);
    }

    pub fn filter(preamble: &BitcoinPreamble, whitelist: &Vec<BitcoinCommand>, blacklist: &Vec<BitcoinCommand>) -> bool {
        if whitelist.len() > 0 {
            for cmd in whitelist.iter() {
                if *cmd == preamble.command {
                    return true;
                }
            }
            debug!("Filtered non-whitelisted command '{:?}'", preamble.command.to_string());
            return false;
        }

        if blacklist.len() > 0 {
            for cmd in blacklist.iter() {
                if *cmd == preamble.command {
                    debug!("Filtered blacklisted command '{:?}'", preamble.command.to_string());
                    return false;
                }
            }

            return true;
        }

        return false;
    }

    fn register_socket(&mut self, event_id: usize, socket: mio_net::TcpStream) -> Result<(), Error> {
        let upstream_sync = net::TcpStream::connect_timeout(&self.backend_addr, Duration::new(2, 0)).map_err(|_e| Error::ConnectionError)?;
        let upstream = mio_net::TcpStream::from_stream(upstream_sync).map_err(|_e| Error::ConnectionError)?;

        let forward = ForwardState::new();
        
        self.messages.insert(event_id, forward);
        self.downstream.insert(event_id, socket);
        self.upstream.insert(event_id, upstream);

        debug!("Registered event {}", event_id);
        Ok(())
    }

    fn deregister_socket(&mut self, event_id: usize) -> () {
        self.messages.remove(&event_id);
        self.downstream.remove(&event_id);
        self.upstream.remove(&event_id);

        debug!("De-egistered event {}", event_id);
    }

    pub fn process_new_sockets(&mut self, pollstate: &mut NetworkPollState) -> () {
        for (event_id, sock) in pollstate.new.drain() {
            let _ = self.register_socket(event_id, sock);
        }
    }

    pub fn process_socket_io(magic: u32, whitelist: &Vec<BitcoinCommand>, blacklist: &Vec<BitcoinCommand>, inbound: &mut mio_net::TcpStream, forward: &mut ForwardState, upstream: &mut mio_net::TcpStream) -> bool {
        loop {
            if forward.preamble.is_none() {
                match forward.read_preamble(magic, inbound) {
                    Ok(true) => {},
                    Ok(false) => {
                        return true;
                    },
                    Err(Error::Blocked) => {
                        return true;
                    },
                    Err(_e) => {
                        return false;
                    }
                }
            }

            if forward.preamble.is_some() {
                if BitcoinProxy::filter(&forward.preamble.as_ref().unwrap(), whitelist, blacklist) {
                    return false;
                }

                match forward.read_payload(inbound, upstream) {
                    Ok(0) => {
                        return false;
                    },
                    Ok(_) => {},
                    Err(Error::Blocked) => {
                        return true;
                    }
                    Err(_e) => {
                        return false;
                    }
                }
            }
        }
    }

    pub fn process_ready_sockets(&mut self, pollstate: &mut NetworkPollState) -> () {
        let mut broken = vec![];

        for event_id in pollstate.ready.iter() {
            match (self.messages.get_mut(event_id), self.downstream.get_mut(event_id), self.upstream.get_mut(event_id)) {
                (Some(ref mut forward), Some(ref mut downstream), Some(ref mut upstream)) => {
                    let alive = BitcoinProxy::process_socket_io(self.magic, &self.whitelist, &self.blacklist, downstream, forward, upstream);
                    if !alive {
                        broken.push(*event_id);
                    }
                }
                (_, _, _) => {
                    broken.push(*event_id);
                }
            }
        }

        for event_id in broken {
            self.deregister_socket(event_id);
        }
    }

    pub fn poll(&mut self, timeout: u64) -> Result<(), Error> {
        let mut poll_state = self.network.poll(timeout)?;
        self.process_new_sockets(&mut poll_state);
        self.process_ready_sockets(&mut poll_state);
        Ok(())
    }
}

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

fn usage(prog_name: String) -> () {
    eprintln!("Usage: {} [-w|-b MSG1,MSG2,MSG3,...] [-v VERBOSITY] [-n NETWORK-NAME] port upstream_addr", prog_name);
    process::exit(1);
}

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

    let mut proxy = BitcoinProxy::new(magic, port, backend).unwrap();
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

