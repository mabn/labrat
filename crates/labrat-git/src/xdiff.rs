//! A port of git's xdiff line diff (the default Myers algorithm): record classification and
//! discarding (`xprepare.c`), the divide-and-conquer Myers search with git's cost heuristics
//! (`xdiffi.c`: `xdl_recs_cmp`, `xdl_split`) and change compaction with the indent heuristic
//! (`xdl_change_compact`). Lines are compared as raw bytes, as `git diff` does without
//! whitespace options, so the edit scripts match git's line for line, including ties.

use std::collections::HashMap;

const XDL_MAX_COST_MIN: isize = 256;
const XDL_HEUR_MIN_COST: isize = 256;
const XDL_LINE_MAX: isize = isize::MAX;
const XDL_SNAKE_CNT: isize = 20;
const XDL_K_HEUR: isize = 4;
const XDL_MAX_EQLIMIT: isize = 1024;
const XDL_SIMSCAN_WINDOW: isize = 100;
const XDL_KPDIS_RUN: isize = 4;

const MAX_INDENT: i32 = 200;
const MAX_BLANKS: i32 = 20;
const START_OF_FILE_PENALTY: i32 = 1;
const END_OF_FILE_PENALTY: i32 = 21;
const TOTAL_BLANK_WEIGHT: i32 = -30;
const POST_BLANK_WEIGHT: i32 = 6;
const RELATIVE_INDENT_PENALTY: i32 = -4;
const RELATIVE_INDENT_WITH_BLANK_PENALTY: i32 = 10;
const RELATIVE_OUTDENT_PENALTY: i32 = 24;
const RELATIVE_OUTDENT_WITH_BLANK_PENALTY: i32 = 17;
const RELATIVE_DEDENT_PENALTY: i32 = 23;
const RELATIVE_DEDENT_WITH_BLANK_PENALTY: i32 = 17;
const INDENT_WEIGHT: i32 = 60;
const INDENT_HEURISTIC_MAX_SLIDING: isize = 100;

/// One change of an edit script (git's `xdchange_t`): `old_len` lines of the old file starting
/// at `old` (0-based) are replaced by `new_len` lines of the new file starting at `new`.
/// Either length may be zero (pure insertion / deletion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub old: usize,
    pub old_len: usize,
    pub new: usize,
    pub new_len: usize,
}

/// A run of an edit script in file order. A change is reported as its `Delete` followed by its
/// `Insert`, the order in which git prints `-` and `+` lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Equal { old: usize, new: usize, len: usize },
    Delete { old: usize, new: usize, len: usize },
    Insert { old: usize, new: usize, len: usize },
}

/// Splits a buffer into lines, each keeping its `\n` (the last one may lack it), like xdiff's
/// records.
pub fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    data.split_inclusive(|&c| c == b'\n').collect()
}

/// `git diff`'s edit script between two line lists (Myers with the indent heuristic).
pub fn diff(a: &[&[u8]], b: &[&[u8]]) -> Vec<Change> {
    diff_with(a, b, true)
}

/// The edit script, optionally without the indent heuristic (git's merge machinery runs
/// without it).
pub fn diff_with(a: &[&[u8]], b: &[&[u8]], indent_heuristic: bool) -> Vec<Change> {
    let (mut f1, mut f2) = prepare(a, b);
    do_diff(&mut f1, &mut f2);
    change_compact(&mut f1, &f2, indent_heuristic);
    change_compact(&mut f2, &f1, indent_heuristic);
    build_script(&f1, &f2)
}

/// Expands an edit script into equal / delete / insert runs covering both files.
pub fn ops(changes: &[Change], old_len: usize, new_len: usize) -> Vec<Op> {
    let mut out = vec![];
    let (mut o, mut n) = (0, 0);
    for c in changes {
        if c.old > o {
            out.push(Op::Equal { old: o, new: n, len: c.old - o });
        }
        if c.old_len > 0 {
            out.push(Op::Delete { old: c.old, new: c.new, len: c.old_len });
        }
        if c.new_len > 0 {
            out.push(Op::Insert { old: c.old + c.old_len, new: c.new, len: c.new_len });
        }
        o = c.old + c.old_len;
        n = c.new + c.new_len;
    }
    if o < old_len {
        out.push(Op::Equal { old: o, new: n, len: old_len - o });
    }
    debug_assert_eq!(new_len - n, old_len - o);
    out
}

