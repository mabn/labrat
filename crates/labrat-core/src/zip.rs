//! Minimal zip archives for CI job artifacts: writing (stored entries, fixed timestamps, so
//! the same files always give the same bytes) and reading (stored and deflated entries).

use std::io::Read;

/// A zip archive of `files` (path, content), in the given order. Directories are implied by
/// the paths, as `zip` without `-D` would store them only when given.
pub fn write(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    // 1980-01-01 00:00, the earliest DOS date.
    let (time, date): (u16, u16) = (0, 0x21);
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let n = name.as_bytes();
        out.extend_from_slice(&0x04034b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&time.to_le_bytes());
        out.extend_from_slice(&date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(n);
        out.extend_from_slice(data);

        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        central.extend_from_slice(&0x031eu16.to_le_bytes()); // made by: Unix, 3.0
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&time.to_le_bytes());
        central.extend_from_slice(&date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(n.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&(0o100644u32 << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(n);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// One entry of an archive.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    /// Unix mode bits when the archive records them.
    pub mode: Option<u32>,
    pub size: u64,
    method: u16,
    compressed: Vec<u8>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }

    /// The entry's content (stored or deflated).
    pub fn data(&self) -> Option<Vec<u8>> {
        match self.method {
            0 => Some(self.compressed.clone()),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(&self.compressed[..]).read_to_end(&mut out).ok()?;
                Some(out)
            }
            _ => None,
        }
    }
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// The entries of a zip archive, from its central directory; `None` when it isn't one.
pub fn read(b: &[u8]) -> Option<Vec<Entry>> {
    let eocd = (0..=b.len().checked_sub(22)?).rev().take(65_557).find(|&i| u32_at(b, i) == Some(0x06054b50))?;
    let count = u16_at(b, eocd + 10)? as usize;
    let mut p = u32_at(b, eocd + 16)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(b, p)? != 0x02014b50 {
            return None;
        }
        let made_by = u16_at(b, p + 4)?;
        let method = u16_at(b, p + 10)?;
        let csize = u32_at(b, p + 20)? as usize;
        let size = u32_at(b, p + 24)? as u64;
        let nlen = u16_at(b, p + 28)? as usize;
        let elen = u16_at(b, p + 30)? as usize;
        let clen = u16_at(b, p + 32)? as usize;
        let ext = u32_at(b, p + 38)?;
        let local = u32_at(b, p + 42)? as usize;
        let name = String::from_utf8_lossy(b.get(p + 46..p + 46 + nlen)?).into_owned();
        p += 46 + nlen + elen + clen;
        if u32_at(b, local)? != 0x04034b50 {
            return None;
        }
        let start = local + 30 + u16_at(b, local + 26)? as usize + u16_at(b, local + 28)? as usize;
        let compressed = b.get(start..start + csize)?.to_vec();
        let mode = (made_by >> 8 == 3).then_some(ext >> 16).filter(|m| *m != 0);
        out.push(Entry { name, mode, size, method, compressed });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip() {
        let files = vec![("dist/a.txt".to_string(), b"hello\n".to_vec()), ("b.md".to_string(), b"# b\n".to_vec())];
        let z = super::write(&files);
        let e = super::read(&z).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].name, "dist/a.txt");
        assert_eq!(e[0].data().unwrap(), b"hello\n");
        assert_eq!(e[1].mode, Some(0o100644));
    }
}
