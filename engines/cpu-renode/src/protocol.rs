//! Minimal client for Renode's External Control API (protocol version 0).
//!
//! This is a Rust port of the parts of the reference C client shipped with
//! Renode 1.17.0 in `tools/external_control_client/lib/{connection.c,
//! renode_api.c,common.h}` that the CPU engine needs. Wire format, all
//! little-endian, all structs packed:
//!
//! ```text
//! envelope : u16 id, u32 size, then `size` payload bytes
//!            id has bit 15 set for client-initiated requests and their
//!            responses; server-initiated event requests have it clear.
//! payload  : u16 command, u8 type, then command data
//!            type: 0 request, 1 event request, 2 success, 3 invalid command, 4 error
//! commands : 0 CHECK_VERSION(u32 version = 0)
//!            1 RUN_FOR(u64 ns)                     -> success when the run is done
//!            2 GET_TIME(u64 ignored)               -> u64 ns
//!            3 GET_MACHINE(u32 len, name)          -> i32 machine descriptor
//!            5 GPIO instance(i32 -1, i32 md, u32 len, name) -> i32 gpio descriptor
//!            5 GPIO op(i32 gpio, i8 op, i32 pin [, ...])
//!                op 0 GET_STATE                    -> u8 state
//!                op 1 SET_STATE + u8 state
//!                op 2 REGISTER_EVENT + i32 event descriptor
//! events   : type 1, command 5: i32 event descriptor, u64 time_ns, u8 state,
//!            7 padding bytes (the C struct `renode_gpio_event_data_t`).
//!            The client must answer each with (command, success) under the
//!            same envelope id.
//! ```
//!
//! Renode's `renode_time_t` is an integer number of nanoseconds: the C client's
//! `renode_create_time(v, TU_NANOSECONDS)` multiplies by 1, and running for
//! 1 ns then reading the time returns 1 (verified against Renode 1.17.0).
//!
//! The client is single-threaded and synchronous: it never sends a new request
//! before the previous one has been answered, and it acknowledges GPIO events
//! inline while waiting for that answer. GPIO events therefore arrive in
//! emulation order and are returned to the caller in that order.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const CLIENT_INITIATED: u16 = 0x8000;
const PROTOCOL_VERSION: u32 = 0;

const CMD_CHECK_VERSION: u16 = 0;
const CMD_RUN_FOR: u16 = 1;
const CMD_GET_TIME: u16 = 2;
const CMD_GET_MACHINE: u16 = 3;
const CMD_GPIO: u16 = 5;

const TYPE_REQUEST: u8 = 0;
const TYPE_EVENT_REQUEST: u8 = 1;
const TYPE_SUCCESS: u8 = 2;
const TYPE_INVALID_COMMAND: u8 = 3;
const TYPE_ERROR: u8 = 4;

const GPIO_GET_STATE: i8 = 0;
const GPIO_SET_STATE: i8 = 1;
const GPIO_REGISTER_EVENT: i8 = 2;

/// A GPIO state change reported by Renode while it was running.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GpioEvent {
    /// Event descriptor chosen by the client at registration.
    pub descriptor: i32,
    /// Renode virtual time of the change, in nanoseconds.
    pub time_ns: u64,
    /// New state of the pin.
    pub state: bool,
}

/// Error talking to the External Control server.
#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    /// The server answered with an error or an unexpected message.
    Server(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Io(e) => write!(f, "external control I/O: {e}"),
            ProtocolError::Server(m) => write!(f, "external control: {m}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(e: io::Error) -> Self {
        ProtocolError::Io(e)
    }
}

type Result<T> = std::result::Result<T, ProtocolError>;

/// Synchronous External Control API connection.
pub struct Client {
    stream: TcpStream,
    next_id: u16,
    events: Vec<GpioEvent>,
}