/// One side of the diff (git's `xdfile_t`).
struct XdFile<'a> {
    recs: &'a [&'a [u8]],
    /// Equivalence class of every record.
    ha: Vec<usize>,
    /// Changed flags with a sentinel on both ends: record `i` lives at `rchg[i + 1]`.
    rchg: Vec<bool>,
    /// Records taking part in the Myers search (not trimmed, not discarded) ...
    rindex: Vec<usize>,
    /// ... and their classes.
    reff_ha: Vec<usize>,
    dstart: isize,
    dend: isize,
}

impl XdFile<'_> {
    fn nrec(&self) -> isize {
        self.recs.len() as isize
    }

    fn chg(&self, i: isize) -> bool {
        self.rchg[(i + 1) as usize]
    }

    fn set_chg(&mut self, i: isize, v: bool) {
        self.rchg[(i + 1) as usize] = v;
    }
}

fn bogosqrt(mut n: isize) -> isize {
    let mut i = 1;
    while n > 0 {
        i <<= 1;
        n >>= 2;
    }
    i
}

/// `xdl_prepare_env` for the Myers algorithm: classify, trim common ends, discard records.
fn prepare<'a>(a: &'a [&'a [u8]], b: &'a [&'a [u8]]) -> (XdFile<'a>, XdFile<'a>) {
    let mut classes: HashMap<&[u8], usize> = HashMap::new();
    // Occurrences of each class in the old (`[0]`) and new (`[1]`) file.
    let mut counts: Vec<[isize; 2]> = vec![];
    let mut classify = |recs: &'a [&'a [u8]], pass: usize| -> Vec<usize> {
        recs.iter()
            .map(|r| {
                let id = *classes.entry(r).or_insert_with(|| {
                    counts.push([0, 0]);
                    counts.len() - 1
                });
                counts[id][pass] += 1;
                id
            })
            .collect()
    };
    let ha1 = classify(a, 0);
    let ha2 = classify(b, 1);
    let new_file = |recs: &'a [&'a [u8]], ha: Vec<usize>| XdFile {
        recs,
        ha,
        rchg: vec![false; recs.len() + 2],
        rindex: vec![],
        reff_ha: vec![],
        dstart: 0,
        dend: 0,
    };
    let mut f1 = new_file(a, ha1);
    let mut f2 = new_file(b, ha2);
    trim_ends(&mut f1, &mut f2);
    cleanup_records(&counts, &mut f1, &mut f2);
    (f1, f2)
}

/// `xdl_trim_ends`.
fn trim_ends(f1: &mut XdFile, f2: &mut XdFile) {
    let lim = f1.recs.len().min(f2.recs.len());
    let mut i = 0;
    while i < lim && f1.ha[i] == f2.ha[i] {
        i += 1;
    }
    f1.dstart = i as isize;
    f2.dstart = i as isize;
    let lim = lim - i;
    let (n1, n2) = (f1.recs.len(), f2.recs.len());
    let mut j = 0;
    while j < lim && f1.ha[n1 - 1 - j] == f2.ha[n2 - 1 - j] {
        j += 1;
    }
    f1.dend = n1 as isize - j as isize - 1;
    f2.dend = n2 as isize - j as isize - 1;
}

/// `xdl_clean_mmatch`: whether a multi-match record sits in a run of unmatched records and
/// should be discarded.
fn clean_mmatch(dis: &[u8], i: isize, mut s: isize, mut e: isize) -> bool {
    if i - s > XDL_SIMSCAN_WINDOW {
        s = i - XDL_SIMSCAN_WINDOW;
    }
    if e - i > XDL_SIMSCAN_WINDOW {
        e = i + XDL_SIMSCAN_WINDOW;
    }
    let (mut rdis0, mut rpdis0) = (0, 1);
    let mut r = 1;
    while i - r >= s {
        match dis[(i - r) as usize] {
            0 => rdis0 += 1,
            2 => rpdis0 += 1,
            _ => break,
        }
        r += 1;
    }
    if rdis0 == 0 {
        return false;
    }
    let (mut rdis1, mut rpdis1) = (0, 1);
    let mut r = 1;
    while i + r <= e {
        match dis[(i + r) as usize] {
            0 => rdis1 += 1,
            2 => rpdis1 += 1,
            _ => break,
        }
        r += 1;
    }
    if rdis1 == 0 {
        return false;
    }
    rdis1 += rdis0;
    rpdis1 += rpdis0;
    rpdis1 * XDL_KPDIS_RUN < rpdis1 + rdis1
}

