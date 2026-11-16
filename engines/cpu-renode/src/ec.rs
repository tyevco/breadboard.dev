//! The External Control exposure fix (ADR-015 D5, Phase 2 H3): a reverse
//! connection.
//!
//! Renode 1.17.0's External Control server binds `IPAddress.Any` and calls
//! `Accept()` on a raw socket, with no hook to refuse a peer
//! (`ExternalControlSocket.cs` at `v1.17.0`, lines 166-180 and 252). So
//! Renode never listens: `bb` listens on `127.0.0.1` ([`EcListener`]) and
//! the script ends with `emulation CreateExternalControlClient "bb" <port>`,
//! with which Renode connects out to `IPAddress.Loopback` (line 174).
//!
//! `bb`'s listener admits one peer: the connection whose socket belongs to
//! `bb`'s own Renode process (its child, or the leased pool process). The
//! check reads the peer's socket inode from `/proc/net/tcp` (the entry whose
//! local address is the peer's and whose remote address is the listener's)
//! and looks for `socket:[<inode>]` among `/proc/<pid>/fd`. Any other
//! connection is shut down and closed before a byte is read from it.
//!
//! Facts of Renode 1.17.0's client mode that the engine relies on (checked
//! against the sources at the `v1.17.0` tag and a running Renode, ADR-015
//! Amendment 2):
//!
//! - `ExternalControlClient`'s constructor sends `CHECK_VERSION` from the
//!   monitor thread, so with the handler of a non-default thread: its id
//!   has bit 15 set (`0x8001`). It blocks the monitor until `bb` answers.
//! - Renode's default handler thread serves `bb`'s requests from the
//!   *internal* handler in client mode (`GetHandlerForCurrentThread`), and
//!   incoming messages are routed by id bit 15
//!   (`GetHandlerForMessage`), so `bb`'s request ids must have bit 15
//!   clear ([`crate::protocol::Client::reverse`]).
//! - A client whose peer closes is disposed, not restarted
//!   (`Disconnect(isClient ? State.Disposed : State.Unconnected)`, line
//!   295). The engine's reconnect (ADR-004 Amendment 8) therefore calls the
//!   same object's private `RestartConnection` by reflection from a monitor
//!   command ([`RECONNECT_PY`]): the GPIO hooks of the first connection
//!   send through that object, so events keep arriving once and are not
//!   registered again, as with the server's restart before.

use std::io;
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

/// The monitor command that reconnects Renode's External Control client to
/// the port it was created with (ADR-015 Amendment 2). It calls
/// `ExternalControlSocket.RestartConnection` (private: a new socket,
/// `Connect` to `IPAddress.Loopback`, new handlers and a new receive
/// thread) on the one External Control object of the emulation, and prints
/// `bb-ec-reconnect <n>` with the number of objects restarted. It must be
/// sent only after the previous connection's threads have ended
/// ([`wait_ec_closed`]); it touches no emulation state.
pub const RECONNECT_PY: &str = "python \"import clr; \
from System.Reflection import BindingFlags; \
from Antmicro.Renode.Core import EmulationManager; \
from Antmicro.Renode.Network import ExternalControlSocket; \
bb_m = clr.GetClrType(ExternalControlSocket).GetMethod('RestartConnection', BindingFlags.NonPublic | BindingFlags.Instance); \
bb_e = [e for e in EmulationManager.Instance.CurrentEmulation.ExternalsManager.Externals if isinstance(e, ExternalControlSocket)]; \
[bb_m.Invoke(e, None) for e in bb_e]; \
print('bb-ec-reconnect %d' % len(bb_e))\"";

/// The line [`RECONNECT_PY`] prints when it restarted exactly one client.
pub const RECONNECT_OK: &str = "bb-ec-reconnect 1";

/// The script line that makes Renode connect out to `bb`'s listener.
pub fn create_client_line(port: u16) -> String {
    format!("emulation CreateExternalControlClient \"bb\" {port}")
}

/// `bb`'s External Control listener on `127.0.0.1` (an ephemeral port). It
/// lives as long as the engine: Renode reconnects to the same port.
pub struct EcListener {
    listener: TcpListener,
    port: u16,
    refused: u64,
}

