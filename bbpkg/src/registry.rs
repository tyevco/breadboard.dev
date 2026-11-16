//! Package registries (ADR-011, `spec/registry.md`): the static layout, the
//! signed snapshot, the index, the client cache and resolution.
//!
//! - **Layout** (D1): `config.json`, `index/<ns>/<name>.jsonl` (one canonical
//!   JSON line per version, append-only), `dl/<ns>/<name>/<vers>.tar`
//!   (deterministic archives, [`crate::archive`]), `snapshot.json` (serial
//!   and the sha256 of `config.json` and of every index file) and
//!   `snapshot.json.minisig`.
//! - **Trust** (D3): the snapshot's minisign signature is verified against
//!   the registry's key (compiled into `bb` for the public registry, from
//!   `bb.toml` for a private one); every other file is checked against the
//!   snapshot, and every archive's tree hash is recomputed after extraction.
//! - **Rollback** (D2): the cache keeps the highest serial seen per registry
//!   and refuses a lower one.
//! - **Transport** (D4): a `file://` registry is read directly; an
//!   `https://` one through `curl --proto =https`. Nothing else is fetched.
//! - **Cache:** `$BB_REGISTRY_CACHE`, else `$BB_CACHE_DIR/registry`, else
//!   `$XDG_CACHE_HOME/bb/registry`, else `~/.cache/bb/registry`; one
//!   directory per registry ([`cache_key`]) with the verified snapshot,
//!   config and index files, the highest serial and the extracted trees
//!   (`trees/<hex>`, keyed by tree hash).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bbcore::cjson::Json;

use crate::lock::LockedPackage;
use crate::package::{self, Package};
use crate::schema::Registry as Schemas;
use crate::semver::{Req, Version};
use crate::sha256;

/// The registry format this build reads (`config.json` `format`).
pub const FORMAT: i128 = 1;
/// The size cap of one archive (ADR-011 D8: 2 MB).
pub const MAX_ARCHIVE: u64 = 2_000_000;

// --- formats -----------------------------------------------------------------

fn obj<'a>(j: &'a Json, what: &str) -> Result<&'a [(String, Json)], String> {
    j.as_obj()
        .ok_or_else(|| format!("{what}: expected an object"))
}

fn keys_only(o: &[(String, Json)], allowed: &[&str], what: &str) -> Result<(), String> {
    for (k, _) in o {
        if !allowed.contains(&k.as_str()) {
            return Err(format!(
                "{what}: unknown key {k:?} (a newer registry format: update bb, ADR-011 D12)"
            ));
        }
    }
    Ok(())
}

fn get<'a>(o: &'a [(String, Json)], k: &str, what: &str) -> Result<&'a Json, String> {
    o.iter()
        .find(|(x, _)| x == k)
        .map(|(_, v)| v)
        .ok_or_else(|| format!("{what}: missing {k:?}"))
}

fn get_str(o: &[(String, Json)], k: &str, what: &str) -> Result<String, String> {
    get(o, k, what)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{what}.{k}: expected a string"))
}

fn opt_str(o: &[(String, Json)], k: &str, what: &str) -> Result<Option<String>, String> {
    match o.iter().find(|(x, _)| x == k).map(|(_, v)| v) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{what}.{k}: expected a string or null")),
    }
}

fn get_int(o: &[(String, Json)], k: &str, what: &str) -> Result<i128, String> {
    get(o, k, what)?
        .as_int()
        .ok_or_else(|| format!("{what}.{k}: expected an integer"))
}

fn is_hash(s: &str) -> bool {
    s.strip_prefix("sha256:")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// `config.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The archive URL template: `{id}` and `{vers}` are substituted. A
    /// template without `://` is relative to the registry's URL.
    pub dl: String,
    /// The registry's minisign public keys (informative: the trusted key is
    /// the one `bb` or `bb.toml` holds; a rotation lists both).
    pub keys: Vec<String>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let j = Json::parse(text).map_err(|e| format!("config.json: {e}"))?;
        let o = obj(&j, "config.json")?;
        keys_only(o, &["dl", "format", "keys"], "config.json")?;
        let format = get_int(o, "format", "config.json")?;
        if format != FORMAT {
            return Err(format!(
                "config.json: registry format {format}; this bb reads format {FORMAT}: update bb \
                 (ADR-011 D12)"
            ));
        }
        let dl = get_str(o, "dl", "config.json")?;
        if !dl.contains("{id}") || !dl.contains("{vers}") {
            return Err("config.json: dl must contain {id} and {vers}".into());
        }
        let keys = get(o, "keys", "config.json")?
            .as_arr()
            .ok_or("config.json.keys: expected an array")?
            .iter()
            .map(|k| {
                k.as_str()
                    .map(str::to_string)
                    .ok_or("config.json.keys: strings")
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Config { dl, keys })
    }

    pub fn to_json(&self) -> Json {
        Json::obj([
            ("dl", Json::str(&self.dl)),
            ("format", Json::Int(FORMAT)),
            ("keys", Json::Arr(self.keys.iter().map(Json::str).collect())),
        ])
    }
}