/// `xdl_cleanup_records`: records without a counterpart on the other side are marked changed
/// up front; records occurring too often are dropped when surrounded by unmatched ones.
fn cleanup_records(counts: &[[isize; 2]], f1: &mut XdFile, f2: &mut XdFile) {
    fn mark(counts: &[[isize; 2]], f: &XdFile, other: usize) -> Vec<u8> {
        let mut dis = vec![0u8; f.recs.len()];
        let mlim = bogosqrt(f.nrec()).min(XDL_MAX_EQLIMIT);
        for i in f.dstart..=f.dend {
            let nm = counts[f.ha[i as usize]][other];
            dis[i as usize] = if nm == 0 {
                0
            } else if nm >= mlim {
                2
            } else {
                1
            };
        }
        dis
    }
    fn keep(f: &mut XdFile, dis: &[u8]) {
        for i in f.dstart..=f.dend {
            let d = dis[i as usize];
            if d == 1 || (d == 2 && !clean_mmatch(dis, i, f.dstart, f.dend)) {
                f.rindex.push(i as usize);
                f.reff_ha.push(f.ha[i as usize]);
            } else {
                f.set_chg(i, true);
            }
        }
    }
    let dis1 = mark(counts, f1, 1);
    let dis2 = mark(counts, f2, 0);
    keep(f1, &dis1);
    keep(f2, &dis2);
}

struct Split {
    i1: isize,
    i2: isize,
    min_lo: bool,
    min_hi: bool,
}

/// Diagonal vectors of the forward and backward searches, indexed by diagonal `d` (which may
/// be negative).
struct Kv {
    f: Vec<isize>,
    b: Vec<isize>,
    off: isize,
}

impl Kv {
    #[inline]
    fn f(&self, d: isize) -> isize {
        self.f[(d + self.off) as usize]
    }
    #[inline]
    fn b(&self, d: isize) -> isize {
        self.b[(d + self.off) as usize]
    }
    #[inline]
    fn set_f(&mut self, d: isize, v: isize) {
        self.f[(d + self.off) as usize] = v;
    }
    #[inline]
    fn set_b(&mut self, d: isize, v: isize) {
        self.b[(d + self.off) as usize] = v;
    }
}

struct Env {
    mxcost: isize,
    snake_cnt: isize,
    heur_min: isize,
}

