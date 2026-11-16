//! Deterministic package archives (ADR-011 D6, `spec/registry.md` section 4).
//!
//! An archive is a POSIX ustar: the files the tree hash covers (dotfiles and
//! `tests/out/` excluded, [`crate::lock::tree_files`]) and their parent
//! directories, entries sorted by path (a directory as `<path>/`), mode 0644
//! (0755 for a directory), uid and gid 0, empty user and group names, mtime
//! 0, no extended headers, then two zero blocks. Two packs of one tree are
//! byte-identical, and the tree hash of the extracted tree is the source's.
//!
//! The reader is strict: it accepts only that form's entry types (regular
//! files and directories), relative paths without `.`, `..` or empty
//! components, a valid header checksum and no entry twice. Ownership, mode
//! and mtime are not taken from the archive: an extracted file is written
//! as a fresh file, so only the paths and contents reach the tree hash.

use std::collections::BTreeSet;
use std::path::Path;

const BLOCK: usize = 512;

fn octal(field: &mut [u8], v: u64) -> Result<(), String> {
    // `field.len() - 1` octal digits, zero-padded, then a NUL.
    let digits = field.len() - 1;
    let s = format!("{v:0digits$o}");
    if s.len() > digits {
        return Err(format!(
            "value {v} does not fit a {}-byte ustar field",
            field.len()
        ));
    }
    field[..digits].copy_from_slice(s.as_bytes());
    field[digits] = 0;
    Ok(())
}

/// Split `path` into ustar's `prefix` (155 bytes) and `name` (100 bytes).
fn split_name(path: &str) -> Result<(&str, &str), String> {
    if path.len() <= 100 {
        return Ok(("", path));
    }
    // Split at a '/' so that the name fits and the prefix fits.
    for (i, b) in path.bytes().enumerate().rev() {
        if b == b'/' && path.len() - i - 1 <= 100 && i <= 155 && i > 0 {
            return Ok((&path[..i], &path[i + 1..]));
        }
    }
    Err(format!("{path}: too long for a ustar header (ADR-011 D6)"))
}

fn header(path: &str, size: u64, dir: bool) -> Result<[u8; BLOCK], String> {
    let mut h = [0u8; BLOCK];
    let (prefix, name) = split_name(path)?;
    h[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut h[100..108], if dir { 0o755 } else { 0o644 })?;
    octal(&mut h[108..116], 0)?; // uid
    octal(&mut h[116..124], 0)?; // gid
    octal(&mut h[124..136], size)?;
    octal(&mut h[136..148], 0)?; // mtime
    h[148..156].copy_from_slice(b"        "); // checksum, as spaces while summing
    h[156] = if dir { b'5' } else { b'0' };
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    // uname, gname empty; devmajor, devminor zero.
    octal(&mut h[329..337], 0)?;
    octal(&mut h[337..345], 0)?;
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let sum: u64 = h.iter().map(|&b| b as u64).sum();
    // Six octal digits, NUL, space (the common form).
    let s = format!("{sum:06o}");
    h[148..154].copy_from_slice(s.as_bytes());
    h[154] = 0;
    h[155] = b' ';
    Ok(h)
}

/// Pack a package directory into its deterministic archive.
pub fn pack(dir: &Path) -> Result<Vec<u8>, String> {
    let files = crate::lock::tree_files(dir)?;
    let mut entries: Vec<(String, Option<&Path>)> = Vec::new();
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for (rel, _) in &files {
        let mut acc = String::new();
        let parts: Vec<&str> = rel.split('/').collect();
        for p in &parts[..parts.len() - 1] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            dirs.insert(format!("{acc}/"));
        }
    }
    for d in &dirs {
        entries.push((d.clone(), None));
    }
    for (rel, abs) in &files {
        entries.push((rel.clone(), Some(abs.as_path())));
    }
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut out = Vec::new();
    for (path, abs) in entries {
        match abs {
            None => out.extend_from_slice(&header(&path, 0, true)?),
            Some(abs) => {
                let bytes =
                    std::fs::read(abs).map_err(|e| format!("read {}: {e}", abs.display()))?;
                out.extend_from_slice(&header(&path, bytes.len() as u64, false)?);
                out.extend_from_slice(&bytes);
                let pad = (BLOCK - bytes.len() % BLOCK) % BLOCK;
                out.extend(std::iter::repeat_n(0u8, pad));
            }
        }
    }
    out.extend(std::iter::repeat_n(0u8, 2 * BLOCK));
    Ok(out)
}

