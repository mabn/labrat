//! Builds the web UI (`web/`) with pnpm and embeds the result (`web/dist`) into the binary, and
//! converts GitLab's GraphQL introspection files into the compact form labrat loads its schemas
//! from (see `graphql_schemas`).
//!
//! The UI is rebuilt only when its sources change. Without pnpm on `PATH`, or with
//! `LABRAT_SKIP_WEB_BUILD=1`, whatever is already in `web/dist` is embedded; when that is empty the
//! binary serves a page that says how to build the UI.

use serde_json::Value;
use std::fmt::Write;
use std::io::{Read, Write as _};
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

/// A type reference in GraphQL notation: `[String!]!`.
fn type_ref(t: &Value) -> String {
    match t["kind"].as_str() {
        Some("NON_NULL") => format!("{}!", type_ref(&t["ofType"])),
        Some("LIST") => format!("[{}]", type_ref(&t["ofType"])),
        _ => t["name"].as_str().unwrap_or_default().to_string(),
    }
}

/// Keeps a value on one line: `\\`, `\t` and `\n` escapes.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn input_line(kind: char, i: &Value, out: &mut String) {
    let default = match i["defaultValue"].as_str() {
        Some(d) => format!("={}", escape(d)),
        None => String::new(),
    };
    writeln!(out, "{kind}\t{}\t{}\t{default}", i["name"].as_str().unwrap_or_default(), type_ref(&i["type"])).unwrap();
}

/// The parts of an introspection result that labrat's schema uses, one record per line, gzipped:
/// `Q`/`M` name the query and mutation types; `T kind name` starts a type, followed by its `F name
/// type` fields (each followed by its `A name type =default` arguments), `I name type =default`
/// input fields, `P name` possible types and `E name` enum values. Loading this at runtime takes
/// a few milliseconds; parsing the introspection JSON took tens.
fn graphql_schema(src: &Path, dest: &Path) {
    println!("cargo:rerun-if-changed={}", src.display());
    let mut json = vec![];
    flate2::read::GzDecoder::new(std::fs::File::open(src).unwrap()).read_to_end(&mut json).unwrap();
    let v: Value = serde_json::from_slice(&json).unwrap();
    let root = if v.get("data").is_some() { &v["data"] } else { &v };
    let schema = &root["__schema"];
    let mut out = String::new();
    writeln!(out, "Q\t{}", schema["queryType"]["name"].as_str().unwrap_or("Query")).unwrap();
    writeln!(out, "M\t{}", schema["mutationType"]["name"].as_str().unwrap_or("Mutation")).unwrap();
    for t in schema["types"].as_array().unwrap() {
        writeln!(out, "T\t{}\t{}", t["kind"].as_str().unwrap(), t["name"].as_str().unwrap()).unwrap();
        for f in t["fields"].as_array().into_iter().flatten() {
            writeln!(out, "F\t{}\t{}", f["name"].as_str().unwrap(), type_ref(&f["type"])).unwrap();
            for a in f["args"].as_array().into_iter().flatten() {
                input_line('A', a, &mut out);
            }
        }
        for i in t["inputFields"].as_array().into_iter().flatten() {
            input_line('I', i, &mut out);
        }
        for p in t["possibleTypes"].as_array().into_iter().flatten() {
            writeln!(out, "P\t{}", p["name"].as_str().unwrap()).unwrap();
        }
        for e in t["enumValues"].as_array().into_iter().flatten() {
            writeln!(out, "E\t{}", e["name"].as_str().unwrap()).unwrap();
        }
    }
    let mut gz = flate2::write::GzEncoder::new(std::fs::File::create(dest).unwrap(), flate2::Compression::default());
    gz.write_all(out.as_bytes()).unwrap();
    gz.finish().unwrap();
}

fn graphql_schemas(manifest: &Path, out_dir: &Path) {
    graphql_schema(&manifest.join("data/graphql-introspection.json.gz"), &out_dir.join("graphql-schema-ce.gz"));
    graphql_schema(&manifest.join("data/graphql-introspection-ee.json.gz"), &out_dir.join("graphql-schema-ee.gz"));
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    graphql_schemas(&manifest, &PathBuf::from(std::env::var("OUT_DIR").unwrap()));
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