/// `snapshot.json`: the serial and the sha256 of every file it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub serial: u64,
    pub files: BTreeMap<String, String>,
}

impl Snapshot {
    pub fn parse(text: &str) -> Result<Snapshot, String> {
        let j = Json::parse(text).map_err(|e| format!("snapshot.json: {e}"))?;
        if j.to_canonical() != text {
            return Err("snapshot.json: not in canonical form (spec/canonical-json.md)".into());
        }
        let o = obj(&j, "snapshot.json")?;
        keys_only(o, &["files", "serial"], "snapshot.json")?;
        let serial = u64::try_from(get_int(o, "serial", "snapshot.json")?)
            .map_err(|_| "snapshot.json.serial: expected a non-negative integer")?;
        let mut files = BTreeMap::new();
        for (k, v) in obj(get(o, "files", "snapshot.json")?, "snapshot.json.files")? {
            let h = v
                .as_str()
                .filter(|h| is_hash(h))
                .ok_or_else(|| format!("snapshot.json.files.{k}: expected sha256:<hex>"))?;
            if k != "config.json"
                && !(k.starts_with("index/") && k.ends_with(".jsonl") && !k.contains(".."))
            {
                return Err(format!("snapshot.json.files: {k:?} is not a registry file"));
            }
            files.insert(k.clone(), h.to_string());
        }
        Ok(Snapshot { serial, files })
    }

    pub fn to_text(&self) -> String {
        Json::obj([
            (
                "files",
                Json::Obj(
                    self.files
                        .iter()
                        .map(|(k, v)| (k.clone(), Json::str(v)))
                        .collect(),
                ),
            ),
            ("serial", Json::Int(self.serial as i128)),
        ])
        .to_canonical()
    }
}

/// A dependency of an index line (a module on its chip).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dep {
    pub id: String,
    pub req: String,
}

/// A review recorded by the registry's CI (ADR-011 D1, ADR-012).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub login: String,
    pub review: i128,
}

/// The index line's provenance summary, written by the registry's CI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexProvenance {
    pub source: Option<String>,
    pub confidence: Option<String>,
    pub reviewed_by: Vec<Review>,
    pub generated_by: Option<String>,
    /// From the claim checker (ADR-012, milestone E2); `null` until it runs.
    pub unverified_claims: Option<i128>,
}

/// One version of one package in the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexLine {
    pub id: String,
    pub vers: String,
    pub tree: String,
    pub size: u64,
    pub license: String,
    pub schema: String,
    pub deps: Vec<Dep>,
    pub provenance: IndexProvenance,
    pub platforms: Vec<String>,
    pub yanked: bool,
    pub pubtime: String,
    /// Legal removal (D2): the archive is deleted and the reason recorded.
    pub removed: Option<String>,
}