fn parse_octal(field: &[u8], what: &str) -> Result<u64, String> {
    let s: Vec<u8> = field
        .iter()
        .copied()
        .take_while(|&b| b != 0)
        .filter(|&b| b != b' ')
        .collect();
    if s.is_empty() {
        return Ok(0);
    }
    if !s.iter().all(|b| (b'0'..=b'7').contains(b)) {
        return Err(format!("archive: {what}: not an octal number"));
    }
    u64::from_str_radix(std::str::from_utf8(&s).expect("ascii"), 8)
        .map_err(|e| format!("archive: {what}: {e}"))
}

fn cstr(field: &[u8]) -> Result<&str, String> {
    let n = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    std::str::from_utf8(&field[..n]).map_err(|_| "archive: a path that is not UTF-8".to_string())
}

/// One entry of an archive: a directory (`content: None`) or a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub content: Option<Vec<u8>>,
}

/// Read an archive strictly (see the module documentation).
pub fn read(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut off = 0;
    loop {
        if off + BLOCK > bytes.len() {
            return Err("archive: truncated (no end-of-archive blocks)".into());
        }
        let h = &bytes[off..off + BLOCK];
        if h.iter().all(|&b| b == 0) {
            let next = bytes.get(off + BLOCK..off + 2 * BLOCK);
            if next.is_none_or(|n| n.iter().any(|&b| b != 0)) {
                return Err("archive: one zero block, then data".into());
            }
            if bytes[off + 2 * BLOCK..].iter().any(|&b| b != 0) {
                return Err("archive: data after the end-of-archive blocks".into());
            }
            break;
        }
        let want = parse_octal(&h[148..156], "checksum")?;
        let sum: u64 = h
            .iter()
            .enumerate()
            .map(|(i, &b)| {
                if (148..156).contains(&i) {
                    b' ' as u64
                } else {
                    b as u64
                }
            })
            .sum();
        if sum != want {
            return Err(format!("archive: header checksum mismatch at byte {off}"));
        }
        if &h[257..263] != b"ustar\0" {
            return Err("archive: not a POSIX ustar header (ADR-011 D6)".into());
        }
        let name = cstr(&h[0..100])?;
        let prefix = cstr(&h[345..500])?;
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        let size = parse_octal(&h[124..136], "size")?;
        let kind = h[156];
        let (is_dir, rel) = match kind {
            b'0' | 0 => (false, path.as_str()),
            b'5' => (true, path.strip_suffix('/').unwrap_or(&path)),
            k => {
                return Err(format!(
                    "archive: {path}: entry type {:?} is not allowed (only files and \
                     directories; no links, devices or extended headers, ADR-011 D6)",
                    k as char
                ))
            }
        };
        if rel.is_empty()
            || rel.starts_with('/')
            || rel.contains('\\')
            || rel
                .split('/')
                .any(|c| c.is_empty() || c == "." || c == "..")
        {
            return Err(format!("archive: {path:?}: not a plain relative path"));
        }
        if rel.split('/').any(|c| c.starts_with('.')) {
            return Err(format!(
                "archive: {path:?}: a dotfile (never in a package archive)"
            ));
        }
        if !seen.insert(rel.to_string()) {
            return Err(format!("archive: {rel} twice"));
        }
        off += BLOCK;
        if is_dir {
            if size != 0 {
                return Err(format!("archive: directory {rel} with a size"));
            }
            out.push(Entry {
                path: rel.to_string(),
                content: None,
            });
            continue;
        }
        let size = usize::try_from(size).map_err(|_| "archive: size overflow".to_string())?;
        let end = off
            .checked_add(size)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| format!("archive: {rel}: truncated content"))?;
        out.push(Entry {
            path: rel.to_string(),
            content: Some(bytes[off..end].to_vec()),
        });
        off = end + (BLOCK - size % BLOCK) % BLOCK;
    }
    // A file's parent must not be a file.
    let files: BTreeSet<&str> = out
        .iter()
        .filter(|e| e.content.is_some())
        .map(|e| e.path.as_str())
        .collect();
    for e in &out {
        let mut p = e.path.as_str();
        while let Some((parent, _)) = p.rsplit_once('/') {
            if files.contains(parent) {
                return Err(format!("archive: {} is inside the file {parent}", e.path));
            }
            p = parent;
        }
    }
    Ok(out)
}

