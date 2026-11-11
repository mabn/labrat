//! Git objects: ids, kinds, parsing and serialization.

use crate::{Error, Result};
use sha1::{Digest, Sha1};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Oid(pub [u8; 20]);

impl Oid {
    pub const ZERO: Oid = Oid([0; 20]);

    pub fn from_hex(s: &str) -> Result<Oid> {
        let bytes = hex::decode(s).map_err(|_| Error::Invalid(format!("bad object id {s:?}")))?;
        Oid::from_bytes(&bytes)
    }

    pub fn from_bytes(b: &[u8]) -> Result<Oid> {
        let arr: [u8; 20] = b
            .try_into()
            .map_err(|_| Error::Invalid("bad object id length".into()))?;
        Ok(Oid(arr))
    }

    pub fn hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0; 20]
    }

    /// Hashes an object of `kind` with `data` the way git does.
    pub fn hash(kind: Kind, data: &[u8]) -> Oid {
        let mut h = Sha1::new();
        h.update(format!("{} {}\0", kind.name(), data.len()).as_bytes());
        h.update(data);
        Oid(h.finalize().into())
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.hex())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Commit = 1,
    Tree = 2,
    Blob = 3,
    Tag = 4,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Commit => "commit",
            Kind::Tree => "tree",
            Kind::Blob => "blob",
            Kind::Tag => "tag",
        }
    }

    pub fn from_code(c: u8) -> Result<Kind> {
        Ok(match c {
            1 => Kind::Commit,
            2 => Kind::Tree,
            3 => Kind::Blob,
            4 => Kind::Tag,
            _ => return Err(Error::Invalid(format!("bad object type {c}"))),
        })
    }
}

/// `Name <email> 1700000000 +0100`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub email: String,
    /// Seconds since the epoch.
    pub time: i64,
    /// Offset in minutes east of UTC.
    pub offset: i32,
}

impl Signature {
    pub fn parse(s: &str) -> Result<Signature> {
        let lt = s
            .find('<')
            .ok_or_else(|| Error::Invalid("bad signature".into()))?;
        let gt = s
            .rfind('>')
            .ok_or_else(|| Error::Invalid("bad signature".into()))?;
        let name = s[..lt].trim_end().to_string();
        let email = s[lt + 1..gt].to_string();
        let mut rest = s[gt + 1..].split_whitespace();
        let time = rest.next().and_then(|t| t.parse().ok()).unwrap_or(0);
        let tz = rest.next().unwrap_or("+0000");
        let sign = if tz.starts_with('-') { -1 } else { 1 };
        let digits = tz.trim_start_matches(['+', '-']);
        let hh: i32 = digits.get(0..2).and_then(|x| x.parse().ok()).unwrap_or(0);
        let mm: i32 = digits.get(2..4).and_then(|x| x.parse().ok()).unwrap_or(0);
        Ok(Signature {
            name,
            email,
            time,
            offset: sign * (hh * 60 + mm),
        })
    }

    pub fn format(&self) -> String {
        let sign = if self.offset < 0 { '-' } else { '+' };
        let off = self.offset.abs();
        format!(
            "{} <{}> {} {}{:02}{:02}",
            self.name,
            self.email,
            self.time,
            sign,
            off / 60,
            off % 60
        )
    }
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub tree: Oid,
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    /// Extra headers (e.g. `gpgsig`, `encoding`) kept verbatim, in order.
    pub extra: Vec<(String, String)>,
    pub message: String,
}

