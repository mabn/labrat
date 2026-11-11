//! Git smart HTTP protocol (v0/v1): ref advertisement, upload-pack and receive-pack.

use crate::object::Oid;
use crate::pack;
use crate::store::Repo;
use crate::walk;
use crate::{Error, Result};

pub fn pkt(line: &str) -> Vec<u8> {
    pkt_bytes(line.as_bytes())
}

pub fn pkt_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = format!("{:04x}", data.len() + 4).into_bytes();
    out.extend_from_slice(data);
    out
}

pub const FLUSH: &[u8] = b"0000";

/// Splits pkt-lines; `None` marks a flush packet. Stops after `max` lines if given.
pub fn read_pkts(data: &[u8]) -> Result<(Vec<Option<Vec<u8>>>, usize)> {
    let mut out = vec![];
    let mut pos = 0;
    while pos + 4 <= data.len() {
        let len =
            usize::from_str_radix(std::str::from_utf8(&data[pos..pos + 4]).unwrap_or("x"), 16)
                .map_err(|_| Error::Invalid("bad pkt-line".into()))?;
        if len == 0 {
            out.push(None);
            pos += 4;
            continue;
        }
        if len < 4 || pos + len > data.len() {
            return Err(Error::Invalid("bad pkt-line length".into()));
        }
        out.push(Some(data[pos + 4..pos + len].to_vec()));
        pos += len;
    }
    Ok((out, pos))
}

/// Sends data on a side-band channel, split into 64k packets.
pub fn sideband(channel: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![];
    for chunk in data.chunks(65515) {
        let mut d = vec![channel];
        d.extend_from_slice(chunk);
        out.extend(pkt_bytes(&d));
    }
    out
}

const AGENT: &str = "agent=git/2.51.0-labrat";

/// Body of `GET /info/refs?service=...`.
pub fn advertise(
    repo: &Repo,
    service: &str,
    head: Option<&str>,
    hidden: &dyn Fn(&str) -> bool,
) -> Result<Vec<u8>> {
    let mut out = pkt(&format!("# service={service}\n"));
    out.extend_from_slice(FLUSH);
    let mut refs: Vec<(String, Oid)> = repo
        .refs()?
        .into_iter()
        .filter(|(n, _)| !hidden(n))
        .collect();
    let caps = if service == "git-upload-pack" {
        let mut c = "multi_ack thin-pack side-band side-band-64k ofs-delta shallow deepen-relative no-progress multi_ack_detailed allow-tip-sha1-in-want allow-reachable-sha1-in-want".to_string();
        if let Some(h) = head {
            if repo.branch(h)?.is_some() {
                c.push_str(&format!(" symref=HEAD:refs/heads/{h}"));
            }
        }
        format!("{c} object-format=sha1 {AGENT}")
    } else {
        format!(
            "report-status report-status-v2 delete-refs side-band-64k quiet atomic ofs-delta push-options object-format=sha1 {AGENT}"
        )
    };
    let mut lines: Vec<(String, Oid)> = vec![];
    if service == "git-upload-pack" {
        if let Some(h) = head {
            if let Some(o) = repo.branch(h)? {
                lines.push(("HEAD".into(), o));
            }
        }
    }
    refs.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, oid) in &refs {
        lines.push((name.clone(), *oid));
        if service == "git-upload-pack" && name.starts_with("refs/tags/") {
            let peeled = repo.peel(oid)?;
            if peeled != *oid {
                lines.push((format!("{name}^{{}}"), peeled));
            }
        }
    }
    if lines.is_empty() {
        out.extend(pkt_bytes(
            format!("{} capabilities^{{}}\0{caps}\n", Oid::ZERO).as_bytes(),
        ));
    } else {
        for (i, (name, oid)) in lines.iter().enumerate() {
            if i == 0 {
                out.extend(pkt_bytes(format!("{oid} {name}\0{caps}\n").as_bytes()));
            } else {
                out.extend(pkt(&format!("{oid} {name}\n")));
            }
        }
    }
    out.extend_from_slice(FLUSH);
    Ok(out)
}

