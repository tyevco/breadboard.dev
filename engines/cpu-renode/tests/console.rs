//! The Renode console driver (ADR-004 Part B, plan step 2) against a real
//! Renode 1.17.0: batches end at their sync line, errors are visible,
//! `Clear` disposes a machine, the output reaches the lease log, and the
//! External Control server's life cycle that the pool depends on.
//!
//! Needs Renode and the spike firmware like `blinky.rs`: with
//! `BB_REQUIRE_RENODE=1` a missing tool fails the test instead of skipping.

use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use bb_cpu_renode::console::{Console, COMMAND_ERROR};
use bb_cpu_renode::protocol::Client;
use bb_cpu_renode::{find_renode, STM32F103_OFFLINE_REPL};

const T: Duration = Duration::from_secs(60);

fn skip(msg: &str) {
    let required = std::env::var("BB_REQUIRE_RENODE").is_ok_and(|v| v == "1");
    assert!(!required, "BB_REQUIRE_RENODE=1 but {msg}");
    eprintln!("SKIP: {msg}");
}

fn workdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bb-console-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn blinky() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/spike/firmware");
    let ok = Command::new("make")
        .args(["-s", "-C"])
        .arg(&dir)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        skip("building the spike firmware failed (run `make setup`)");
        return None;
    }
    Some(std::fs::canonicalize(dir.join("build/blinky.elf")).unwrap())
}

fn console(dir: &std::path::Path) -> Option<Console> {
    let Some(renode) = find_renode() else {
        skip("renode not installed (run `make setup`, or set BB_RENODE)");
        return None;
    };
    Some(Console::spawn(&renode, dir, T).expect("spawn console renode"))
}

fn port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A session like the engine's: the offline STM32F103, the blinky ELF, the
/// determinism settings and an External Control server.
fn session(dir: &std::path::Path, elf: &std::path::Path, port: u16) -> PathBuf {
    let repl = dir.join("platform.repl");
    std::fs::write(&repl, STM32F103_OFFLINE_REPL).unwrap();
    let s = dir.join(format!("session-{port}.resc"));
    std::fs::write(
        &s,
        format!(
            "mach create \"bb\"\n\
             machine LoadPlatformDescription @{}\n\
             sysbus LoadELF @{}\n\
             emulation SetSeed 0\n\
             emulation SetGlobalQuantum \"0.000010000\"\n\
             emulation SetGlobalSerialExecution true\n\
             emulation SetGlobalAdvanceImmediately true\n\
             logLevel 2\n\
             emulation CreateExternalControlServer \"bb\" {port}\n",
            repl.display(),
            elf.display()
        ),
    )
    .unwrap();
    s
}

fn wait(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    f()
}

#[test]
fn batches_errors_clear_and_log() {
    let dir = workdir("batch");
    let Some(mut c) = console(&dir) else { return };
    // A batch ends at its sync line; the output holds the echo.
    let out = c.run(&["Clear"], T).unwrap();
    assert!(!out.contains(COMMAND_ERROR), "{out}");
    // A failing command inside an include is reported, and the console goes
    // on with the next batch.
    let bad = dir.join("bad.resc");
    std::fs::write(
        &bad,
        "mach create \"x\"\nmachine LoadPlatformDescription @/nonexistent/bb.repl\n",
    )
    .unwrap();
    let out = c.run(&[&format!("include @{}", bad.display())], T).unwrap();
    assert!(out.contains(COMMAND_ERROR), "{out}");
    // An unknown command is reported, the sync line after it still answers.
    let out = c.run(&["bb_no_such_command"], T).unwrap();
    assert!(
        out.contains("No such command or device: bb_no_such_command"),
        "{out}"
    );
    // Clear disposes the machine that the failed include created.
    let out = c.run(&["Clear"], T).unwrap();
    assert!(out.contains("x: Disposed"), "{out}");
    let out = c.run(&["mach"], T).unwrap();
    assert!(out.contains("No machine selected"), "{out}");
    // Log routing: the output goes to the lease's log while it is set.
    let log = dir.join("renode.log");
    c.set_log(Some(std::fs::File::create(&log).unwrap()));
    c.run(&["bb_logged_command"], T).unwrap();
    c.set_log(None);
    c.run(&["bb_not_logged"], T).unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("bb_logged_command"), "{text}");
    assert!(!text.contains("bb_not_logged"), "{text}");
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

/// What the pool's end of lease relies on (ADR-004 Amendment 2): after the
/// client disconnects, Renode 1.17.0's server listens again on the same
/// port (`ExternalControlSocket.Disconnect(Unconnected)` restarts it, bound
/// to 0.0.0.0), and `Clear` with the server in that state releases it. A
/// second session in the same process gives the same time and pin state.
#[test]
fn external_control_relisten_and_clear() {
    let Some(elf) = blinky() else { return };
    let dir = workdir("ec");
    let Some(mut c) = console(&dir) else { return };
    let mut first = None;
    for round in 0..2 {
        let p = port();
        let s = session(&dir, &elf, p);
        let out = c
            .run(
                &["Clear", "logLevel 1", &format!("include @{}", s.display())],
                T,
            )
            .unwrap();
        assert!(!out.contains(COMMAND_ERROR), "{out}");
        assert!(
            c.listening_ports().contains(&p),
            "server listens after the include"
        );
        let stream = TcpStream::connect(("127.0.0.1", p)).unwrap();
        let mut cl = Client::handshake(stream, T).unwrap();
        let md = cl.get_machine("bb").unwrap();
        let gpio = cl.get_gpio(md, "gpioPortA").unwrap();
        cl.gpio_register_event(gpio, 5, 0).unwrap();
        cl.run_for(20_000_000).unwrap();
        let t = cl.get_time().unwrap();
        let events = cl.take_events();
        assert_eq!(t, 20_000_000);
        // Accepting closed the listening socket.
        assert!(
            !c.listening_ports().contains(&p),
            "no listener while connected"
        );
        cl.close();
        drop(cl);
        assert!(
            wait(Duration::from_secs(5), || c.listening_ports().contains(&p)),
            "round {round}: Renode listens again after the disconnect"
        );
        let out = c.run(&["Clear"], T).unwrap();
        assert!(!out.contains(COMMAND_ERROR), "{out}");
        assert!(
            wait(Duration::from_secs(5), || !c.listening_ports().contains(&p)),
            "round {round}: Clear releases the listener"
        );
        match &first {
            None => first = Some(events),
            Some(f) => assert_eq!(f, &events, "second session differs from the first"),
        }
    }
    assert!(c.listening_ports().is_empty(), "{:?}", c.listening_ports());
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}