impl IndexLine {
    pub fn parse(text: &str) -> Result<IndexLine, String> {
        let what = "index line";
        let j = Json::parse(text).map_err(|e| format!("{what}: {e}"))?;
        if j.to_canonical() != format!("{text}\n") {
            return Err(format!(
                "{what}: not in canonical form (spec/canonical-json.md)"
            ));
        }
        let o = obj(&j, what)?;
        keys_only(
            o,
            &[
                "deps",
                "id",
                "license",
                "platforms",
                "provenance",
                "pubtime",
                "removed",
                "schema",
                "size",
                "tree",
                "vers",
                "yanked",
            ],
            what,
        )?;
        let id = get_str(o, "id", what)?;
        let vers = get_str(o, "vers", what)?;
        Version::parse(&vers).map_err(|e| format!("{what} {id}: {e}"))?;
        let tree = get_str(o, "tree", what)?;
        if !is_hash(&tree) {
            return Err(format!("{what} {id} {vers}: tree is not sha256:<hex>"));
        }
        let size = u64::try_from(get_int(o, "size", what)?)
            .map_err(|_| format!("{what} {id} {vers}: size"))?;
        let deps = get(o, "deps", what)?
            .as_arr()
            .ok_or_else(|| format!("{what} {id}: deps: expected an array"))?
            .iter()
            .map(|d| {
                let o = obj(d, "deps[]")?;
                keys_only(o, &["id", "req"], "deps[]")?;
                let dep = Dep {
                    id: get_str(o, "id", "deps[]")?,
                    req: get_str(o, "req", "deps[]")?,
                };
                Req::parse(&dep.req)?;
                Ok(dep)
            })
            .collect::<Result<Vec<_>, String>>()?;
        let p = obj(get(o, "provenance", what)?, "provenance")?;
        keys_only(
            p,
            &[
                "confidence",
                "generated_by",
                "reviewed_by",
                "source",
                "unverified_claims",
            ],
            "provenance",
        )?;
        let reviewed_by = get(p, "reviewed_by", "provenance")?
            .as_arr()
            .ok_or("provenance.reviewed_by: expected an array")?
            .iter()
            .map(|r| {
                let o = obj(r, "reviewed_by[]")?;
                keys_only(o, &["login", "review"], "reviewed_by[]")?;
                Ok(Review {
                    login: get_str(o, "login", "reviewed_by[]")?,
                    review: get_int(o, "review", "reviewed_by[]")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let unverified_claims = match get(p, "unverified_claims", "provenance")? {
            Json::Null => None,
            Json::Int(n) => Some(*n),
            _ => return Err("provenance.unverified_claims: an integer or null".into()),
        };
        let provenance = IndexProvenance {
            source: opt_str(p, "source", "provenance")?,
            confidence: opt_str(p, "confidence", "provenance")?,
            reviewed_by,
            generated_by: opt_str(p, "generated_by", "provenance")?,
            unverified_claims,
        };
        let platforms = get(o, "platforms", what)?
            .as_arr()
            .ok_or("platforms: expected an array")?
            .iter()
            .map(|x| x.as_str().map(str::to_string).ok_or("platforms: strings"))
            .collect::<Result<Vec<_>, _>>()?;
        let yanked = get(o, "yanked", what)?
            .as_bool()
            .ok_or("yanked: expected a boolean")?;
        Ok(IndexLine {
            license: get_str(o, "license", what)?,
            schema: get_str(o, "schema", what)?,
            pubtime: get_str(o, "pubtime", what)?,
            removed: opt_str(o, "removed", what)?,
            id,
            vers,
            tree,
            size,
            deps,
            provenance,
            platforms,
            yanked,
        })
    }

    /// The canonical line, without its `\n`.
    pub fn to_line(&self) -> String {
        let s = |o: &Option<String>| o.as_ref().map(Json::str).unwrap_or(Json::Null);
        let p = &self.provenance;
        let mut members: Vec<(String, Json)> = vec![
            (
                "deps".into(),
                Json::Arr(
                    self.deps
                        .iter()
                        .map(|d| Json::obj([("id", Json::str(&d.id)), ("req", Json::str(&d.req))]))
                        .collect(),
                ),
            ),
            ("id".into(), Json::str(&self.id)),
            ("license".into(), Json::str(&self.license)),
            (
                "platforms".into(),
                Json::Arr(self.platforms.iter().map(Json::str).collect()),
            ),
            (
                "provenance".into(),
                Json::obj([
                    ("confidence", s(&p.confidence)),
                    ("generated_by", s(&p.generated_by)),
                    (
                        "reviewed_by",
                        Json::Arr(
                            p.reviewed_by
                                .iter()
                                .map(|r| {
                                    Json::obj([
                                        ("login", Json::str(&r.login)),
                                        ("review", Json::Int(r.review)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    ("source", s(&p.source)),
                    (
                        "unverified_claims",
                        p.unverified_claims.map(Json::Int).unwrap_or(Json::Null),
                    ),
                ]),
            ),
            ("pubtime".into(), Json::str(&self.pubtime)),
        ];
        if let Some(r) = &self.removed {
            members.push(("removed".into(), Json::str(r)));
        }
        members.extend([
            ("schema".into(), Json::str(&self.schema)),
            ("size".into(), Json::Int(self.size as i128)),
            ("tree".into(), Json::str(&self.tree)),
            ("vers".into(), Json::str(&self.vers)),
            ("yanked".into(), Json::Bool(self.yanked)),
        ]);
        let t = Json::Obj(members).to_canonical();
        t.trim_end_matches('\n').to_string()
    }

    pub fn version(&self) -> Version {
        Version::parse(&self.vers).expect("checked at parse")
    }

    /// Usable by a new resolution: neither yanked nor removed.
    pub fn available(&self) -> bool {
        !self.yanked && self.removed.is_none()
    }
}

/// Parse one index file: every line canonical, one id, each version once.
pub fn parse_index(id: &str, text: &str) -> Result<Vec<IndexLine>, String> {
    let mut out: Vec<IndexLine> = Vec::new();
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(format!("index/{id}.jsonl: the last line has no newline"));
    }
    for (n, l) in text.lines().enumerate() {
        let line = IndexLine::parse(l).map_err(|e| format!("index/{id}.jsonl:{}: {e}", n + 1))?;
        if line.id != id {
            return Err(format!(
                "index/{id}.jsonl:{}: a line for {}",
                n + 1,
                line.id
            ));
        }
        if out.iter().any(|x| x.vers == line.vers) {
            return Err(format!("index/{id}.jsonl: version {} twice", line.vers));
        }
        out.push(line);
    }
    Ok(out)
}

// --- keys and signatures -------------------------------------------------------

/// Check a minisign public key (the base64 line, `RW...`).
pub fn check_key(k: &str) -> Result<(), String> {
    minisign_verify::PublicKey::from_base64(k)
        .map(|_| ())
        .map_err(|e| format!("minisign public key {k:?}: {e}"))
}

/// Verify `data` against a minisign signature file's text with any of
/// `keys`. Legacy (non-prehashed) signatures are refused.
pub fn verify(data: &[u8], sig_text: &str, keys: &[String]) -> Result<(), String> {
    let sig = minisign_verify::Signature::decode(sig_text)
        .map_err(|e| format!("snapshot.json.minisig: {e}"))?;
    let mut last = String::from("no key");
    for k in keys {
        let pk = minisign_verify::PublicKey::from_base64(k)
            .map_err(|e| format!("minisign public key {k:?}: {e}"))?;
        match pk.verify(data, &sig, false) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!(
        "snapshot.json: the signature does not verify with the registry's key ({last}); \
         refusing the registry (ADR-011 D3)"
    ))
}

// --- transport and cache ---------------------------------------------------------

/// `<url>` with exactly one trailing `/`.
pub fn normalize_url(url: &str) -> String {
    format!("{}/", url.trim_end_matches('/'))
}

/// Check a registry URL: `https://` or `file://` (ADR-011 D4).
pub fn check_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") || url.starts_with("file:///") {
        Ok(())
    } else {
        Err(format!(
            "registry url {url:?}: https:// or file:///<absolute path> (ADR-011 D4)"
        ))
    }
}

/// Read a `file://` URL or fetch an `https://` one with curl. `Ok(None)`:
/// the file does not exist (a 404 or a missing file).
pub fn fetch(url: &str) -> Result<Option<Vec<u8>>, String> {
    if let Some(p) = url.strip_prefix("file://") {
        if p.contains('%') {
            return Err(format!(
                "{url}: percent-escapes are not supported in a file:// url"
            ));
        }
        return match std::fs::read(p) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read {p}: {e}")),
        };
    }
    if !url.starts_with("https://") {
        return Err(format!(
            "{url}: bb fetches only https:// and file:// urls (ADR-011 D4)"
        ));
    }
    let out = std::process::Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--location",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "20",
            "--max-time",
            "300",
            "--write-out",
            "\n%{http_code}",
            "--output",
            "-",
            url,
        ])
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "curl not found: bb open fetches https:// registries and vendor files with curl \
                 (ADR-011 D4); install curl, or use bb open --offline with a warm cache"
                    .to_string()
            } else {
                format!("curl: {e}")
            }
        })?;
    let mut body = out.stdout;
    let nl = body.iter().rposition(|&b| b == b'\n');
    let code = nl
        .map(|i| String::from_utf8_lossy(&body[i + 1..]).trim().to_string())
        .unwrap_or_default();
    if let Some(i) = nl {
        body.truncate(i);
    }
    if !out.status.success() {
        return Err(format!(
            "fetch {url}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    match code.as_str() {
        "200" => Ok(Some(body)),
        "404" | "410" => Ok(None),
        c => Err(format!("fetch {url}: HTTP {c}")),
    }
}

/// The registry cache root (see the module documentation).
pub fn cache_root() -> Result<PathBuf, String> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    env("BB_REGISTRY_CACHE")
        .or_else(|| env("BB_CACHE_DIR").map(|d| d.join("registry")))
        .or_else(|| env("XDG_CACHE_HOME").map(|d| d.join("bb/registry")))
        .or_else(|| env("HOME").map(|d| d.join(".cache/bb/registry")))
        .ok_or_else(|| "no cache directory for the registry (set BB_REGISTRY_CACHE or HOME)".into())
}

/// A registry's cache directory name: its URL without the scheme, every
/// byte outside `[A-Za-z0-9._-]` as `_` (`https://a.io/r/` -> `a.io_r`;
/// `file:///srv/reg/` -> `file_srv_reg`).
pub fn cache_key(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let mut s: String = if scheme == "file" {
        format!("file{rest}")
    } else {
        rest.to_string()
    };
    s = s
        .trim_end_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    s
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

fn hex_of(tree: &str) -> &str {
    tree.strip_prefix("sha256:").unwrap_or(tree)
}

/// A registry as the client knows it: a name for messages (`public`, or the
/// `bb.toml` `[registries.<name>]` name), its URL and its trusted keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
    pub keys: Vec<String>,
}

/// An extracted tree in the cache, its tree hash recomputed (`None`: not
/// cached, or the cached copy no longer verifies and was set aside).
pub fn cached_tree(root: &Path, url: &str, tree: &str) -> Result<Option<PathBuf>, String> {
    let d = root.join(cache_key(url)).join("trees").join(hex_of(tree));
    if !d.join("part.toml").is_file() {
        return Ok(None);
    }
    if crate::lock::tree_hash(&d)? == tree {
        Ok(Some(d))
    } else {
        Ok(None)
    }
}

/// The cached index line of `id` `vers` (as last verified), for the yank
/// warning of `bb run`, which never fetches.
pub fn cached_line(root: &Path, url: &str, id: &str, vers: &str) -> Option<IndexLine> {
    let p = root
        .join(cache_key(url))
        .join("index")
        .join(format!("{id}.jsonl"));
    let text = std::fs::read_to_string(p).ok()?;
    parse_index(id, &text)
        .ok()?
        .into_iter()
        .find(|l| l.vers == vers)
}

/// One registry, its snapshot verified, for the length of one `bb open`.
pub struct Session {
    pub remote: Remote,
    dir: PathBuf,
    offline: bool,
    pub snapshot: Snapshot,
    pub config: Config,
    index: BTreeMap<String, Option<Vec<IndexLine>>>,
}

impl Session {
    /// Verify the registry's snapshot (online: fetch it; offline: the
    /// cached one), check the serial against the highest seen and read its
    /// config.
    pub fn open(remote: &Remote, root: &Path, offline: bool) -> Result<Session, String> {
        check_url(&remote.url)?;
        let url = normalize_url(&remote.url);
        let dir = root.join(cache_key(&url));
        let what = format!("registry {} ({url})", remote.name);
        let (snap_bytes, sig_bytes) = if offline {
            let s = std::fs::read(dir.join("snapshot.json"));
            let g = std::fs::read(dir.join("snapshot.json.minisig"));
            match (s, g) {
                (Ok(s), Ok(g)) => (s, g),
                _ => {
                    return Err(format!(
                        "{what}: not in the cache, and bb open --offline fetches nothing; run \
                         bb open without --offline once"
                    ))
                }
            }
        } else {
            let s = fetch(&format!("{url}snapshot.json"))
                .map_err(|e| format!("{what}: {e}"))?
                .ok_or_else(|| format!("{what}: no snapshot.json (not a registry?)"))?;
            let g = fetch(&format!("{url}snapshot.json.minisig"))
                .map_err(|e| format!("{what}: {e}"))?
                .ok_or_else(|| {
                    format!("{what}: no snapshot.json.minisig: an unsigned registry is refused (ADR-011 D3)")
                })?;
            (s, g)
        };
        let sig_text = String::from_utf8(sig_bytes.clone())
            .map_err(|_| format!("{what}: snapshot.json.minisig is not text"))?;
        verify(&snap_bytes, &sig_text, &remote.keys).map_err(|e| format!("{what}: {e}"))?;
        let snap_text = String::from_utf8(snap_bytes.clone())
            .map_err(|_| format!("{what}: snapshot.json is not UTF-8"))?;
        let snapshot = Snapshot::parse(&snap_text).map_err(|e| format!("{what}: {e}"))?;
        // Rollback protection (D2): never accept a lower serial than seen.
        let serial_file = dir.join("serial");
        let seen: Option<u64> = std::fs::read_to_string(&serial_file)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        if let Some(seen) = seen {
            if snapshot.serial < seen {
                return Err(format!(
                    "{what}: snapshot serial {} is lower than the highest seen, {seen}: a \
                     rolled-back registry is refused (ADR-011 D2)",
                    snapshot.serial
                ));
            }
            if snapshot.serial == seen {
                if let Ok(old) = std::fs::read(dir.join("snapshot.json")) {
                    if old != snap_bytes {
                        return Err(format!(
                            "{what}: two different snapshots carry serial {seen}: refused \
                             (ADR-011 D2)"
                        ));
                    }
                }
            }
        }
        if !offline {
            write_atomic(&dir.join("snapshot.json"), &snap_bytes)?;
            write_atomic(&dir.join("snapshot.json.minisig"), &sig_bytes)?;
            if seen.is_none_or(|s| snapshot.serial > s) {
                write_atomic(&serial_file, format!("{}\n", snapshot.serial).as_bytes())?;
            }
        }
        let mut s = Session {
            remote: Remote {
                url,
                ..remote.clone()
            },
            dir,
            offline,
            snapshot,
            config: Config {
                dl: String::new(),
                keys: Vec::new(),
            },
            index: BTreeMap::new(),
        };
        let c = s
            .file("config.json")?
            .ok_or_else(|| format!("{what}: the snapshot does not cover config.json"))?;
        s.config =
            Config::parse(&String::from_utf8_lossy(&c)).map_err(|e| format!("{what}: {e}"))?;
        Ok(s)
    }

    fn what(&self) -> String {
        format!("registry {} ({})", self.remote.name, self.remote.url)
    }

    /// A file the snapshot covers, checked against its sha256 (`None`: the
    /// snapshot does not list it).
    fn file(&self, rel: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(want) = self.snapshot.files.get(rel) else {
            return Ok(None);
        };
        let cached = self.dir.join("files").join(rel);
        let bytes = match std::fs::read(&cached) {
            Ok(b) if format!("sha256:{}", sha256::hex_digest(&b)) == *want => b,
            _ if self.offline => {
                return Err(format!(
                    "{}: {rel} is not in the cache (bb open --offline fetches nothing)",
                    self.what()
                ))
            }
            _ => {
                let b = fetch(&format!("{}{rel}", self.remote.url))
                    .map_err(|e| format!("{}: {e}", self.what()))?
                    .ok_or_else(|| {
                        format!(
                            "{}: {rel} is listed in the snapshot but missing",
                            self.what()
                        )
                    })?;
                let got = format!("sha256:{}", sha256::hex_digest(&b));
                if got != *want {
                    return Err(format!(
                        "{}: {rel} has {got}, the signed snapshot says {want}: a modified file \
                         is refused (ADR-011 D3)",
                        self.what()
                    ));
                }
                write_atomic(&cached, &b)?;
                b
            }
        };
        Ok(Some(bytes))
    }

    /// The index lines of `id` (`None`: not on this registry).
    pub fn lines(&mut self, id: &str) -> Result<Option<Vec<IndexLine>>, String> {
        if let Some(l) = self.index.get(id) {
            return Ok(l.clone());
        }
        let rel = format!("index/{id}.jsonl");
        let lines = match self.file(&rel)? {
            None => None,
            Some(b) => {
                let text = String::from_utf8(b).map_err(|_| format!("{rel}: not UTF-8"))?;
                let l = parse_index(id, &text).map_err(|e| format!("{}: {e}", self.what()))?;
                // The run-time yank check reads the verified copy.
                write_atomic(
                    &self.dir.join("index").join(format!("{id}.jsonl")),
                    text.as_bytes(),
                )?;
                Some(l)
            }
        };
        self.index.insert(id.to_string(), lines.clone());
        Ok(lines)
    }

    /// The archive URL of a line.
    pub fn archive_url(&self, line: &IndexLine) -> String {
        let t = self
            .config
            .dl
            .replace("{id}", &line.id)
            .replace("{vers}", &line.vers);
        if t.contains("://") {
            t
        } else {
            format!("{}{}", self.remote.url, t.trim_start_matches('/'))
        }
    }

    /// The extracted tree of a line, fetched and verified if not cached.
    pub fn tree(&self, line: &IndexLine) -> Result<PathBuf, String> {
        if let Some(d) = cached_tree(
            self.dir.parent().expect("root"),
            &self.remote.url,
            &line.tree,
        )? {
            return Ok(d);
        }
        let what = format!("{}: {} {}", self.what(), line.id, line.vers);
        if let Some(r) = &line.removed {
            return Err(format!(
                "{what} was removed ({r}) and no verified copy is in the cache (ADR-011 D2)"
            ));
        }
        if self.offline {
            return Err(format!(
                "{what}: not in the cache (bb open --offline fetches nothing)"
            ));
        }
        let url = self.archive_url(line);
        if url.starts_with("http://") {
            return Err(format!(
                "{what}: {url}: bb never fetches http:// (ADR-011 D4)"
            ));
        }
        let bytes = fetch(&url)
            .map_err(|e| format!("{what}: {e}"))?
            .ok_or_else(|| format!("{what}: {url} is missing"))?;
        if bytes.len() as u64 != line.size {
            return Err(format!(
                "{what}: the archive has {} bytes, the index says {}: refused (ADR-011 D2)",
                bytes.len(),
                line.size
            ));
        }
        let trees = self.dir.join("trees");
        std::fs::create_dir_all(&trees).map_err(|e| format!("create {}: {e}", trees.display()))?;
        let tmp = trees.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            hex_of(&line.tree)
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        crate::archive::extract(&bytes, &tmp).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp);
            format!("{what}: {e}")
        })?;
        let got = crate::lock::tree_hash(&tmp)?;
        if got != line.tree {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!(
                "{what}: the extracted tree hashes to {got}, the index says {}: a tampered \
                 archive is refused (ADR-011 D2)",
                line.tree
            ));
        }
        let dest = trees.join(hex_of(&line.tree));
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp, &dest).map_err(|e| format!("rename {}: {e}", dest.display()))?;
        Ok(dest)
    }
}