impl EcListener {
    /// Bind `127.0.0.1:0`.
    pub fn bind() -> io::Result<EcListener> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        Ok(EcListener {
            listener,
            port,
            refused: 0,
        })
    }

    /// The listener's port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Connections refused so far (not owned by the Renode process).
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Wait until process `pid` connects, refusing every other connection.
    /// `check` runs between polls (the child exited, the pool reported an
    /// error) and ends the wait with its error. Wall-clock bounded by
    /// `deadline`, polled every `poll`; nothing here reaches a result.
    pub fn accept_from(
        &mut self,
        pid: u32,
        deadline: Instant,
        poll: Duration,
        mut check: impl FnMut() -> Result<(), String>,
    ) -> Result<TcpStream, String> {
        loop {
            match self.listener.accept() {
                Ok((s, peer)) => {
                    // Accepted sockets inherit the listener's non-blocking
                    // flag on some platforms; the client is blocking.
                    let _ = s.set_nonblocking(false);
                    if peer_is_process(&s, pid) {
                        return Ok(s);
                    }
                    let _ = s.shutdown(Shutdown::Both);
                    drop(s);
                    self.refused += 1;
                    eprintln!(
                        "bb: refused an External Control connection from {peer}: \
                         its socket is not Renode's (pid {pid})"
                    );
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    check()?;
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "Renode (pid {pid}) did not connect to 127.0.0.1:{} in time",
                            self.port
                        ));
                    }
                    std::thread::sleep(poll);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("External Control listener: {e}")),
            }
        }
    }
}

/// Whether the peer of `s` (a loopback connection accepted by this
/// process) is a socket of process `pid`.
pub fn peer_is_process(s: &TcpStream, pid: u32) -> bool {
    let (Ok(SocketAddr::V4(peer)), Ok(SocketAddr::V4(local))) = (s.peer_addr(), s.local_addr())
    else {
        return false;
    };
    let Ok(table) = std::fs::read_to_string("/proc/net/tcp") else {
        return false;
    };
    match tcp_socket_inode(&table, peer, local) {
        Some(inode) => process_has_socket(pid, inode),
        None => false,
    }
}

/// The inode of the socket in `/proc/net/tcp` text `table` whose local
/// address is `local` and whose remote address is `remote` (`None` if
/// there is none, or its inode is 0).
pub fn tcp_socket_inode(table: &str, local: SocketAddrV4, remote: SocketAddrV4) -> Option<u64> {
    let (l, r) = (proc_addr(local), proc_addr(remote));
    table.lines().skip(1).find_map(|line| {
        // sl local rem st tx:rx tr:when retrnsmt uid timeout inode
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.len() > 9 && w[1].eq_ignore_ascii_case(&l) && w[2].eq_ignore_ascii_case(&r) {
            w[9].parse().ok().filter(|i| *i != 0)
        } else {
            None
        }
    })
}

/// An IPv4 address as `/proc/net/tcp` prints it: the address word in host
/// byte order and the port, both in hex (`0100007F:1F90` for
/// 127.0.0.1:8080 on a little-endian host).
fn proc_addr(a: SocketAddrV4) -> String {
    format!(
        "{:08X}:{:04X}",
        u32::from_ne_bytes(a.ip().octets()),
        a.port()
    )
}

/// Whether one of process `pid`'s file descriptors is socket `inode`.
pub fn process_has_socket(pid: u32, inode: u64) -> bool {
    let want = format!("socket:[{inode}]");
    let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return false;
    };
    rd.flatten()
        .any(|e| std::fs::read_link(e.path()).is_ok_and(|t| t.as_os_str() == want.as_str()))
}

/// Whether process `pid` has an established connection to
/// `127.0.0.1:port` (`/proc/net/tcp`). The exposure gate uses it to know
/// that its foreign client is connected before Renode is.
pub fn connected_to(pid: u32, port: u16) -> bool {
    let rem = proc_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    std::fs::read_to_string("/proc/net/tcp")
        .unwrap_or_default()
        .lines()
        .skip(1)
        .any(|ln| {
            let w: Vec<&str> = ln.split_whitespace().collect();
            w.len() > 9
                && w[2] == rem
                && w[3] == "01"
                && w[9].parse().is_ok_and(|i| process_has_socket(pid, i))
        })
}

