//! Builds the web UI (`web/`) with pnpm and embeds the result (`web/dist`) into the binary.
//!
//! The UI is rebuilt only when its sources change. Without pnpm on `PATH`, or with
//! `LABRAT_SKIP_WEB_BUILD=1`, whatever is already in `web/dist` is embedded; when that is empty the
//! binary serves a page that says how to build the UI.

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The UI's inputs, relative to `web/`. Watching `dist/` itself would rerun this script after
/// every build, since the build writes it.
const INPUTS: &[&str] =
    &["src", "public", "index.html", "package.json", "pnpm-lock.yaml", "tsconfig.json", "vite.config.ts"];

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            collect(&path, root, out);
        } else {
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, path));
        }
    }
}

fn pnpm(web: &Path, args: &[&str]) {
    let program = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    let status = Command::new(program).args(args).current_dir(web).status();
    match status {
        Ok(s) if s.success() => {}
        Ok(s) => panic!(
            "`pnpm {}` in web/ failed ({s}). Fix the UI, or set LABRAT_SKIP_WEB_BUILD=1 to embed the existing web/dist.",
            args.join(" ")
        ),
        Err(e) => panic!("could not run pnpm: {e}"),
    }
}

fn has_pnpm() -> bool {
    let program = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    Command::new(program).arg("--version").output().is_ok_and(|o| o.status.success())
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let web = manifest.join("../../web");
    let dist = web.join("dist");
    println!("cargo:rerun-if-env-changed=LABRAT_SKIP_WEB_BUILD");

    let skip = std::env::var_os("LABRAT_SKIP_WEB_BUILD").is_some_and(|v| !v.is_empty() && v != "0");
    if !skip && web.join("package.json").exists() && has_pnpm() {
        for input in INPUTS {
            println!("cargo:rerun-if-changed={}", web.join(input).display());
        }
        pnpm(&web, &["install", "--frozen-lockfile"]);
        pnpm(&web, &["build"]);
    } else {
        // Embed a UI built by hand; pick it up when it is rebuilt.
        println!("cargo:rerun-if-changed={}", dist.display());
        if !skip {
            println!("cargo:warning=pnpm not found; embedding the existing web/dist (if any) without rebuilding the web UI");
        }
    }

    let mut files = vec![];
    collect(&dist, &dist, &mut files);
    files.sort();
    let mut code = String::from("pub static ASSETS: &[(&str, &[u8])] = &[\n");
    for (rel, path) in &files {
        let abs = path.canonicalize().unwrap();
        writeln!(code, "    ({rel:?}, include_bytes!({:?})),", abs.display().to_string()).unwrap();
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("web_assets.rs");
    std::fs::write(out, code).unwrap();
}