/// `xdl_split`: finds the middle snake of the box, or a good-enough split point once the
/// cost heuristics kick in.
#[allow(clippy::too_many_arguments)]
fn split(
    ha1: &[usize],
    off1: isize,
    lim1: isize,
    ha2: &[usize],
    off2: isize,
    lim2: isize,
    kv: &mut Kv,
    need_min: bool,
    env: &Env,
) -> Split {
    let h1 = |i: isize| ha1[i as usize];
    let h2 = |i: isize| ha2[i as usize];
    let dmin = off1 - lim2;
    let dmax = lim1 - off2;
    let fmid = off1 - off2;
    let bmid = lim1 - lim2;
    let odd = (fmid - bmid) & 1 != 0;
    let (mut fmin, mut fmax) = (fmid, fmid);
    let (mut bmin, mut bmax) = (bmid, bmid);

    kv.set_f(fmid, off1);
    kv.set_b(bmid, lim1);

    let mut ec: isize = 1;
    loop {
        let mut got_snake = false;

        if fmin > dmin {
            fmin -= 1;
            kv.set_f(fmin - 1, -1);
        } else {
            fmin += 1;
        }
        if fmax < dmax {
            fmax += 1;
            kv.set_f(fmax + 1, -1);
        } else {
            fmax -= 1;
        }

        let mut d = fmax;
        while d >= fmin {
            let mut i1 = if kv.f(d - 1) >= kv.f(d + 1) {
                kv.f(d - 1) + 1
            } else {
                kv.f(d + 1)
            };
            let prev1 = i1;
            let mut i2 = i1 - d;
            while i1 < lim1 && i2 < lim2 && h1(i1) == h2(i2) {
                i1 += 1;
                i2 += 1;
            }
            if i1 - prev1 > env.snake_cnt {
                got_snake = true;
            }
            kv.set_f(d, i1);
            if odd && bmin <= d && d <= bmax && kv.b(d) <= i1 {
                return Split { i1, i2, min_lo: true, min_hi: true };
            }
            d -= 2;
        }

        if bmin > dmin {
            bmin -= 1;
            kv.set_b(bmin - 1, XDL_LINE_MAX);
        } else {
            bmin += 1;
        }
        if bmax < dmax {
            bmax += 1;
            kv.set_b(bmax + 1, XDL_LINE_MAX);
        } else {
            bmax -= 1;
        }

        let mut d = bmax;
        while d >= bmin {
            let mut i1 = if kv.b(d - 1) < kv.b(d + 1) {
                kv.b(d - 1)
            } else {
                kv.b(d + 1) - 1
            };
            let prev1 = i1;
            let mut i2 = i1 - d;
            while i1 > off1 && i2 > off2 && h1(i1 - 1) == h2(i2 - 1) {
                i1 -= 1;
                i2 -= 1;
            }
            if prev1 - i1 > env.snake_cnt {
                got_snake = true;
            }
            kv.set_b(d, i1);
            if !odd && fmin <= d && d <= fmax && i1 <= kv.f(d) {
                return Split { i1, i2, min_lo: true, min_hi: true };
            }
            d -= 2;
        }

        if need_min {
            ec += 1;
            continue;
        }

        if got_snake && ec > env.heur_min {
            let mut best = 0;
            let mut spl = (0, 0);
            let mut d = fmax;
            while d >= fmin {
                let dd = if d > fmid { d - fmid } else { fmid - d };
                let i1 = kv.f(d);
                let i2 = i1 - d;
                let v = (i1 - off1) + (i2 - off2) - dd;
                if v > XDL_K_HEUR * ec
                    && v > best
                    && off1 + env.snake_cnt <= i1
                    && i1 < lim1
                    && off2 + env.snake_cnt <= i2
                    && i2 < lim2
                {
                    let mut k = 1;
                    while h1(i1 - k) == h2(i2 - k) {
                        if k == env.snake_cnt {
                            best = v;
                            spl = (i1, i2);
                            break;
                        }
                        k += 1;
                    }
                }
                d -= 2;
            }
            if best > 0 {
                return Split { i1: spl.0, i2: spl.1, min_lo: true, min_hi: false };
            }

            let mut best = 0;
            let mut d = bmax;
            while d >= bmin {
                let dd = if d > bmid { d - bmid } else { bmid - d };
                let i1 = kv.b(d);
                let i2 = i1 - d;
                let v = (lim1 - i1) + (lim2 - i2) - dd;
                if v > XDL_K_HEUR * ec
                    && v > best
                    && off1 < i1
                    && i1 <= lim1 - env.snake_cnt
                    && off2 < i2
                    && i2 <= lim2 - env.snake_cnt
                {
                    let mut k = 0;
                    while h1(i1 + k) == h2(i2 + k) {
                        if k == env.snake_cnt - 1 {
                            best = v;
                            spl = (i1, i2);
                            break;
                        }
                        k += 1;
                    }
                }
                d -= 2;
            }
            if best > 0 {
                return Split { i1: spl.0, i2: spl.1, min_lo: false, min_hi: true };
            }
        }

        if ec >= env.mxcost {
            let (mut fbest, mut fbest1) = (-1, -1);
            let mut d = fmax;
            while d >= fmin {
                let mut i1 = kv.f(d).min(lim1);
                let mut i2 = i1 - d;
                if lim2 < i2 {
                    i1 = lim2 + d;
                    i2 = lim2;
                }
                if fbest < i1 + i2 {
                    fbest = i1 + i2;
                    fbest1 = i1;
                }
                d -= 2;
            }
            let (mut bbest, mut bbest1) = (XDL_LINE_MAX, XDL_LINE_MAX);
            let mut d = bmax;
            while d >= bmin {
                let mut i1 = off1.max(kv.b(d));
                let mut i2 = i1 - d;
                if i2 < off2 {
                    i1 = off2 + d;
                    i2 = off2;
                }
                if i1 + i2 < bbest {
                    bbest = i1 + i2;
                    bbest1 = i1;
                }
                d -= 2;
            }
            return if (lim1 + lim2) - bbest < fbest - (off1 + off2) {
                Split { i1: fbest1, i2: fbest - fbest1, min_lo: true, min_hi: false }
            } else {
                Split { i1: bbest1, i2: bbest - bbest1, min_lo: false, min_hi: true }
            };
        }
        ec += 1;
    }
}

