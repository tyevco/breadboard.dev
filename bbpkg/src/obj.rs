//! Strict field extraction from a TOML table: every key must be consumed,
//! so an unknown key is an error (ADR-006 D2: "unknown keys are an error in
//! v0"), and a reserved key is refused with its own message (D11).

use std::collections::BTreeSet;

use crate::toml::{Table, Value};

/// A table being read, with its path for messages (`[[analog]] #2`).
pub struct Obj<'a> {
    pub path: String,
    t: &'a Table,
    used: BTreeSet<&'a str>,
}

impl<'a> Obj<'a> {
    pub fn new(path: impl Into<String>, t: &'a Table) -> Obj<'a> {
        Obj {
            path: path.into(),
            t,
            used: BTreeSet::new(),
        }
    }

    fn at(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_string()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    pub fn has(&self, key: &str) -> bool {
        self.t.contains_key(key)
    }

    pub fn get(&mut self, key: &str) -> Option<&'a Value> {
        let (k, v) = self.t.get_key_value(key)?;
        self.used.insert(k.as_str());
        Some(v)
    }

    pub fn str_opt(&mut self, key: &str) -> Result<Option<String>, String> {
        match self.get(key) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(v) => Err(format!(
                "{}: expected a string, got {}",
                self.at(key),
                v.type_name()
            )),
        }
    }

    pub fn str_req(&mut self, key: &str) -> Result<String, String> {
        self.str_opt(key)?
            .ok_or_else(|| format!("{}: required", self.at(key)))
    }

    pub fn strs(&mut self, key: &str) -> Result<Vec<String>, String> {
        match self.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(a)) => a
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("{}[{i}]: expected a string", self.at(key)))
                })
                .collect(),
            Some(v) => Err(format!(
                "{}: expected an array of strings, got {}",
                self.at(key),
                v.type_name()
            )),
        }
    }

    pub fn table_opt(&mut self, key: &str) -> Result<Option<Obj<'a>>, String> {
        let at = self.at(key);
        match self.get(key) {
            None => Ok(None),
            Some(Value::Table(t)) => Ok(Some(Obj::new(at, t))),
            Some(v) => Err(format!("{at}: expected a table, got {}", v.type_name())),
        }
    }

    /// An array of tables (`[[key]]`).
    pub fn tables(&mut self, key: &str) -> Result<Vec<Obj<'a>>, String> {
        let at = self.at(key);
        match self.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Array(a)) => a
                .iter()
                .enumerate()
                .map(|(i, v)| match v {
                    Value::Table(t) => Ok(Obj::new(format!("{at}[{i}]"), t)),
                    _ => Err(format!("{at}[{i}]: expected a table")),
                })
                .collect(),
            Some(v) => Err(format!(
                "{at}: expected an array of tables, got {}",
                v.type_name()
            )),
        }
    }

    /// A table of strings (`params = { a = "1" }`), in key order.
    pub fn str_map(&mut self, key: &str) -> Result<Vec<(String, String)>, String> {
        let at = self.at(key);
        match self.get(key) {
            None => Ok(Vec::new()),
            Some(Value::Table(t)) => t
                .iter()
                .map(|(k, v)| match v {
                    Value::String(s) => Ok((k.clone(), s.clone())),
                    v => Err(format!(
                        "{at}.{k}: expected a string, got {}",
                        v.type_name()
                    )),
                })
                .collect(),
            Some(v) => Err(format!("{at}: expected a table, got {}", v.type_name())),
        }
    }

    /// Refuse reserved keys with the D11 message.
    pub fn reserved(&self, keys: &[&str], schema: &str) -> Result<(), String> {
        for k in keys {
            if self.t.contains_key(*k) {
                return Err(format!("{}: reserved for {schema}", self.at(k)));
            }
        }
        Ok(())
    }

    /// Every key must have been read.
    pub fn done(self) -> Result<(), String> {
        for k in self.t.keys() {
            if !self.used.contains(k.as_str()) {
                return Err(format!("{}: unknown key", self.at(k)));
            }
        }
        Ok(())
    }
}
