//! Packfile reading (with delta resolution) and writing (without deltas).

use crate::object::{Kind, Oid};
use crate::store::Repo;
use crate::{Error, Result};
use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::io::Write;

enum Raw {
    Full(Kind, Vec<u8>),
    OfsDelta(usize, Vec<u8>),
    RefDelta(Oid, Vec<u8>),
}

fn inflate(data: &[u8], expected: usize) -> Result<(Vec<u8>, usize)> {
    let mut d = Decompress::new(true);
    let mut out: Vec<u8> = Vec::with_capacity(expected + 16);
    loop {
        if out.capacity() - out.len() < 1024 {
            out.reserve(out.capacity().max(4096));
        }
        let (in_before, out_before) = (d.total_in(), out.len());
        let status = d
            .decompress_vec(
                &data[d.total_in() as usize..],
                &mut out,
                FlushDecompress::None,
            )
            .map_err(|e| Error::Invalid(format!("zlib: {e}")))?;
        if status == Status::StreamEnd {
            return Ok((out, d.total_in() as usize));
        }
        if d.total_in() == in_before && out.len() == out_before {
            return Err(Error::Invalid("truncated pack entry".into()));
        }
    }
}

/// Applies a git delta to `base`.
pub fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    fn varint(d: &[u8], i: &mut usize) -> usize {
        let mut v = 0usize;
        let mut shift = 0;
        loop {
            let b = d[*i];
            *i += 1;
            v |= ((b & 0x7f) as usize) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                return v;
            }
        }
    }
    let mut i = 0;
    let src_size = varint(delta, &mut i);
    if src_size != base.len() {
        return Err(Error::Invalid("delta base size mismatch".into()));
    }
    let dst_size = varint(delta, &mut i);
    let mut out = Vec::with_capacity(dst_size);
    while i < delta.len() {
        let op = delta[i];
        i += 1;
        if op & 0x80 != 0 {
            let mut off = 0usize;
            let mut size = 0usize;
            for (bit, shift) in [(0x01, 0), (0x02, 8), (0x04, 16), (0x08, 24)] {
                if op & bit != 0 {
                    off |= (delta[i] as usize) << shift;
                    i += 1;
                }
            }
            for (bit, shift) in [(0x10, 0), (0x20, 8), (0x40, 16)] {
                if op & bit != 0 {
                    size |= (delta[i] as usize) << shift;
                    i += 1;
                }
            }
            if size == 0 {
                size = 0x10000;
            }
            out.extend_from_slice(
                base.get(off..off + size)
                    .ok_or_else(|| Error::Invalid("bad delta copy".into()))?,
            );
        } else if op != 0 {
            let n = op as usize;
            out.extend_from_slice(
                delta
                    .get(i..i + n)
                    .ok_or_else(|| Error::Invalid("bad delta insert".into()))?,
            );
            i += n;
        } else {
            return Err(Error::Invalid("bad delta opcode".into()));
        }
    }
    if out.len() != dst_size {
        return Err(Error::Invalid("delta result size mismatch".into()));
    }
    Ok(out)
}

/// Parses a pack and stores all its objects; returns their ids. Ref-deltas may point at
/// objects already in the store (thin packs).
pub fn unpack(repo: &Repo, data: &[u8]) -> Result<Vec<Oid>> {
    if data.len() < 12 || &data[0..4] != b"PACK" {
        if data.is_empty() {
            return Ok(vec![]);
        }
        return Err(Error::Invalid("not a pack".into()));
    }
    let count = u32::from_be_bytes(data[8..12].try_into().unwrap()) as usize;
    let mut pos = 12;
    let mut entries: Vec<(usize, Raw)> = Vec::with_capacity(count);
    for _ in 0..count {
        let start = pos;
        let mut b = data[pos];
        pos += 1;
        let ty = (b >> 4) & 7;
        let mut size = (b & 0x0f) as usize;
        let mut shift = 4;
        while b & 0x80 != 0 {
            b = data[pos];
            pos += 1;
            size |= ((b & 0x7f) as usize) << shift;
            shift += 7;
        }
        let raw = match ty {
            1..=4 => {
                let (d, used) = inflate(&data[pos..], size)?;
                pos += used;
                Raw::Full(Kind::from_code(ty)?, d)
            }
            6 => {
                let mut c = data[pos];
                pos += 1;
                let mut off = (c & 0x7f) as usize;
                while c & 0x80 != 0 {
                    c = data[pos];
                    pos += 1;
                    off = ((off + 1) << 7) | (c & 0x7f) as usize;
                }
                let (d, used) = inflate(&data[pos..], size)?;
                pos += used;
                Raw::OfsDelta(start - off, d)
            }
            7 => {
                let base = Oid::from_bytes(&data[pos..pos + 20])?;
                pos += 20;
                let (d, used) = inflate(&data[pos..], size)?;
                pos += used;
                Raw::RefDelta(base, d)
            }
            _ => return Err(Error::Invalid(format!("bad pack entry type {ty}"))),
        };
        entries.push((start, raw));
    }

    let mut by_offset: HashMap<usize, (Kind, Vec<u8>)> = HashMap::new();
    let mut by_oid: HashMap<Oid, (Kind, Vec<u8>)> = HashMap::new();
    let mut oids = Vec::with_capacity(count);
    let mut pending: Vec<(usize, Raw)> = vec![];
    for (off, raw) in entries {
        match raw {
            Raw::Full(k, d) => {
                let oid = Oid::hash(k, &d);
                by_oid.insert(oid, (k, d.clone()));
                by_offset.insert(off, (k, d));
                oids.push(oid);
            }
            other => pending.push((off, other)),
        }
    }
    // Resolve deltas; repeat while progress is made (bases may come later in the pack).
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = vec![];
        for (off, raw) in pending {
            let base = match &raw {
                Raw::OfsDelta(b, _) => by_offset.get(b).cloned(),
                Raw::RefDelta(b, _) => match by_oid.get(b) {
                    Some(x) => Some(x.clone()),
                    None => repo.try_read(b)?,
                },
                Raw::Full(..) => unreachable!(),
            };
            match base {
                Some((k, bdata)) => {
                    let delta = match &raw {
                        Raw::OfsDelta(_, d) | Raw::RefDelta(_, d) => d,
                        Raw::Full(..) => unreachable!(),
                    };
                    let d = apply_delta(&bdata, delta)?;
                    let oid = Oid::hash(k, &d);
                    by_oid.insert(oid, (k, d.clone()));
                    by_offset.insert(off, (k, d));
                    oids.push(oid);
                }
                None => next.push((off, raw)),
            }
        }
        if next.len() == before {
            return Err(Error::Invalid("unresolvable delta base in pack".into()));
        }
        pending = next;
    }
    for (_, (k, d)) in by_oid.iter() {
        repo.write(*k, d)?;
    }
    Ok(oids)
}

/// Writes a pack containing `objects` (undeltified).
pub fn write_pack(repo: &Repo, objects: &[Oid]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&(objects.len() as u32).to_be_bytes());
    for oid in objects {
        let (kind, data) = repo.read(oid)?;
        let mut size = data.len();
        let mut b = ((kind as u8) << 4) | (size & 0x0f) as u8;
        size >>= 4;
        while size > 0 {
            out.push(b | 0x80);
            b = (size & 0x7f) as u8;
            size >>= 7;
        }
        out.push(b);
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::fast());
        enc.write_all(&data).unwrap();
        out.extend_from_slice(&enc.finish().unwrap());
    }
    let sum: [u8; 20] = Sha1::digest(&out).into();
    out.extend_from_slice(&sum);
    Ok(out)
}