/// `xdl_do_diff` for Myers: runs `xdl_recs_cmp` over the non-discarded records.
fn do_diff(f1: &mut XdFile, f2: &mut XdFile) {
    let n1 = f1.reff_ha.len() as isize;
    let n2 = f2.reff_ha.len() as isize;
    let ndiags = n1 + n2 + 3;
    let mut kv = Kv {
        f: vec![0; ndiags as usize],
        b: vec![0; ndiags as usize],
        off: n2 + 1,
    };
    let env = Env {
        mxcost: bogosqrt(ndiags).max(XDL_MAX_COST_MIN),
        snake_cnt: XDL_SNAKE_CNT,
        heur_min: XDL_HEUR_MIN_COST,
    };
    let ha1 = std::mem::take(&mut f1.reff_ha);
    let ha2 = std::mem::take(&mut f2.reff_ha);
    // `xdl_recs_cmp` recursion as an explicit stack, visiting boxes in the same order.
    let mut stack = vec![(0isize, n1, 0isize, n2, false)];
    while let Some((mut off1, mut lim1, mut off2, mut lim2, need_min)) = stack.pop() {
        while off1 < lim1 && off2 < lim2 && ha1[off1 as usize] == ha2[off2 as usize] {
            off1 += 1;
            off2 += 1;
        }
        while off1 < lim1 && off2 < lim2 && ha1[lim1 as usize - 1] == ha2[lim2 as usize - 1] {
            lim1 -= 1;
            lim2 -= 1;
        }
        if off1 == lim1 {
            for i in off2..lim2 {
                let r = f2.rindex[i as usize] as isize;
                f2.set_chg(r, true);
            }
        } else if off2 == lim2 {
            for i in off1..lim1 {
                let r = f1.rindex[i as usize] as isize;
                f1.set_chg(r, true);
            }
        } else {
            let spl = split(&ha1, off1, lim1, &ha2, off2, lim2, &mut kv, need_min, &env);
            stack.push((spl.i1, lim1, spl.i2, lim2, spl.min_hi));
            stack.push((off1, spl.i1, off2, spl.i2, spl.min_lo));
        }
    }
    f1.reff_ha = ha1;
    f2.reff_ha = ha2;
}

#[derive(Clone, Copy)]
struct Group {
    start: isize,
    end: isize,
}

fn group_init(f: &XdFile) -> Group {
    let mut g = Group { start: 0, end: 0 };
    while f.chg(g.end) {
        g.end += 1;
    }
    g
}

/// Moves to the next group; false at the end of the file.
fn group_next(f: &XdFile, g: &mut Group) -> bool {
    if g.end == f.nrec() {
        return false;
    }
    g.start = g.end + 1;
    g.end = g.start;
    while f.chg(g.end) {
        g.end += 1;
    }
    true
}

/// Moves to the previous group; false at the start of the file.
fn group_previous(f: &XdFile, g: &mut Group) -> bool {
    if g.start == 0 {
        return false;
    }
    g.end = g.start - 1;
    g.start = g.end;
    while f.chg(g.start - 1) {
        g.start -= 1;
    }
    true
}

fn group_slide_down(f: &mut XdFile, g: &mut Group) -> bool {
    if g.end < f.nrec() && f.ha[g.start as usize] == f.ha[g.end as usize] {
        f.set_chg(g.start, false);
        g.start += 1;
        f.set_chg(g.end, true);
        g.end += 1;
        while f.chg(g.end) {
            g.end += 1;
        }
        true
    } else {
        false
    }
}

