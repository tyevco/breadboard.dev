//! `vcd-check` (Phase 2 E4, ADR-016 D1 and G1): load a Breadboard `nets.vcd`
//! with `wellen` 0.25.7, the waveform library of the Surfer viewer, and check
//! that it reads the file as bb wrote it.
//!
//! ```text
//! vcd-check <nets.vcd> [--csv <nets.csv>]
//! ```
//!
//! For each file:
//! - the timescale is exactly 1 fs;
//! - every `$var` of the header is a variable in wellen's hierarchy, with
//!   nothing extra: `real 64` as `VarType::Real` with the Real encoding,
//!   `wire 1` as `VarType::Wire`, one bit;
//! - every variable has a value at time 0, and as many changes as the file
//!   has value-change lines for its identifier; the time table ends at the
//!   file's last `#` time;
//! - the `$comment` name map (`  <reference> = <net name>`) names declared
//!   variables only;
//! - with `--csv` (the run's `nets.csv`): its columns are exactly the
//!   `real` variables, through the name map, and at every CSV row the value
//!   wellen holds for each of them (its last change at or before the row's
//!   time) equals the CSV's value, as numbers.
//!
//! It prints one deterministic summary line per variable and exits non-zero
//! with `vcd-check: FAIL: ...` lines when a check fails. The header scan is
//! only the expected side: the subject is what wellen reads.
use std::collections::BTreeMap;
use std::process::ExitCode;

use wellen::{SignalEncoding, SignalValueRef, Timescale, TimescaleUnit, VarType};

struct Decl {
    kind: String,
    size: String,
    id: String,
    name: String,
}

struct Header {
    timescale: String,
    decls: Vec<Decl>,
    /// reference -> net name, from the `$comment` block.
    map: BTreeMap<String, String>,
    /// value-change lines per identifier, and the last `#` time.
    changes: BTreeMap<String, usize>,
    last_time: Option<u64>,
}

fn scan(text: &str) -> Result<Header, String> {
    let mut h = Header {
        timescale: String::new(),
        decls: Vec::new(),
        map: BTreeMap::new(),
        changes: BTreeMap::new(),
        last_time: None,
    };
    let mut lines = text.lines();
    let mut in_comment = false;
    for line in lines.by_ref() {
        let t = line.trim();
        if in_comment {
            if t == "$end" {
                in_comment = false;
            } else if let Some((r, n)) = t.split_once(" = ") {
                h.map.insert(r.to_string(), n.to_string());
            }
            continue;
        }
        if t == "$comment" {
            in_comment = true;
        } else if let Some(ts) = t.strip_prefix("$timescale ") {
            h.timescale = ts.trim_end_matches("$end").trim().to_string();
        } else if let Some(v) = t.strip_prefix("$var ") {
            let f: Vec<&str> = v.split_whitespace().collect();
            if f.len() != 5 || f[4] != "$end" {
                return Err(format!("unexpected $var line: {line}"));
            }
            h.decls.push(Decl {
                kind: f[0].into(),
                size: f[1].into(),
                id: f[2].into(),
                name: f[3].into(),
            });
        } else if t == "$enddefinitions $end" {
            break;
        }
    }
    for line in lines {
        let t = line.trim();
        if let Some(n) = t.strip_prefix('#') {
            h.last_time = Some(n.parse().map_err(|e| format!("time {t}: {e}"))?);
        } else if let Some(r) = t.strip_prefix('r') {
            let id = r
                .split_whitespace()
                .nth(1)
                .ok_or(format!("bad change {t}"))?;
            *h.changes.entry(id.to_string()).or_default() += 1;
        } else if !t.is_empty() && !t.starts_with('$') {
            *h.changes.entry(t[1..].to_string()).or_default() += 1;
        }
    }
    Ok(h)
}

fn value_text(v: &SignalValueRef) -> String {
    match v {
        SignalValueRef::Real(r) => format!("{r}"),
        SignalValueRef::String(s) => format!("str:{s}"),
        other => other.to_bit_string().unwrap_or_else(|| "?".into()),
    }
}

