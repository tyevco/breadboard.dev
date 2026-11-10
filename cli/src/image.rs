//! Firmware image identity (ADR-008 D5, Phase 1 M6).
//!
//! The hash of an image of kind `elf` is the sha256 of its loadable content,
//! not of the file: two builds of the same sources differ in their debug
//! information (the build path, ADR-008 Context 5) while the bytes the
//! simulator loads are the same. `bb flash` records it in `bb.toml`
//! `[targets]`, `bb run` checks it and copies it into `run.json`.
//!
//! The hashed stream (`spec/project.md` section 1) is, in this order:
//!
//! 1. the 15 bytes `bb-elf-image/0\n`;
//! 2. for each `PT_LOAD` program header with `p_filesz > 0`, sorted by
//!    `p_paddr` (then `p_offset`): `p_paddr` and `p_filesz` as u64
//!    little-endian, then the segment's `p_filesz` file bytes;
//! 3. the entry point `e_entry` as u64 little-endian.
//!
//! `p_paddr` is the load address (an initialised-data segment is loaded at
//! its flash address); the zero fill past `p_filesz` is implied by
//! `p_memsz` and is the program's to clear, so it is not part of the image.
//! ELF32 and ELF64, little- and big-endian, are read; anything else is an
//! error.

/// `sha256:<hex>` of the ELF image (module doc).
pub fn elf_image_hash(bytes: &[u8]) -> Result<String, String> {
    let segs = elf_load_segments(bytes)?;
    let mut stream = Vec::with_capacity(bytes.len() + 64);
    stream.extend_from_slice(b"bb-elf-image/0\n");
    for s in &segs.segments {
        stream.extend_from_slice(&s.paddr.to_le_bytes());
        stream.extend_from_slice(&(s.data.len() as u64).to_le_bytes());
        stream.extend_from_slice(s.data);
    }
    stream.extend_from_slice(&segs.entry.to_le_bytes());
    Ok(format!("sha256:{}", bbpkg::sha256::hex_digest(&stream)))
}

pub struct Segment<'a> {
    pub paddr: u64,
    pub data: &'a [u8],
}

pub struct LoadImage<'a> {
    pub entry: u64,
    pub segments: Vec<Segment<'a>>,
}