fn group_slide_up(f: &mut XdFile, g: &mut Group) -> bool {
    if g.start > 0 && f.ha[g.start as usize - 1] == f.ha[g.end as usize - 1] {
        g.start -= 1;
        f.set_chg(g.start, true);
        g.end -= 1;
        f.set_chg(g.end, false);
        while f.chg(g.start - 1) {
            g.start -= 1;
        }
        true
    } else {
        false
    }
}

/// Indentation width of a line (tabs to multiples of 8), or -1 for whitespace-only lines.
fn get_indent(rec: &[u8]) -> i32 {
    let mut ret = 0;
    for &c in rec {
        if !matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            return ret;
        } else if c == b' ' {
            ret += 1;
        } else if c == b'\t' {
            ret += 8 - ret % 8;
        }
        if ret >= MAX_INDENT {
            return MAX_INDENT;
        }
    }
    -1
}

#[derive(Default)]
struct SplitMeasurement {
    end_of_file: bool,
    indent: i32,
    pre_blank: i32,
    pre_indent: i32,
    post_blank: i32,
    post_indent: i32,
}

#[derive(Default, Clone, Copy)]
struct SplitScore {
    effective_indent: i32,
    penalty: i32,
}

fn measure_split(f: &XdFile, split: isize) -> SplitMeasurement {
    let mut m = SplitMeasurement::default();
    if split >= f.nrec() {
        m.end_of_file = true;
        m.indent = -1;
    } else {
        m.indent = get_indent(f.recs[split as usize]);
    }
    m.pre_indent = -1;
    let mut i = split - 1;
    while i >= 0 {
        m.pre_indent = get_indent(f.recs[i as usize]);
        if m.pre_indent != -1 {
            break;
        }
        m.pre_blank += 1;
        if m.pre_blank == MAX_BLANKS {
            m.pre_indent = 0;
            break;
        }
        i -= 1;
    }
    m.post_indent = -1;
    let mut i = split + 1;
    while i < f.nrec() {
        m.post_indent = get_indent(f.recs[i as usize]);
        if m.post_indent != -1 {
            break;
        }
        m.post_blank += 1;
        if m.post_blank == MAX_BLANKS {
            m.post_indent = 0;
            break;
        }
        i += 1;
    }
    m
}

fn score_add_split(m: &SplitMeasurement, s: &mut SplitScore) {
    if m.pre_indent == -1 && m.pre_blank == 0 {
        s.penalty += START_OF_FILE_PENALTY;
    }
    if m.end_of_file {
        s.penalty += END_OF_FILE_PENALTY;
    }
    let post_blank = if m.indent == -1 { 1 + m.post_blank } else { 0 };
    let total_blank = m.pre_blank + post_blank;
    s.penalty += TOTAL_BLANK_WEIGHT * total_blank;
    s.penalty += POST_BLANK_WEIGHT * post_blank;
    let indent = if m.indent != -1 { m.indent } else { m.post_indent };
    let any_blanks = total_blank != 0;
    s.effective_indent += indent;
    if indent == -1 || m.pre_indent == -1 {
    } else if indent > m.pre_indent {
        s.penalty += if any_blanks {
            RELATIVE_INDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_INDENT_PENALTY
        };
    } else if indent == m.pre_indent {
    } else if m.post_indent != -1 && m.post_indent > indent {
        s.penalty += if any_blanks {
            RELATIVE_OUTDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_OUTDENT_PENALTY
        };
    } else {
        s.penalty += if any_blanks {
            RELATIVE_DEDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_DEDENT_PENALTY
        };
    }
}

fn score_cmp(s1: &SplitScore, s2: &SplitScore) -> i32 {
    let cmp_indents = (s1.effective_indent > s2.effective_indent) as i32
        - (s1.effective_indent < s2.effective_indent) as i32;
    INDENT_WEIGHT * cmp_indents + (s1.penalty - s2.penalty)
}