// --- resolution -------------------------------------------------------------------

/// How a resolver may reach registries.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// `bb open`: fetch what is missing.
    Online,
    /// `bb open --offline`: the cache only.
    Offline,
    /// `bb run`: only the trees the lock names, from the cache; never a
    /// snapshot, never the network.
    Locked,
}

/// Resolves the package ids no local source provides (ADR-011 D4).
pub struct Resolver {
    /// `[registries.<name>]`: each with the namespaces routed to it.
    pub routed: Vec<(Remote, Vec<String>)>,
    /// The public registry, for every unrouted namespace (`None`: none).
    pub public: Option<Remote>,
    pub access: Access,
    root: PathBuf,
    sessions: BTreeMap<String, Session>,
    /// Warnings for the caller to print (yanked versions kept by a lock).
    pub warnings: Vec<String>,
    /// Every tree this resolver supplied: canonical directory -> source.
    sources: BTreeMap<PathBuf, String>,
    /// Requirements from the index lines' deps (a module's chip).
    reqs: BTreeMap<String, String>,
}

impl Resolver {
    pub fn new(
        routed: Vec<(Remote, Vec<String>)>,
        public: Option<Remote>,
        access: Access,
    ) -> Result<Resolver, String> {
        Ok(Resolver {
            routed,
            public,
            access,
            root: cache_root()?,
            sessions: BTreeMap::new(),
            warnings: Vec::new(),
            sources: BTreeMap::new(),
            reqs: BTreeMap::new(),
        })
    }