/// Threads of the External Control object in the process whose
/// `/proc/<pid>/task` is `task_dir`: Renode names them
/// `ExternalControlClient_RxHandler` and `…_DefaultHandlerThread` (`comm`
/// is cut to 15 bytes).
pub fn ec_threads(task_dir: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(task_dir) else {
        return 0;
    };
    rd.flatten()
        .filter(|t| {
            std::fs::read_to_string(t.path().join("comm"))
                .is_ok_and(|c| c.starts_with("ExternalControl"))
        })
        .count()
}

/// After `bb` closed the connection: wait until Renode has disposed of its
/// External Control client, i.e. its receive thread has returned and the
/// default handler thread has been joined (`Disconnect(State.Disposed)`).
/// Then a `RestartConnection`, a `Clear` or an exit races nothing. `false`
/// if that did not happen within `limit` or the process is gone.
pub fn wait_ec_closed(pid: u32, limit: Duration) -> bool {
    let tasks = std::path::PathBuf::from(format!("/proc/{pid}/task"));
    let end = Instant::now() + limit;
    loop {
        if !tasks.exists() {
            return false;
        }
        if ec_threads(&tasks) == 0 {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn proc_addr_format() {
        let a = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8080);
        if cfg!(target_endian = "little") {
            assert_eq!(proc_addr(a), "0100007F:1F90");
        }
    }

    #[test]
    fn tcp_socket_inode_matches_both_ends() {
        let a = |p| SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), p);
        let la = proc_addr(a(40000));
        let ra = proc_addr(a(8080));
        let table = format!(
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
             0: {ra} 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1\n\
             1: {ra} {la} 01 00000000:00000000 00:00000000 00000000  1000        0 222 1\n\
             2: {la} {ra} 01 00000000:00000000 00:00000000 00000000  1000        0 333 1\n\
             3: {la} {la} 06 00000000:00000000 00:00000000 00000000  1000        0 0 1\n"
        );
        assert_eq!(tcp_socket_inode(&table, a(40000), a(8080)), Some(333));
        assert_eq!(tcp_socket_inode(&table, a(8080), a(40000)), Some(222));
        assert_eq!(tcp_socket_inode(&table, a(40001), a(8080)), None);
        // A TIME_WAIT entry has inode 0: no socket.
        assert_eq!(tcp_socket_inode(&table, a(40000), a(40000)), None);
    }

    /// A connection from this process is ours, not another pid's; one from a
    /// foreign process (a `bash` with `/dev/tcp`) is refused and gets no
    /// byte, and the listener then still admits the owner.
    #[test]
    fn listener_admits_only_the_owner() {
        let mut l = EcListener::bind().unwrap();
        let port = l.port();
        let me = std::process::id();
        // A foreign process connects first and waits for a byte.
        let mut foreign = std::process::Command::new("bash")
            .args([
                "-c",
                &format!("exec 3<>/dev/tcp/127.0.0.1/{port}; head -c 1 <&3 | wc -c"),
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // Wait until its connection is queued, then connect ours.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !connected_to(foreign.id(), port) {
            assert!(
                Instant::now() < deadline,
                "the foreign client did not connect"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let own = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        let s = l
            .accept_from(me, Instant::now() + Duration::from_secs(10), MS, || Ok(()))
            .unwrap();
        assert_eq!(s.peer_addr().unwrap(), own.local_addr().unwrap());
        assert_eq!(l.refused(), 1);
        let mut out = String::new();
        foreign
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        foreign.wait().unwrap();
        assert_eq!(out.trim(), "0", "the foreign client read a byte");
        // A pid that is not ours never matches our own connection.
        assert!(!peer_is_process(&s, 1));
    }

    #[test]
    fn accept_from_ends_on_check_and_deadline() {
        let mut l = EcListener::bind().unwrap();
        let e = l
            .accept_from(1, Instant::now() + Duration::from_secs(10), MS, || {
                Err("child exited".into())
            })
            .unwrap_err();
        assert_eq!(e, "child exited");
        let e = l.accept_from(1, Instant::now(), MS, || Ok(())).unwrap_err();
        assert!(e.contains("did not connect"), "{e}");
    }

    #[test]
    fn ec_threads_reads_comm() {
        let root = std::env::temp_dir().join(format!("bb-ec-threads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(ec_threads(&root), 0, "missing directory");
        let t = |tid: &str, comm: &str| {
            let d = root.join(tid);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
        };
        t("1", "Renode");
        assert_eq!(ec_threads(&root), 0);
        t("2", "ExternalControl");
        t("3", "ExternalControl");
        assert_eq!(ec_threads(&root), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