/// `xdl_change_compact`: slides each group of changed lines in `f` to merge it with
/// neighbouring groups, align it with a change in `fo`, or (indent heuristic) place it at the
/// most readable position.
fn change_compact(f: &mut XdFile, fo: &XdFile, indent_heuristic: bool) {
    let mut g = group_init(f);
    let mut go = group_init(fo);
    loop {
        if g.end != g.start {
            let mut groupsize;
            let mut earliest_end;
            let mut end_matching_other;
            loop {
                groupsize = g.end - g.start;
                end_matching_other = -1;
                while group_slide_up(f, &mut g) {
                    let ok = group_previous(fo, &mut go);
                    assert!(ok, "xdiff: group sync broken sliding up");
                }
                earliest_end = g.end;
                if go.end > go.start {
                    end_matching_other = g.end;
                }
                while group_slide_down(f, &mut g) {
                    let ok = group_next(fo, &mut go);
                    assert!(ok, "xdiff: group sync broken sliding down");
                    if go.end > go.start {
                        end_matching_other = g.end;
                    }
                }
                if groupsize == g.end - g.start {
                    break;
                }
            }

            if g.end == earliest_end {
                // No shifting was possible.
            } else if end_matching_other != -1 {
                while go.end == go.start {
                    let ok = group_slide_up(f, &mut g) && group_previous(fo, &mut go);
                    assert!(ok, "xdiff: match disappeared");
                }
            } else if indent_heuristic {
                let mut shift = earliest_end;
                if g.end - groupsize - 1 > shift {
                    shift = g.end - groupsize - 1;
                }
                if g.end - INDENT_HEURISTIC_MAX_SLIDING > shift {
                    shift = g.end - INDENT_HEURISTIC_MAX_SLIDING;
                }
                let mut best_shift = -1;
                let mut best_score = SplitScore::default();
                while shift <= g.end {
                    let mut score = SplitScore::default();
                    score_add_split(&measure_split(f, shift), &mut score);
                    score_add_split(&measure_split(f, shift - groupsize), &mut score);
                    if best_shift == -1 || score_cmp(&score, &best_score) <= 0 {
                        best_score = score;
                        best_shift = shift;
                    }
                    shift += 1;
                }
                while g.end > best_shift {
                    let ok = group_slide_up(f, &mut g) && group_previous(fo, &mut go);
                    assert!(ok, "xdiff: best shift unreached");
                }
            }
        }
        if !group_next(f, &mut g) {
            break;
        }
        let ok = group_next(fo, &mut go);
        assert!(ok, "xdiff: group sync broken moving to next group");
    }
}

/// `xdl_build_script`, producing changes in file order.
fn build_script(f1: &XdFile, f2: &XdFile) -> Vec<Change> {
    let mut out = vec![];
    let (mut i1, mut i2) = (f1.nrec(), f2.nrec());
    while i1 >= 0 || i2 >= 0 {
        if f1.chg(i1 - 1) || f2.chg(i2 - 1) {
            let (l1, l2) = (i1, i2);
            while f1.chg(i1 - 1) {
                i1 -= 1;
            }
            while f2.chg(i2 - 1) {
                i2 -= 1;
            }
            out.push(Change {
                old: i1 as usize,
                old_len: (l1 - i1) as usize,
                new: i2 as usize,
                new_len: (l2 - i2) as usize,
            });
        }
        i1 -= 1;
        i2 -= 1;
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_block_prefers_deletion_first() {
        let a = split_lines(b"one\ntwo\nthree\nfour\nfive\nsix\n");
        let b = split_lines(b"four\nfive\nsix\none\ntwo\nthree\n");
        assert_eq!(
            diff(&a, &b),
            vec![
                Change { old: 0, old_len: 3, new: 0, new_len: 0 },
                Change { old: 6, old_len: 0, new: 3, new_len: 3 },
            ]
        );
    }

    #[test]
    fn empty_sides() {
        let e: Vec<&[u8]> = vec![];
        let a = split_lines(b"x\ny\n");
        assert_eq!(diff(&e, &e), vec![]);
        assert_eq!(diff(&e, &a), vec![Change { old: 0, old_len: 0, new: 0, new_len: 2 }]);
        assert_eq!(diff(&a, &e), vec![Change { old: 0, old_len: 2, new: 0, new_len: 0 }]);
    }
}