/// Extract an archive into `dest`, which must not exist yet.
pub fn extract(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let entries = read(bytes)?;
    if dest.exists() {
        return Err(format!("{}: already exists", dest.display()));
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    for e in entries {
        let p = dest.join(&e.path);
        match e.content {
            None => std::fs::create_dir_all(&p),
            Some(c) => {
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d)
                        .map_err(|err| format!("create {}: {err}", d.display()))?;
                }
                std::fs::write(&p, c)
            }
        }
        .map_err(|err| format!("write {}: {err}", p.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("bbpkg-archive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn pack_is_deterministic_and_round_trips_the_tree_hash() {
        // G5: two packs byte-identical; the extracted tree's hash is the source's.
        let src = tmp("src");
        std::fs::create_dir_all(src.join("analog")).unwrap();
        std::fs::create_dir_all(src.join("tests/out")).unwrap();
        let long = "d".repeat(90);
        std::fs::create_dir_all(src.join(&long)).unwrap();
        std::fs::write(src.join("part.toml"), "a\n").unwrap();
        std::fs::write(src.join("analog/r.lib"), vec![7u8; 1300]).unwrap();
        std::fs::write(src.join(".hidden"), "x").unwrap();
        std::fs::write(src.join("tests/out/run.txt"), "x").unwrap();
        std::fs::write(src.join(format!("{long}/{}", "f".repeat(40))), "long\n").unwrap();
        let a = pack(&src).unwrap();
        let b = pack(&src).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len() % BLOCK, 0);
        let names: Vec<String> = read(&a).unwrap().into_iter().map(|e| e.path).collect();
        assert_eq!(
            names,
            vec![
                "analog".to_string(),
                "analog/r.lib".to_string(),
                long.clone(),
                format!("{long}/{}", "f".repeat(40)),
                "part.toml".to_string()
            ]
        );
        let dst = tmp("dst");
        extract(&a, &dst).unwrap();
        assert_eq!(
            crate::lock::tree_hash(&dst).unwrap(),
            crate::lock::tree_hash(&src).unwrap()
        );
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn the_reader_refuses_what_pack_never_writes() {
        let one = |path: &str, kind: u8| {
            let mut h = header(path, 0, false).unwrap();
            h[156] = kind;
            h[148..156].copy_from_slice(b"        ");
            let sum: u64 = h.iter().map(|&b| b as u64).sum();
            h[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
            h[154] = 0;
            let mut v = h.to_vec();
            v.extend(vec![0u8; 2 * BLOCK]);
            v
        };
        assert!(read(&one("ok.txt", b'0')).is_ok());
        for (p, k, want) in [
            ("l", b'2', "not allowed"),
            ("h", b'1', "not allowed"),
            ("x", b'x', "not allowed"),
            ("../evil", b'0', "relative path"),
            ("/abs", b'0', "relative path"),
            ("a//b", b'0', "relative path"),
            (".git/config", b'0', "dotfile"),
        ] {
            let e = read(&one(p, k)).unwrap_err();
            assert!(e.contains(want), "{p}: {e}");
        }
        // A corrupted header byte fails the checksum; a missing end, truncation.
        let mut v = one("ok.txt", b'0');
        v[3] ^= 1;
        assert!(read(&v).unwrap_err().contains("checksum"));
        let v = one("ok.txt", b'0');
        assert!(read(&v[..BLOCK]).unwrap_err().contains("truncated"));
        let mut v = one("ok.txt", b'0');
        v.extend(vec![1u8; 3]);
        assert!(read(&v).is_err());
    }
}