/// Handles a `git-upload-pack` POST and returns the response body.
pub fn upload_pack(repo: &Repo, body: &[u8]) -> Result<Vec<u8>> {
    let (lines, _) = read_pkts(body)?;
    let mut wants = vec![];
    let mut haves = vec![];
    let mut done = false;
    let mut caps: Vec<String> = vec![];
    let mut client_shallow: Vec<Oid> = vec![];
    let mut deepen: Option<u64> = None;
    let mut relative = false;
    for line in lines.into_iter().flatten() {
        let text = String::from_utf8_lossy(&line).trim_end().to_string();
        if let Some(rest) = text.strip_prefix("shallow ") {
            client_shallow.push(Oid::from_hex(rest.trim())?);
            continue;
        }
        if let Some(rest) = text.strip_prefix("deepen ") {
            deepen = rest.trim().parse().ok();
            continue;
        }
        if text == "deepen-relative" {
            relative = true;
            continue;
        }
        if let Some(rest) = text.strip_prefix("want ") {
            let (oid, c) = rest.split_once(' ').unwrap_or((rest, ""));
            if caps.is_empty() && !c.is_empty() {
                caps = c.split(' ').map(str::to_string).collect();
            }
            wants.push(Oid::from_hex(oid)?);
        } else if let Some(rest) = text.strip_prefix("have ") {
            haves.push(Oid::from_hex(rest.trim())?);
        } else if text == "done" {
            done = true;
        }
    }
    let has = |c: &str| caps.iter().any(|x| x == c);
    relative = relative || has("deepen-relative");
    // Shallow clones: the shallow-update section comes first in every (stateless) response.
    let mut out = vec![];
    let mut shallow_cut: Option<(std::collections::HashSet<Oid>, std::collections::HashSet<Oid>)> = None;
    if let Some(n) = deepen {
        let tips: Vec<Oid> = wants.iter().map(|w| repo.peel(w).unwrap_or(*w)).collect();
        let (boundary, reached) = walk::shallow_boundary(repo, &tips, &client_shallow, n, relative)?;
        let known: std::collections::HashSet<Oid> = client_shallow.iter().copied().collect();
        for b in &boundary {
            if !known.contains(b) {
                out.extend(pkt(&format!("shallow {b}\n")));
            }
        }
        for s in &client_shallow {
            if reached.contains(s) && !boundary.contains(s) {
                out.extend(pkt(&format!("unshallow {s}\n")));
            }
        }
        out.extend_from_slice(FLUSH);
        // Commits still shallow on the client keep their parents out of the pack.
        let mut cut = boundary.clone();
        cut.extend(client_shallow.iter().filter(|s| !reached.contains(*s) || boundary.contains(*s)).copied());
        shallow_cut = Some((cut, known));
    } else if !client_shallow.is_empty() {
        let known: std::collections::HashSet<Oid> = client_shallow.iter().copied().collect();
        shallow_cut = Some((known.clone(), known));
    }
    let multi_ack_detailed = has("multi_ack_detailed");
    let multi_ack = has("multi_ack") || multi_ack_detailed;
    let common: Vec<Oid> = haves
        .iter()
        .filter(|h| repo.exists(h).unwrap_or(false))
        .copied()
        .collect();
    // A request with wants and `deepen` but no haves is only the shallow negotiation: upload-pack
    // sends the shallow list and stops at the end of the request.
    if !done && haves.is_empty() && deepen.is_some() {
        return Ok(out);
    }
    if !done {
        // Negotiation round of the stateless protocol: acknowledge common commits.
        if multi_ack {
            for c in &common {
                let suffix = if multi_ack_detailed {
                    " common"
                } else {
                    " continue"
                };
                out.extend(pkt(&format!("ACK {c}{suffix}\n")));
            }
        }
        out.extend(pkt("NAK\n"));
        return Ok(out);
    }
    if common.is_empty() {
        out.extend(pkt("NAK\n"));
    } else if multi_ack {
        for c in &common {
            let suffix = if multi_ack_detailed {
                " common"
            } else {
                " continue"
            };
            out.extend(pkt(&format!("ACK {c}{suffix}\n")));
        }
        out.extend(pkt(&format!("ACK {}\n", common.last().unwrap())));
    } else {
        out.extend(pkt(&format!("ACK {}\n", common[0])));
    }
    let objects = match &shallow_cut {
        Some((cut, client)) => {
            // Tag objects asked for directly go along with the commits they point at.
            let mut objs: Vec<Oid> = wants.iter().filter(|w| repo.peel(w).map(|p| p != **w).unwrap_or(false)).copied().collect();
            let tips: Vec<Oid> = wants.iter().map(|w| repo.peel(w).unwrap_or(*w)).collect();
            objs.extend(walk::objects_for_shallow_pack(repo, &tips, &common, cut, client)?);
            objs
        }
        None => walk::objects_for_pack(repo, &wants, &common)?,
    };
    let data = pack::write_pack(repo, &objects)?;
    if has("side-band-64k") || has("side-band") {
        out.extend(sideband(1, &data));
        out.extend_from_slice(FLUSH);
    } else {
        out.extend(data);
    }
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct RefUpdate {
    pub old: Oid,
    pub new: Oid,
    pub name: String,
}

pub struct ReceiveRequest {
    pub updates: Vec<RefUpdate>,
    pub capabilities: Vec<String>,
    pub push_options: Vec<String>,
    pub pack: Vec<u8>,
}

/// Parses a `git-receive-pack` POST body.
pub fn parse_receive(body: &[u8]) -> Result<ReceiveRequest> {
    let mut pos = 0;
    let mut updates = vec![];
    let mut capabilities = vec![];
    // Commands until the first flush.
    loop {
        if pos + 4 > body.len() {
            break;
        }
        let len =
            usize::from_str_radix(std::str::from_utf8(&body[pos..pos + 4]).unwrap_or("x"), 16)
                .map_err(|_| Error::Invalid("bad pkt-line".into()))?;
        if len == 0 {
            pos += 4;
            break;
        }
        let line = &body[pos + 4..pos + len];
        pos += len;
        let (cmd, caps) = match line.iter().position(|&b| b == 0) {
            Some(i) => (&line[..i], Some(&line[i + 1..])),
            None => (line, None),
        };
        if let Some(c) = caps {
            capabilities = String::from_utf8_lossy(c)
                .split_whitespace()
                .map(str::to_string)
                .collect();
        }
        let text = String::from_utf8_lossy(cmd).trim_end().to_string();
        // A shallow client lists its shallow commits before the commands.
        if text.starts_with("shallow ") {
            continue;
        }
        let mut parts = text.splitn(3, ' ');
        let old = Oid::from_hex(parts.next().unwrap_or(""))?;
        let new = Oid::from_hex(parts.next().unwrap_or(""))?;
        let name = parts.next().unwrap_or("").to_string();
        updates.push(RefUpdate { old, new, name });
    }
    let mut push_options = vec![];
    if capabilities.iter().any(|c| c == "push-options") {
        loop {
            if pos + 4 > body.len() {
                break;
            }
            let len =
                usize::from_str_radix(std::str::from_utf8(&body[pos..pos + 4]).unwrap_or("x"), 16)
                    .map_err(|_| Error::Invalid("bad pkt-line".into()))?;
            if len == 0 {
                pos += 4;
                break;
            }
            push_options.push(
                String::from_utf8_lossy(&body[pos + 4..pos + len])
                    .trim_end()
                    .to_string(),
            );
            pos += len;
        }
    }
    Ok(ReceiveRequest {
        updates,
        capabilities,
        push_options,
        pack: body[pos..].to_vec(),
    })
}

/// Builds the receive-pack response. `results` holds `None` for success or an error reason
/// per update; `messages` are shown to the user as `remote:` lines.
pub fn receive_response(
    req: &ReceiveRequest,
    unpack_error: Option<&str>,
    results: &[(String, Option<String>)],
    messages: &str,
) -> Vec<u8> {
    let sideband64 = req.capabilities.iter().any(|c| c == "side-band-64k");
    let mut status = vec![];
    match unpack_error {
        None => status.extend(pkt("unpack ok\n")),
        Some(e) => status.extend(pkt(&format!("unpack {e}\n"))),
    }
    for (name, err) in results {
        match err {
            None => status.extend(pkt(&format!("ok {name}\n"))),
            Some(e) => status.extend(pkt(&format!("ng {name} {e}\n"))),
        }
    }
    status.extend_from_slice(FLUSH);
    if !sideband64 {
        return status;
    }
    let mut out = vec![];
    if !messages.is_empty() {
        out.extend(sideband(2, messages.as_bytes()));
    }
    out.extend(sideband(1, &status));
    out.extend_from_slice(FLUSH);
    out
}
