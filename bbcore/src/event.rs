//! JSON-lines event stream. Schema: `spec/event-stream.md`.
//!
//! Determinism: every record is a Rust struct variant serialized by serde, so
//! the key order is fixed (`type` first, then fields in declaration order).
//! No maps with unordered keys and no floats appear in the records; voltages
//! are integer microvolts and times are integer femtoseconds.

use std::io::{self, Write};

use serde::{Deserialize, Serialize};

use crate::time::Time;

/// Severity of a [`Event::Log`] record.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// A log line produced by an engine during an advance (e.g. UART output).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRecord {
    /// Sim time the line was produced (must be <= the grant it was produced in).
    pub t: Time,
    pub level: LogLevel,
    pub msg: String,
}

/// A machine reset of a CPU engine during an advance (ADR-009 D7, milestone
/// P5): the `reset` record of its engine, `cause` `bor`. `ram` is `kept` on
/// `exit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResetRecord {
    /// Sim time of the reset (must be <= the grant it happened in).
    pub t: Time,
    pub phase: ResetPhase,
}

/// One line of the event stream.
///
/// Field order here **is** the on-disk key order; changing it is a schema
/// change and needs an ADR (see AGENTS.md).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// The kernel granted every engine the interval `(t_fs, until_fs]`.
    Grant {
        t_fs: Time,
        until_fs: Time,
        /// What bounded the grant: `engine:<name>`, `sample`, `loop_quantum`,
        /// `max_quantum` or `t_end` (schema version 1, ADR-003 D4).
        limited_by: String,
    },
    /// A digital net changed level at `t_fs`.
    PinEdge {
        t_fs: Time,
        net: String,
        /// Driving pin as `<engine>.<pin>`.
        driver: String,
        /// New level, 0 or 1.
        level: u8,
    },
    /// A probed net's value at a fixed-rate sample time.
    NetSample {
        t_fs: Time,
        net: String,
        /// Analog value in integer microvolts (absent for digital nets).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uv: Option<i64>,
        /// Digital level 0/1 (absent for analog nets).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        level: Option<u8>,
    },
    /// A log line from an engine (or the kernel, source `kernel`).
    Log {
        t_fs: Time,
        source: String,
        level: LogLevel,
        msg: String,
    },
    /// Result of a check (event stream version 2, ADR-010 D7): emitted by
    /// the assertions of a bench or a test (milestone B2), never by the
    /// scheduler itself. `value` and `limit` are integers in the base unit
    /// `unit` (`uV`, `fs`, `nA`, `udegC` for µ°C, or `count`); `pass` is present
    /// unless the outcome is `not_evaluable`, for version 1 consumers.
    Assert {
        t_fs: Time,
        name: String,
        outcome: AssertOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pass: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
        /// The lowest chosen rung over the instances on the nets the check
        /// reads (ADR-009 D1).
        rung: String,
        msg: String,
    },
    /// A machine reset of a CPU engine (event stream version 2, ADR-009 D7;
    /// milestone P5): `cause` is `bor`, `phase` is `enter`, `exit` or
    /// `stop`; `ram` is `kept` on `exit`. It sorts like a `log` of its
    /// engine.
    Reset {
        t_fs: Time,
        source: String,
        cause: String,
        phase: ResetPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ram: Option<String>,
    },
}

/// An `assert` record's outcome (event stream version 2, ADR-010 D7).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertOutcome {
    Pass,
    Fail,
    NotEvaluable,
}

/// A `reset` record's phase (event stream version 2, ADR-009 D7).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetPhase {
    Enter,
    Exit,
    Stop,
}

impl ResetPhase {
    /// The record's `phase` string.
    pub fn as_str(&self) -> &'static str {
        match self {
            ResetPhase::Enter => "enter",
            ResetPhase::Exit => "exit",
            ResetPhase::Stop => "stop",
        }
    }

    /// The phase named `s` (`enter`, `exit` or `stop`).
    pub fn parse(s: &str) -> Option<ResetPhase> {
        match s {
            "enter" => Some(ResetPhase::Enter),
            "exit" => Some(ResetPhase::Exit),
            "stop" => Some(ResetPhase::Stop),
            _ => None,
        }
    }
}

/// The event stream's schema version (`spec/event-stream.md`; version 2
/// with the Phase 2 schema epoch). The stream carries no version field: a
/// version 1 file is a version 2 file, since version 2 only adds records.
pub const EVENT_STREAM_VERSION: u32 = 2;

impl Event {
    /// The record's `t_fs`.
    pub fn time(&self) -> Time {
        match self {
            Event::Grant { t_fs, .. }
            | Event::PinEdge { t_fs, .. }
            | Event::NetSample { t_fs, .. }
            | Event::Log { t_fs, .. }
            | Event::Assert { t_fs, .. }
            | Event::Reset { t_fs, .. } => *t_fs,
        }
    }