impl Commit {
    pub fn parse(data: &[u8]) -> Result<Commit> {
        let text = String::from_utf8_lossy(data);
        let (head, message) = match text.find("\n\n") {
            Some(i) => (&text[..i], text[i + 2..].to_string()),
            None => (&text[..], String::new()),
        };
        let mut tree = None;
        let mut parents = vec![];
        let mut author = None;
        let mut committer = None;
        let mut extra: Vec<(String, String)> = vec![];
        for line in head.split('\n') {
            if let Some(cont) = line.strip_prefix(' ') {
                if let Some(last) = extra.last_mut() {
                    last.1.push('\n');
                    last.1.push_str(cont);
                }
                continue;
            }
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            match k {
                "tree" => tree = Some(Oid::from_hex(v)?),
                "parent" => parents.push(Oid::from_hex(v)?),
                "author" => author = Some(Signature::parse(v)?),
                "committer" => committer = Some(Signature::parse(v)?),
                _ => extra.push((k.to_string(), v.to_string())),
            }
        }
        Ok(Commit {
            tree: tree.ok_or_else(|| Error::Invalid("commit without tree".into()))?,
            parents,
            author: author.ok_or_else(|| Error::Invalid("commit without author".into()))?,
            committer: committer
                .ok_or_else(|| Error::Invalid("commit without committer".into()))?,
            extra,
            message,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut s = format!("tree {}\n", self.tree);
        for p in &self.parents {
            s.push_str(&format!("parent {p}\n"));
        }
        s.push_str(&format!("author {}\n", self.author.format()));
        s.push_str(&format!("committer {}\n", self.committer.format()));
        for (k, v) in &self.extra {
            s.push_str(&format!("{k} {}\n", v.replace('\n', "\n ")));
        }
        s.push('\n');
        s.push_str(&self.message);
        s.into_bytes()
    }

    /// First line of the message.
    pub fn title(&self) -> String {
        self.message.lines().next().unwrap_or("").to_string()
    }
}

pub const MODE_TREE: u32 = 0o40000;
pub const MODE_FILE: u32 = 0o100644;
pub const MODE_EXEC: u32 = 0o100755;
pub const MODE_LINK: u32 = 0o120000;
pub const MODE_COMMIT: u32 = 0o160000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: u32,
    pub name: String,
    pub oid: Oid,
}

impl TreeEntry {
    pub fn is_tree(&self) -> bool {
        self.mode == MODE_TREE
    }

    /// Git's tree sort key: directories sort as if their name ended with '/'.
    fn sort_key(&self) -> Vec<u8> {
        let mut k = self.name.as_bytes().to_vec();
        if self.is_tree() {
            k.push(b'/');
        }
        k
    }
}

pub fn parse_tree(data: &[u8]) -> Result<Vec<TreeEntry>> {
    let mut out = vec![];
    let mut i = 0;
    while i < data.len() {
        let sp = data[i..]
            .iter()
            .position(|&b| b == b' ')
            .ok_or_else(|| Error::Invalid("bad tree".into()))?
            + i;
        let nul = data[sp..]
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| Error::Invalid("bad tree".into()))?
            + sp;
        let mode = u32::from_str_radix(std::str::from_utf8(&data[i..sp]).unwrap_or("0"), 8)
            .map_err(|_| Error::Invalid("bad tree mode".into()))?;
        let name = String::from_utf8_lossy(&data[sp + 1..nul]).into_owned();
        let oid = Oid::from_bytes(
            data.get(nul + 1..nul + 21)
                .ok_or_else(|| Error::Invalid("bad tree".into()))?,
        )?;
        out.push(TreeEntry { mode, name, oid });
        i = nul + 21;
    }
    Ok(out)
}

pub fn encode_tree(entries: &mut [TreeEntry]) -> Vec<u8> {
    entries.sort_by_key(|e| e.sort_key());
    let mut out = vec![];
    for e in entries.iter() {
        out.extend_from_slice(format!("{:o} {}\0", e.mode, e.name).as_bytes());
        out.extend_from_slice(&e.oid.0);
    }
    out
}

#[derive(Clone, Debug)]
pub struct Tag {
    pub object: Oid,
    pub kind: String,
    pub name: String,
    pub tagger: Option<Signature>,
    pub message: String,
}

impl Tag {
    pub fn parse(data: &[u8]) -> Result<Tag> {
        let text = String::from_utf8_lossy(data);
        let (head, message) = match text.find("\n\n") {
            Some(i) => (&text[..i], text[i + 2..].to_string()),
            None => (&text[..], String::new()),
        };
        let mut object = None;
        let mut kind = String::new();
        let mut name = String::new();
        let mut tagger = None;
        for line in head.lines() {
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            match k {
                "object" => object = Some(Oid::from_hex(v)?),
                "type" => kind = v.to_string(),
                "tag" => name = v.to_string(),
                "tagger" => tagger = Some(Signature::parse(v)?),
                _ => {}
            }
        }
        Ok(Tag {
            object: object.ok_or_else(|| Error::Invalid("tag without object".into()))?,
            kind,
            name,
            tagger,
            message,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut s = format!(
            "object {}\ntype {}\ntag {}\n",
            self.object, self.kind, self.name
        );
        if let Some(t) = &self.tagger {
            s.push_str(&format!("tagger {}\n", t.format()));
        }
        s.push('\n');
        s.push_str(&self.message);
        s.into_bytes()
    }
}