    /// The registry a package id resolves on (D4): its namespace's routed
    /// registry, else the public one. A routed namespace never falls
    /// through to another registry.
    pub fn remote_for(&self, id: &str) -> Result<(Remote, bool), String> {
        let ns = id.split('/').next().unwrap_or("");
        if let Some((r, _)) = self.routed.iter().find(|(_, n)| n.iter().any(|x| x == ns)) {
            return Ok((r.clone(), true));
        }
        self.public
            .clone()
            .map(|r| (r, false))
            .ok_or_else(|| format!("package {id}: no registry is configured for namespace {ns}"))
    }

    /// The source a resolved package directory came from (`None`: local).
    pub fn source_of(&self, dir: &Path) -> Option<String> {
        let c = std::fs::canonicalize(dir).ok()?;
        self.sources.get(&c).cloned()
    }

    fn session(&mut self, r: &Remote) -> Result<&mut Session, String> {
        let url = normalize_url(&r.url);
        if !self.sessions.contains_key(&url) {
            let s = Session::open(r, &self.root, self.access == Access::Offline)?;
            self.sessions.insert(url.clone(), s);
        }
        Ok(self.sessions.get_mut(&url).expect("inserted"))
    }

    fn load(
        &mut self,
        dir: PathBuf,
        id: &str,
        vers: &str,
        source: String,
        schemas: &Schemas,
    ) -> Result<Package, String> {
        let pkg =
            package::load(&dir, schemas).map_err(|e| format!("{id} {vers} ({source}): {e}"))?;
        if pkg.part.id != id || pkg.part.version != vers {
            return Err(format!(
                "{source}: the archive of {id} {vers} holds {} {}: refused",
                pkg.part.id, pkg.part.version
            ));
        }
        let c = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        self.sources.insert(c, source);
        Ok(pkg)
    }