    /// Parse one line of the stream.
    pub fn from_line(line: &str) -> Result<Event, serde_json::Error> {
        serde_json::from_str(line)
    }
}

/// Writes [`Event`]s as JSON lines (`\n` terminated, no trailing spaces).
pub struct EventSink<W: Write> {
    w: W,
    lines: u64,
}

impl<W: Write> EventSink<W> {
    pub fn new(w: W) -> Self {
        EventSink { w, lines: 0 }
    }

    /// Serialize one record and a newline.
    pub fn emit(&mut self, ev: &Event) -> io::Result<()> {
        serde_json::to_writer(&mut self.w, ev).map_err(io::Error::from)?;
        self.w.write_all(b"\n")?;
        self.lines += 1;
        Ok(())
    }

    /// Number of records written so far.
    pub fn lines(&self) -> u64 {
        self.lines
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.w.flush()
    }

    pub fn get_ref(&self) -> &W {
        &self.w
    }

    pub fn into_inner(self) -> W {
        self.w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_order_is_fixed() {
        let mut sink = EventSink::new(Vec::new());
        sink.emit(&Event::Grant {
            t_fs: Time::ZERO,
            until_fs: Time::from_us(10),
            limited_by: "engine:renode".into(),
        })
        .unwrap();
        sink.emit(&Event::NetSample {
            t_fs: Time::from_us(10),
            net: "LED_A".into(),
            uv: Some(1_900_000),
            level: None,
        })
        .unwrap();
        sink.emit(&Event::PinEdge {
            t_fs: Time::from_ms(50),
            net: "PA5".into(),
            driver: "renode.PA5".into(),
            level: 1,
        })
        .unwrap();
        let out = String::from_utf8(sink.into_inner()).unwrap();
        assert_eq!(
            out,
            concat!(
                r#"{"type":"grant","t_fs":0,"until_fs":10000000000,"limited_by":"engine:renode"}"#,
                "\n",
                r#"{"type":"net_sample","t_fs":10000000000,"net":"LED_A","uv":1900000}"#,
                "\n",
                r#"{"type":"pin_edge","t_fs":50000000000000,"net":"PA5","driver":"renode.PA5","level":1}"#,
                "\n",
            )
        );
        for line in out.lines() {
            let ev = Event::from_line(line).unwrap();
            assert_eq!(serde_json::to_string(&ev).unwrap(), line);
        }
    }

    #[test]
    fn version_2_records_have_the_documented_key_order() {
        // ADR-010 D7 (the spec's example) and ADR-009 D7.
        let a = Event::Assert {
            t_fs: Time::from_fs(10_500_000_000_000_000),
            name: "LED-DATA-V4".into(),
            outcome: AssertOutcome::Pass,
            pass: Some(true),
            value: Some(2_971_511),
            limit: Some(2_670_000),
            unit: Some("uV".into()),
            rung: "fast".into(),
            msg: "VLED at P0.06 rise".into(),
        };
        let line = serde_json::to_string(&a).unwrap();
        assert_eq!(
            line,
            r#"{"type":"assert","t_fs":10500000000000000,"name":"LED-DATA-V4","outcome":"pass","pass":true,"value":2971511,"limit":2670000,"unit":"uV","rung":"fast","msg":"VLED at P0.06 rise"}"#
        );
        assert_eq!(Event::from_line(&line).unwrap(), a);
        let ne = Event::Assert {
            t_fs: Time::ZERO,
            name: "LSW-2".into(),
            outcome: AssertOutcome::NotEvaluable,
            pass: None,
            value: None,
            limit: None,
            unit: None,
            rung: "pi".into(),
            msg: "deadline after the run's end".into(),
        };
        assert_eq!(
            serde_json::to_string(&ne).unwrap(),
            r#"{"type":"assert","t_fs":0,"name":"LSW-2","outcome":"not_evaluable","rung":"pi","msg":"deadline after the run's end"}"#
        );
        let r = Event::Reset {
            t_fs: Time::from_us(10),
            source: "renode".into(),
            cause: "bor".into(),
            phase: ResetPhase::Exit,
            ram: Some("kept".into()),
        };
        let line = serde_json::to_string(&r).unwrap();
        assert_eq!(
            line,
            r#"{"type":"reset","t_fs":10000000000,"source":"renode","cause":"bor","phase":"exit","ram":"kept"}"#
        );
        assert_eq!(Event::from_line(&line).unwrap(), r);
        assert_eq!(EVENT_STREAM_VERSION, 2);
    }
}