impl Client {
    /// Wrap a connected stream and perform the version handshake.
    pub fn handshake(stream: TcpStream, io_timeout: Duration) -> Result<Client> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(io_timeout))?;
        stream.set_write_timeout(Some(io_timeout))?;
        let mut c = Client {
            stream,
            next_id: 0,
            events: Vec::new(),
        };
        c.request(CMD_CHECK_VERSION, &PROTOCOL_VERSION.to_le_bytes())?;
        Ok(c)
    }

    /// Descriptor of the machine called `name`.
    pub fn get_machine(&mut self, name: &str) -> Result<i32> {
        let mut d = Vec::new();
        d.extend_from_slice(&(name.len() as u32).to_le_bytes());
        d.extend_from_slice(name.as_bytes());
        let r = self.request(CMD_GET_MACHINE, &d)?;
        let md = read_i32(&r, "GET_MACHINE")?;
        if md < 0 {
            return Err(ProtocolError::Server(format!(
                "invalid machine descriptor {md} for {name:?}"
            )));
        }
        Ok(md)
    }

    /// Descriptor of the GPIO controller `name` (e.g. `gpioPortA`) in machine `md`.
    pub fn get_gpio(&mut self, md: i32, name: &str) -> Result<i32> {
        let mut d = Vec::new();
        d.extend_from_slice(&(-1i32).to_le_bytes());
        d.extend_from_slice(&md.to_le_bytes());
        d.extend_from_slice(&(name.len() as u32).to_le_bytes());
        d.extend_from_slice(name.as_bytes());
        let r = self.request(CMD_GPIO, &d)?;
        read_i32(&r, "GPIO instance")
    }

    /// Current state of pin `pin` of GPIO controller `gpio`.
    pub fn gpio_get_state(&mut self, gpio: i32, pin: i32) -> Result<bool> {
        let r = self.request(CMD_GPIO, &gpio_header(gpio, GPIO_GET_STATE, pin))?;
        match r.as_slice() {
            [b] => Ok(*b != 0),
            _ => Err(ProtocolError::Server(format!(
                "GPIO GET_STATE: expected 1 byte, got {}",
                r.len()
            ))),
        }
    }

    /// Drive pin `pin` of GPIO controller `gpio` (an input from Renode's side).
    pub fn gpio_set_state(&mut self, gpio: i32, pin: i32, state: bool) -> Result<()> {
        let mut d = gpio_header(gpio, GPIO_SET_STATE, pin);
        d.push(u8::from(state));
        self.request(CMD_GPIO, &d)?;
        Ok(())
    }

    /// Ask Renode to report every state change of the pin, tagged `descriptor`.
    pub fn gpio_register_event(&mut self, gpio: i32, pin: i32, descriptor: i32) -> Result<()> {
        let mut d = gpio_header(gpio, GPIO_REGISTER_EVENT, pin);
        d.extend_from_slice(&descriptor.to_le_bytes());
        self.request(CMD_GPIO, &d)?;
        Ok(())
    }

    /// Run the emulation for exactly `ns` nanoseconds of virtual time and
    /// block until Renode has paused again.
    pub fn run_for(&mut self, ns: u64) -> Result<()> {
        self.request(CMD_RUN_FOR, &ns.to_le_bytes())?;
        Ok(())
    }

    /// Renode's elapsed virtual time in nanoseconds.
    pub fn get_time(&mut self) -> Result<u64> {
        // The C client sends 8 bytes here (the output buffer); the server
        // ignores their value.
        let r = self.request(CMD_GET_TIME, &0u64.to_le_bytes())?;
        let b: [u8; 8] = r.as_slice().try_into().map_err(|_| {
            ProtocolError::Server(format!("GET_TIME: expected 8 bytes, got {}", r.len()))
        })?;
        Ok(u64::from_le_bytes(b))
    }

    /// GPIO events received since the last call, in arrival (emulation) order.
    pub fn take_events(&mut self) -> Vec<GpioEvent> {
        std::mem::take(&mut self.events)
    }

    fn request(&mut self, cmd: u16, data: &[u8]) -> Result<Vec<u8>> {
        let id = CLIENT_INITIATED | self.next_id;
        self.next_id = (self.next_id + 1) & !CLIENT_INITIATED;
        let mut payload = Vec::with_capacity(3 + data.len());
        payload.extend_from_slice(&cmd.to_le_bytes());
        payload.push(TYPE_REQUEST);
        payload.extend_from_slice(data);
        self.send(id, &payload)?;
        loop {
            let (mid, body) = self.recv()?;
            if body.len() < 3 {
                return Err(ProtocolError::Server(format!(
                    "short payload ({} bytes)",
                    body.len()
                )));
            }
            let rcmd = u16::from_le_bytes([body[0], body[1]]);
            let rtype = body[2];
            let rdata = &body[3..];
            if mid & CLIENT_INITIATED == 0 {
                self.server_request(mid, rcmd, rtype, rdata)?;
                continue;
            }
            if mid != id {
                return Err(ProtocolError::Server(format!(
                    "response id {mid:#06x} does not match request id {id:#06x}"
                )));
            }
            if rcmd != cmd {
                return Err(ProtocolError::Server(format!(
                    "response for command {rcmd}, expected {cmd}"
                )));
            }
            return match rtype {
                TYPE_SUCCESS => Ok(rdata.to_vec()),
                TYPE_INVALID_COMMAND => Err(ProtocolError::Server(format!(
                    "command {cmd} rejected as invalid"
                ))),
                TYPE_ERROR => Err(ProtocolError::Server(format!(
                    "command {cmd} failed: {}",
                    String::from_utf8_lossy(rdata).trim_end_matches('\0')
                ))),
                t => Err(ProtocolError::Server(format!(
                    "unexpected message type {t} in response to command {cmd}"
                ))),
            };
        }
    }

    fn server_request(&mut self, mid: u16, cmd: u16, rtype: u8, data: &[u8]) -> Result<()> {
        if rtype != TYPE_EVENT_REQUEST || cmd != CMD_GPIO {
            // Answer like the C client does for anything it cannot service.
            self.send(mid, &response_header(cmd, TYPE_INVALID_COMMAND))?;
            return Err(ProtocolError::Server(format!(
                "unexpected server request: command {cmd}, type {rtype}"
            )));
        }
        // i32 descriptor, u64 time, u8 state (+ struct padding).
        if data.len() < 13 {
            return Err(ProtocolError::Server(format!(
                "GPIO event: expected at least 13 bytes, got {}",
                data.len()
            )));
        }
        let descriptor = i32::from_le_bytes(data[0..4].try_into().expect("4 bytes"));
        let time_ns = u64::from_le_bytes(data[4..12].try_into().expect("8 bytes"));
        let state = data[12] != 0;
        self.events.push(GpioEvent {
            descriptor,
            time_ns,
            state,
        });
        self.send(mid, &response_header(cmd, TYPE_SUCCESS))
    }

    fn send(&mut self, id: u16, payload: &[u8]) -> Result<()> {
        let mut buf = Vec::with_capacity(6 + payload.len());
        buf.extend_from_slice(&id.to_le_bytes());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        self.stream.write_all(&buf)?;
        Ok(())
    }

    fn recv(&mut self) -> Result<(u16, Vec<u8>)> {
        let mut env = [0u8; 6];
        self.stream.read_exact(&mut env)?;
        let id = u16::from_le_bytes([env[0], env[1]]);
        let size = u32::from_le_bytes([env[2], env[3], env[4], env[5]]) as usize;
        let mut body = vec![0u8; size];
        self.stream.read_exact(&mut body)?;
        Ok((id, body))
    }
}

fn gpio_header(gpio: i32, op: i8, pin: i32) -> Vec<u8> {
    let mut d = Vec::with_capacity(13);
    d.extend_from_slice(&gpio.to_le_bytes());
    d.extend_from_slice(&op.to_le_bytes());
    d.extend_from_slice(&pin.to_le_bytes());
    d
}

fn response_header(cmd: u16, rtype: u8) -> [u8; 3] {
    let c = cmd.to_le_bytes();
    [c[0], c[1], rtype]
}

fn read_i32(r: &[u8], what: &str) -> Result<i32> {
    let b: [u8; 4] = r
        .try_into()
        .map_err(|_| ProtocolError::Server(format!("{what}: expected 4 bytes, got {}", r.len())))?;
    Ok(i32::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpio_header_layout_is_packed() {
        // i32 id, i8 op, i32 pin = 9 bytes, like the packed C struct.
        assert_eq!(
            gpio_header(3, GPIO_REGISTER_EVENT, 5),
            vec![3, 0, 0, 0, 2, 5, 0, 0, 0]
        );
    }

    #[test]
    fn response_header_layout() {
        assert_eq!(response_header(CMD_GPIO, TYPE_SUCCESS), [5, 0, 2]);
    }
}