fn check_file(path: &str, csv: Option<&str>, fails: &mut Vec<String>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let head = scan(&text)?;
    let opts = wellen::LoadOptions {
        multi_thread: false,
        remove_scopes_with_empty_name: false,
    };
    let mut w = wellen::simple::read_with_options(path, &opts)
        .map_err(|e| format!("{path}: wellen: {e:?}"))?;
    let mut fail = |m: String| fails.push(format!("{path}: {m}"));

    let ts = w.hierarchy().timescale();
    println!("{path}: timescale {ts:?} (header `{}`)", head.timescale);
    if ts != Some(Timescale::new(1, TimescaleUnit::FemtoSeconds)) || head.timescale != "1 fs" {
        fail(format!(
            "timescale is not 1 fs: wellen {ts:?}, header `{}`",
            head.timescale
        ));
    }

    // full name -> (type, encoding, signal, leaf name)
    type VarInfo = (VarType, SignalEncoding, wellen::SignalRef, String);
    let vars: BTreeMap<String, VarInfo> = {
        let h = w.hierarchy();
        h.all_vars()
            .map(|r| {
                let v = &h[r];
                (
                    v.full_name(h),
                    (
                        v.var_type(),
                        v.signal_encoding(h),
                        v.signal_ref(),
                        v.name(h).to_string(),
                    ),
                )
            })
            .collect()
    };
    let refs: Vec<_> = vars.values().map(|v| v.2).collect();
    w.load_signals(&refs);
    let tt = w.time_table().to_vec();

    if vars.len() != head.decls.len() {
        fail(format!(
            "wellen has {} variables, the header declares {}",
            vars.len(),
            head.decls.len()
        ));
    }
    let mut by_name: BTreeMap<String, &Decl> = BTreeMap::new();
    for d in &head.decls {
        // bb writes one `board` scope.
        let full = format!("board.{}", d.name);
        by_name.insert(d.name.clone(), d);
        let Some((ty, enc, sig, leaf)) = vars.get(&full) else {
            fail(format!(
                "`{}` (declared `{} {}`) is missing in wellen's hierarchy",
                full, d.kind, d.size
            ));
            continue;
        };
        // A dotted net name (U10.P1.04) stays one variable in the board scope.
        if *leaf != d.name {
            fail(format!(
                "`{full}`: wellen names the variable `{leaf}`, not `{}`",
                d.name
            ));
        }
        match (d.kind.as_str(), d.size.as_str()) {
            ("real", "64") => {
                if *ty != VarType::Real || *enc != SignalEncoding::Real {
                    fail(format!(
                        "`{full}` is declared real but wellen reads {ty:?} / {enc:?}"
                    ));
                }
            }
            ("wire", "1") => {
                if *ty != VarType::Wire || *enc != SignalEncoding::BitVector(1) {
                    fail(format!(
                        "`{full}` is declared wire 1 but wellen reads {ty:?} / {enc:?}"
                    ));
                }
            }
            (k, s) => fail(format!(
                "`{full}`: bb writes only `real 64` and `wire 1`, not `{k} {s}`"
            )),
        }
        let s = w.get_signal(*sig).ok_or(format!("{full}: no signal"))?;
        let n = s.time_indices().len();
        let first = s.iter_changes().next().map(|(i, _)| tt[i as usize]);
        let last = s
            .iter_changes()
            .last()
            .map(|(i, v)| format!("t={} {}", tt[i as usize], value_text(&v)));
        println!(
            "  {full} [{ty:?} {enc:?}] changes={n} last={}",
            last.as_deref().unwrap_or("none")
        );
        if first != Some(0) {
            fail(format!(
                "`{full}` has no value at time 0 (first change {first:?})"
            ));
        }
        let want = head.changes.get(&d.id).copied().unwrap_or(0);
        if n != want {
            fail(format!(
                "`{full}`: wellen reads {n} changes, the file has {want} for `{}`",
                d.id
            ));
        }
    }
    if tt.last().copied() != head.last_time {
        fail(format!(
            "time table ends at {:?}, the file at {:?}",
            tt.last(),
            head.last_time
        ));
    }
    for (r, n) in &head.map {
        if !by_name.contains_key(r) {
            fail(format!(
                "the $comment map names `{r}` (= {n}), which is not declared"
            ));
        }
    }
    println!(
        "  {} variables, {} time points, name map {} entries",
        vars.len(),
        tt.len(),
        head.map.len()
    );

    if let Some(csv) = csv {
        let ctext = std::fs::read_to_string(csv).map_err(|e| format!("{csv}: {e}"))?;
        let mut rows = ctext.lines();
        let cols: Vec<&str> = rows
            .next()
            .ok_or(format!("{csv}: empty"))?
            .split(',')
            .collect();
        if cols.first() != Some(&"t_fs") {
            return Err(format!("{csv}: the first column is not t_fs"));
        }
        let inv: BTreeMap<&str, &str> = head
            .map
            .iter()
            .map(|(r, n)| (n.as_str(), r.as_str()))
            .collect();
        let reals: Vec<&str> = head
            .decls
            .iter()
            .filter(|d| d.kind == "real")
            .map(|d| d.name.as_str())
            .collect();
        let mut lanes = Vec::new();
        for c in &cols[1..] {
            let r = inv.get(c).copied().unwrap_or(c);
            match vars.get(&format!("board.{r}")) {
                Some((VarType::Real, _, sig, _)) => {
                    let s = w.get_signal(*sig).unwrap();
                    let ch: Vec<(u64, f64)> = s
                        .iter_changes()
                        .map(|(i, v)| match v {
                            SignalValueRef::Real(x) => (tt[i as usize], x),
                            _ => (tt[i as usize], f64::NAN),
                        })
                        .collect();
                    lanes.push((c.to_string(), ch, 0usize));
                }
                _ => fail(format!(
                    "CSV column `{c}` (VCD `{r}`) is not a real variable"
                )),
            }
        }
        if lanes.len() != reals.len() {
            fail(format!(
                "{} CSV columns, {} real variables",
                cols.len() - 1,
                reals.len()
            ));
        }
        let (mut n_rows, mut bad) = (0usize, 0usize);
        for row in rows {
            let f: Vec<&str> = row.split(',').collect();
            let t: u64 = f[0]
                .parse()
                .map_err(|e| format!("{csv}: t_fs {}: {e}", f[0]))?;
            n_rows += 1;
            for (k, (name, ch, pos)) in lanes.iter_mut().enumerate() {
                while *pos + 1 < ch.len() && ch[*pos + 1].0 <= t {
                    *pos += 1;
                }
                let want: f64 = f[k + 1]
                    .parse()
                    .map_err(|e| format!("{csv}: {}: {e}", f[k + 1]))?;
                let got = if ch.is_empty() || ch[*pos].0 > t {
                    f64::NAN
                } else {
                    ch[*pos].1
                };
                if got != want {
                    if bad < 5 {
                        fail(format!(
                            "`{name}` at t={t} fs: wellen reads {got}, the CSV has {want}"
                        ));
                    }
                    bad += 1;
                }
            }
        }
        if bad > 0 {
            fail(format!("{bad} CSV values differ from wellen's"));
        }
        println!(
            "  {csv}: {n_rows} rows x {} real nets equal to wellen's values",
            lanes.len()
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut files, mut csv) = (Vec::new(), None);
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--csv" && i + 1 < args.len() {
            csv = Some(args[i + 1].clone());
            i += 2;
        } else {
            files.push(args[i].clone());
            i += 1;
        }
    }
    if files.len() != 1 {
        eprintln!("usage: vcd-check <nets.vcd> [--csv <nets.csv>]");
        return ExitCode::from(2);
    }
    let mut fails = Vec::new();
    // wellen panics on some malformed files (a `real` value under a 1-bit
    // `wire`): that is a load failure too.
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check_file(&files[0], csv.as_deref(), &mut fails)
    }));
    match r {
        Ok(Ok(())) => {}
        Ok(Err(e)) => fails.push(e),
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            fails.push(format!("{}: wellen could not load it: {msg}", files[0]));
        }
    }
    for f in &fails {
        println!("vcd-check: FAIL: {f}");
    }
    if fails.is_empty() {
        println!("vcd-check: PASS: {}", files[0]);
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