    /// Resolve `spec` (`id` or `id@MAJOR.MINOR.PATCH`), which no local source
    /// provides. `locked` is the lock's entry for the id, if any: its version
    /// is kept (a yanked one with a warning, D2).
    pub fn resolve(
        &mut self,
        spec: &str,
        locked: Option<&LockedPackage>,
        schemas: &Schemas,
    ) -> Result<Package, String> {
        let (id, want) = match spec.split_once('@') {
            Some((i, v)) => (i, Some(v)),
            None => (spec, None),
        };
        // D10: ids are lower-case ASCII; nothing else reaches a URL or a
        // cache path.
        let seg = |s: &str| {
            !s.is_empty()
                && !s.starts_with('.')
                && s.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
                })
        };
        if !id.split_once('/').is_some_and(|(n, m)| seg(n) && seg(m)) {
            return Err(format!(
                "package {id:?}: not a package id (<namespace>/<name>, lower case [a-z0-9._-])"
            ));
        }
        if self.access == Access::Locked {
            let l = locked.filter(|l| l.source.starts_with(crate::lock::REGISTRY_SOURCE)).ok_or_else(|| {
                format!("package {id}: not on the search path, and bb.lock has no registry source for it: run `bb open` (only bb open fetches, ADR-011 D4)")
            })?;
            let url = l
                .source
                .strip_prefix(crate::lock::REGISTRY_SOURCE)
                .expect("checked")
                .to_string();
            let dir = cached_tree(&self.root, &url, &l.tree)?.ok_or_else(|| {
                format!(
                    "package {id} {} ({}): its tree {} is not in the registry cache: run `bb open` \
                     (bb run never fetches, ADR-011 D4)",
                    l.version, l.source, l.tree
                )
            })?;
            if let Some(line) = cached_line(&self.root, &url, id, &l.version) {
                if line.yanked {
                    self.warnings.push(format!(
                        "package {id} {} is yanked on {url}; the lock keeps it (ADR-011 D2)",
                        l.version
                    ));
                }
                if let Some(r) = &line.removed {
                    self.warnings.push(format!(
                        "package {id} {} was removed from {url} ({r}); the verified cached copy is used (ADR-011 D2)",
                        l.version
                    ));
                }
            }
            let (v, s) = (l.version.clone(), l.source.clone());
            return self.load(dir, id, &v, s, schemas);
        }
        let (remote, routed) = self.remote_for(id)?;
        let url = normalize_url(&remote.url);
        let ns = id.split('/').next().unwrap_or("").to_string();
        let lines = self.session(&remote)?.lines(id)?.ok_or_else(|| {
            let mut m = format!(
                "package {id} is not on the search path nor on registry {} ({url})",
                remote.name
            );
            if routed {
                m.push_str(&format!(
                    "; namespace {ns} is routed to it, so no other registry is asked (ADR-011 D4)"
                ));
            }
            m
        })?;
        let line = if let Some(l) = locked.filter(|l| want.is_none_or(|w| w == l.version)) {
            let line = lines
                .iter()
                .find(|x| x.vers == l.version)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "package {id}: bb.lock pins {}, which registry {} ({url}) does not have",
                        l.version, remote.name
                    )
                })?;
            if line.yanked {
                self.warnings.push(format!(
                    "package {id} {} is yanked on registry {} ({url}); bb.lock keeps it (a new resolve would skip it, ADR-011 D2)",
                    line.vers, remote.name
                ));
            }
            if let Some(r) = &line.removed {
                self.warnings.push(format!(
                    "package {id} {} was removed from registry {} ({r}); only a verified cached copy can be used (ADR-011 D2)",
                    line.vers, remote.name
                ));
            }
            line
        } else if let Some(w) = want {
            let line = lines.iter().find(|x| x.vers == w).cloned().ok_or_else(|| {
                format!(
                    "package {id}@{w}: registry {} ({url}) has no such version",
                    remote.name
                )
            })?;
            if !line.available() {
                return Err(format!(
                    "package {id}@{w} is {} on registry {}: a new resolve skips it (ADR-011 D2)",
                    if line.yanked { "yanked" } else { "removed" },
                    remote.name
                ));
            }
            line
        } else {
            let req = match self.reqs.get(id) {
                Some(r) => Some(Req::parse(r)?),
                None => None,
            };
            lines
                .iter()
                .filter(|x| x.available() && req.as_ref().is_none_or(|r| r.matches(&x.version())))
                .max_by_key(|x| x.version())
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "package {id}: no version{} on registry {} ({url}) that is not yanked or removed (ADR-011 D2)",
                        req.as_ref().map(|r| format!(" matching {}", r.as_str())).unwrap_or_default(),
                        remote.name
                    )
                })?
        };
        // D12: a major this bb does not know is refused with "update bb".
        if let Err(e) = schemas.check("bb-part", &line.schema) {
            return Err(format!(
                "package {id} {} on registry {} is {}, which this bb cannot read ({e}): update bb (ADR-011 D12)",
                line.vers, remote.name, line.schema
            ));
        }
        let dir = self.session(&remote)?.tree(&line)?;
        for d in &line.deps {
            self.reqs
                .entry(d.id.clone())
                .or_insert_with(|| d.req.clone());
        }
        self.load(
            dir,
            id,
            &line.vers,
            format!("{}{url}", crate::lock::REGISTRY_SOURCE),
            schemas,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> IndexLine {
        IndexLine {
            id: "ti/tps22917".into(),
            vers: "0.2.0".into(),
            tree: format!("sha256:{}", "a".repeat(64)),
            size: 104231,
            license: "Apache-2.0".into(),
            schema: "bb-part/1".into(),
            deps: vec![Dep {
                id: "x/y".into(),
                req: "^0.1".into(),
            }],
            provenance: IndexProvenance {
                source: Some("datasheet".into()),
                confidence: Some("datasheet_checked".into()),
                reviewed_by: vec![Review {
                    login: "someone".into(),
                    review: 123,
                }],
                generated_by: None,
                unverified_claims: Some(0),
            },
            platforms: vec![],
            yanked: false,
            pubtime: "2027-01-10".into(),
            removed: None,
        }
    }

    #[test]
    fn index_lines_round_trip_canonically() {
        let l = line();
        let t = l.to_line();
        assert!(
            t.starts_with("{\"deps\":[{\"id\":\"x/y\",\"req\":\"^0.1\"}],\"id\":\"ti/tps22917\""),
            "{t}"
        );
        assert_eq!(IndexLine::parse(&t).unwrap(), l);
        let mut r = l.clone();
        r.removed = Some("takedown".into());
        r.yanked = true;
        assert_eq!(IndexLine::parse(&r.to_line()).unwrap(), r);
        // Not canonical, unknown key, bad hash: refused.
        assert!(IndexLine::parse(&t.replacen("{", "{ ", 1)).is_err());
        assert!(
            IndexLine::parse(&t.replacen("\"deps\"", "\"aaa\":1,\"deps\"", 1))
                .unwrap_err()
                .contains("update bb")
        );
        assert!(IndexLine::parse(&t.replace(&"a".repeat(64), "A")).is_err());
        let two = format!("{t}\n{t}\n");
        assert!(parse_index("ti/tps22917", &two)
            .unwrap_err()
            .contains("twice"));
        assert!(parse_index("ti/other", &format!("{t}\n")).is_err());
    }

    #[test]
    fn snapshots_and_config() {
        let mut files = BTreeMap::new();
        files.insert(
            "config.json".to_string(),
            format!("sha256:{}", "b".repeat(64)),
        );
        files.insert(
            "index/ti/tps22917.jsonl".to_string(),
            format!("sha256:{}", "c".repeat(64)),
        );
        let s = Snapshot { serial: 7, files };
        assert_eq!(Snapshot::parse(&s.to_text()).unwrap(), s);
        assert!(Snapshot::parse(&s.to_text().replace("index/ti", "dl/ti")).is_err());
        let c = Config {
            dl: "dl/{id}/{vers}.tar".into(),
            keys: vec!["RWQ".into()],
        };
        assert_eq!(Config::parse(&c.to_json().to_canonical()).unwrap(), c);
        assert!(Config::parse(
            &c.to_json()
                .to_canonical()
                .replace("\"format\":1", "\"format\":2")
        )
        .unwrap_err()
        .contains("update bb"));
    }

    #[test]
    fn cache_keys_and_urls() {
        assert_eq!(
            cache_key("https://a.github.io/bb-registry/"),
            "a.github.io_bb-registry"
        );
        assert_eq!(cache_key("file:///srv/reg/"), "file_srv_reg");
        assert_eq!(cache_key("https://127.0.0.1:8443/"), "127.0.0.1_8443");
        assert!(check_url("http://x/").is_err());
        assert!(check_url("file://relative").is_err());
        assert!(check_url("https://x/").is_ok());
        assert!(fetch("http://example.invalid/")
            .unwrap_err()
            .contains("only https"));
        assert_eq!(normalize_url("https://x"), "https://x/");
    }
}