/// The `PT_LOAD` segments with file content, in load-address order.
pub fn elf_load_segments(b: &[u8]) -> Result<LoadImage<'_>, String> {
    if b.len() < 16 || !b.starts_with(b"\x7fELF") {
        return Err("not an ELF file".into());
    }
    let class64 = match b[4] {
        1 => false,
        2 => true,
        c => return Err(format!("ELF class {c} (expected 1 or 2)")),
    };
    let le = match b[5] {
        1 => true,
        2 => false,
        d => return Err(format!("ELF data encoding {d} (expected 1 or 2)")),
    };
    let rd = |off: usize, n: usize| -> Result<u64, String> {
        let s = b
            .get(off..off + n)
            .ok_or_else(|| format!("truncated ELF (reading {n} bytes at {off})"))?;
        let mut v: u64 = 0;
        for i in 0..n {
            let byte = if le { s[n - 1 - i] } else { s[i] };
            v = (v << 8) | u64::from(byte);
        }
        Ok(v)
    };
    let (entry, phoff, phentsize, phnum) = if class64 {
        (rd(24, 8)?, rd(32, 8)?, rd(54, 2)?, rd(56, 2)?)
    } else {
        (rd(24, 4)?, rd(28, 4)?, rd(42, 2)?, rd(44, 2)?)
    };
    let min_ent = if class64 { 56 } else { 32 };
    if phnum > 0 && phentsize < min_ent {
        return Err(format!(
            "ELF program header entries of {phentsize} bytes (at least {min_ent})"
        ));
    }
    let mut segments = Vec::new();
    let mut keyed = Vec::new();
    for k in 0..phnum {
        let h = usize::try_from(phoff + k * phentsize).map_err(|_| "ELF offset overflow")?;
        let p_type = rd(h, 4)?;
        if p_type != 1 {
            continue; // PT_LOAD only
        }
        let (offset, paddr, filesz) = if class64 {
            (rd(h + 8, 8)?, rd(h + 24, 8)?, rd(h + 32, 8)?)
        } else {
            (rd(h + 4, 4)?, rd(h + 12, 4)?, rd(h + 16, 4)?)
        };
        if filesz == 0 {
            continue;
        }
        let start = usize::try_from(offset).map_err(|_| "ELF offset overflow")?;
        let end = start
            .checked_add(usize::try_from(filesz).map_err(|_| "ELF size overflow")?)
            .ok_or("ELF size overflow")?;
        let data = b
            .get(start..end)
            .ok_or_else(|| format!("PT_LOAD segment at file offset {offset} runs past the file"))?;
        keyed.push((paddr, offset, data));
    }
    if keyed.is_empty() {
        return Err("the ELF file has no PT_LOAD segment with content".into());
    }
    keyed.sort_by_key(|&(p, o, _)| (p, o));
    for (paddr, _, data) in keyed {
        segments.push(Segment { paddr, data });
    }
    Ok(LoadImage { entry, segments })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A minimal little-endian ELF32 with the given PT_LOAD segments
    /// (`(paddr, bytes)`) and entry point, plus `junk` bytes after them that
    /// no segment covers (debug information, say).
    pub fn tiny_elf(segs: &[(u32, &[u8])], entry: u32, junk: &[u8]) -> Vec<u8> {
        let phoff = 52u32;
        let mut data_off = phoff + 32 * segs.len() as u32;
        let mut f = vec![0u8; 52];
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 1; // ELF32
        f[5] = 1; // little-endian
        f[6] = 1;
        f[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        f[18..20].copy_from_slice(&40u16.to_le_bytes()); // EM_ARM
        f[20..24].copy_from_slice(&1u32.to_le_bytes());
        f[24..28].copy_from_slice(&entry.to_le_bytes());
        f[28..32].copy_from_slice(&phoff.to_le_bytes());
        f[40..42].copy_from_slice(&52u16.to_le_bytes());
        f[42..44].copy_from_slice(&32u16.to_le_bytes());
        f[44..46].copy_from_slice(&(segs.len() as u16).to_le_bytes());
        let mut body = Vec::new();
        for (paddr, bytes) in segs {
            let mut ph = [0u8; 32];
            ph[0..4].copy_from_slice(&1u32.to_le_bytes());
            ph[4..8].copy_from_slice(&data_off.to_le_bytes());
            ph[8..12].copy_from_slice(&paddr.to_le_bytes());
            ph[12..16].copy_from_slice(&paddr.to_le_bytes());
            ph[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            ph[20..24].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            f.extend_from_slice(&ph);
            body.extend_from_slice(bytes);
            data_off += bytes.len() as u32;
        }
        f.extend_from_slice(&body);
        f.extend_from_slice(junk);
        f
    }

    #[test]
    fn image_hash_covers_load_bytes_and_entry_only() {
        let a = tiny_elf(&[(0x1000, b"code"), (0x2000, b"data")], 0x1001, b"debug-a");
        let b = tiny_elf(&[(0x1000, b"code"), (0x2000, b"data")], 0x1001, b"debug-bb");
        assert_eq!(elf_image_hash(&a).unwrap(), elf_image_hash(&b).unwrap());
        // Segment order in the file does not matter; addresses do.
        let c = tiny_elf(&[(0x2000, b"data"), (0x1000, b"code")], 0x1001, b"");
        assert_eq!(elf_image_hash(&a).unwrap(), elf_image_hash(&c).unwrap());
        let d = tiny_elf(&[(0x1000, b"code"), (0x2000, b"datA")], 0x1001, b"");
        let e = tiny_elf(&[(0x1000, b"code"), (0x2000, b"data")], 0x1003, b"");
        let f = tiny_elf(&[(0x1000, b"code"), (0x2004, b"data")], 0x1001, b"");
        for x in [&d, &e, &f] {
            assert_ne!(elf_image_hash(&a).unwrap(), elf_image_hash(x).unwrap());
        }
        // The stream, spelled out.
        let mut s = b"bb-elf-image/0\n".to_vec();
        for (p, d) in [(0x1000u64, b"code"), (0x2000u64, b"data")] {
            s.extend_from_slice(&p.to_le_bytes());
            s.extend_from_slice(&4u64.to_le_bytes());
            s.extend_from_slice(d);
        }
        s.extend_from_slice(&0x1001u64.to_le_bytes());
        assert_eq!(
            elf_image_hash(&a).unwrap(),
            format!("sha256:{}", bbpkg::sha256::hex_digest(&s))
        );
    }

    #[test]
    fn image_hash_refuses_non_images() {
        assert!(elf_image_hash(b"\x7fELF not really an image").is_err());
        assert!(elf_image_hash(b"MZ").is_err());
        assert!(elf_image_hash(&tiny_elf(&[], 0, b"")).is_err());
    }
}
