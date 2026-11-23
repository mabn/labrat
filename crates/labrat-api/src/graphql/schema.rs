//! GitLab's 18.11 GraphQL schema, loaded from its introspection result (embedded, gzipped).

use std::collections::HashMap;
use std::io::Read;
use std::sync::LazyLock;

/// CE's runtime schema (`scripts/reference/dump-graphql-schema.rb` on the CE instance). labrat
/// validates and executes queries against it when running as CE.
static INTROSPECTION_GZ: &[u8] = include_bytes!("../../data/graphql-introspection.json.gz");
/// The static introspection file GitLab 18.11 serves, fetched through the API. It is the same
/// file on CE and EE, byte for byte, and describes EE's schema, which labrat uses as EE.
static STATIC_INTROSPECTION_GZ: &[u8] = include_bytes!("../../data/graphql-introspection-ee.json.gz");

fn gunzip(data: &[u8]) -> Vec<u8> {
    let mut b = vec![];
    flate2::read::GzDecoder::new(data).read_to_end(&mut b).expect("introspection data");
    b
}

static RAW: LazyLock<Vec<u8>> = LazyLock::new(|| gunzip(INTROSPECTION_GZ));
static STATIC_RAW: LazyLock<Vec<u8>> = LazyLock::new(|| gunzip(STATIC_INTROSPECTION_GZ));

/// The answer to an introspection query: GitLab serves its static file, whatever was asked.
pub fn introspection_bytes() -> &'static [u8] {
    if current_edition().is_ee() { &STATIC_RAW } else { &RAW }
}

/// The schemas in the compact form `build.rs` makes from the two files above.
static SCHEMA_CE_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/graphql-schema-ce.gz"));
static SCHEMA_EE_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/graphql-schema-ee.gz"));

static SCHEMA_CE: LazyLock<Schema> = LazyLock::new(|| Schema::from_compact(&gunzip(SCHEMA_CE_GZ)));
static SCHEMA_EE: LazyLock<Schema> = LazyLock::new(|| Schema::from_compact(&gunzip(SCHEMA_EE_GZ)));

thread_local! {
    static EDITION: std::cell::Cell<labrat_core::Edition> = const { std::cell::Cell::new(labrat_core::Edition::Ce) };
}

/// Runs `f` with `edition`'s schema as the current one. GraphQL requests run synchronously on
/// one thread, so the schema in force is a thread-local.
pub fn with_edition<T>(edition: labrat_core::Edition, f: impl FnOnce() -> T) -> T {
    let prev = EDITION.with(|e| e.replace(edition));
    let out = f();
    EDITION.with(|e| e.set(prev));
    out
}

fn current_edition() -> labrat_core::Edition {
    EDITION.with(|e| e.get())
}

/// The schema of the edition in force (see `with_edition`).
pub fn schema() -> &'static Schema {
    if current_edition().is_ee() { &SCHEMA_EE } else { &SCHEMA_CE }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

impl TypeRef {
    /// Parses GraphQL notation: `[String!]!`.
    fn parse(s: &str) -> TypeRef {
        if let Some(inner) = s.strip_suffix('!') {
            TypeRef::NonNull(Box::new(TypeRef::parse(inner)))
        } else if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            TypeRef::List(Box::new(TypeRef::parse(inner)))
        } else {
            TypeRef::Named(s.to_string())
        }
    }

    pub fn named(&self) -> &str {
        match self {
            TypeRef::Named(n) => n,
            TypeRef::List(t) | TypeRef::NonNull(t) => t.named(),
        }
    }

    /// GraphQL notation: `[String!]!`.
    pub fn display(&self) -> String {
        match self {
            TypeRef::Named(n) => n.clone(),
            TypeRef::List(t) => format!("[{}]", t.display()),
            TypeRef::NonNull(t) => format!("{}!", t.display()),
        }
    }

    pub fn from_ast(t: &async_graphql_parser::types::Type) -> TypeRef {
        use async_graphql_parser::types::BaseType;
        let base = match &t.base {
            BaseType::Named(n) => TypeRef::Named(n.to_string()),
            BaseType::List(inner) => TypeRef::List(Box::new(TypeRef::from_ast(inner))),
        };
        if t.nullable { base } else { TypeRef::NonNull(Box::new(base)) }
    }
}

#[derive(Debug, Clone)]
pub struct InputDef {
    pub name: String,
    pub ty: TypeRef,
    pub default: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub args: Vec<InputDef>,
    pub ty: TypeRef,
}

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub kind: String,
    pub name: String,
    pub fields: Vec<FieldDef>,
    pub input_fields: Vec<InputDef>,
    pub possible_types: Vec<String>,
    pub enum_values: Vec<String>,
}

impl TypeDef {
    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|f| f.name == name)
    }
}

pub struct Schema {
    /// (kind, name) of every type, in introspection order.
    order: Vec<(String, String)>,
    pub types: HashMap<String, TypeDef>,
    pub query_type: String,
    pub mutation_type: String,
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// An `A` or `I` record: name, type, `=default` or nothing.
fn input(mut parts: std::str::Split<'_, char>) -> InputDef {
    let name = parts.next().unwrap_or_default().to_string();
    let ty = TypeRef::parse(parts.next().unwrap_or_default());
    let default = parts.next().and_then(|d| d.strip_prefix('=')).map(unescape);
    InputDef { name, ty, default }
}

impl Schema {
    /// Type names of the given kinds, in introspection order.
    pub fn type_names(&self, kinds: &[&str]) -> Vec<String> {
        self.order.iter().filter(|(k, _)| kinds.contains(&k.as_str())).map(|(_, n)| n.clone()).collect()
    }

    /// Loads the compact form `build.rs` writes (described there).
    fn from_compact(bytes: &[u8]) -> Schema {
        let text = std::str::from_utf8(bytes).expect("compact schema");
        let mut order: Vec<(String, String)> = vec![];
        let mut defs: Vec<TypeDef> = vec![];
        let (mut query_type, mut mutation_type) = (String::from("Query"), String::from("Mutation"));
        for line in text.lines() {
            let mut parts = line.split('\t');
            let tag = parts.next().unwrap_or_default();
            match tag {
                "Q" => query_type = parts.next().unwrap_or_default().to_string(),
                "M" => mutation_type = parts.next().unwrap_or_default().to_string(),
                "T" => {
                    let kind = parts.next().unwrap_or_default().to_string();
                    let name = parts.next().unwrap_or_default().to_string();
                    order.push((kind.clone(), name.clone()));
                    defs.push(TypeDef { kind, name, fields: vec![], input_fields: vec![], possible_types: vec![], enum_values: vec![] });
                }
                _ => {
                    let Some(t) = defs.last_mut() else { continue };
                    match tag {
                        "F" => {
                            let name = parts.next().unwrap_or_default().to_string();
                            let ty = TypeRef::parse(parts.next().unwrap_or_default());
                            t.fields.push(FieldDef { name, args: vec![], ty });
                        }
                        "A" => {
                            if let Some(f) = t.fields.last_mut() {
                                f.args.push(input(parts));
                            }
                        }
                        "I" => t.input_fields.push(input(parts)),
                        "P" => t.possible_types.push(parts.next().unwrap_or_default().to_string()),
                        "E" => t.enum_values.push(parts.next().unwrap_or_default().to_string()),
                        _ => {}
                    }
                }
            }
        }
        let types = defs.into_iter().map(|t| (t.name.clone(), t)).collect();
        Schema { order, types, query_type, mutation_type }
    }

    pub fn get(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    /// Whether an object of type `concrete` satisfies a fragment on `cond`.
    pub fn applies(&self, cond: &str, concrete: &str) -> bool {
        if cond == concrete {
            return true;
        }
        self.get(cond).is_some_and(|t| t.possible_types.iter().any(|p| p == concrete))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn type_ref(v: &Value) -> TypeRef {
        match v["kind"].as_str() {
            Some("NON_NULL") => TypeRef::NonNull(Box::new(type_ref(&v["ofType"]))),
            Some("LIST") => TypeRef::List(Box::new(type_ref(&v["ofType"]))),
            _ => TypeRef::Named(v["name"].as_str().unwrap().to_string()),
        }
    }

    fn inputs(v: &Value) -> Vec<(String, TypeRef, Option<String>)> {
        v.as_array().into_iter().flatten().map(|i| (i["name"].as_str().unwrap().to_string(), type_ref(&i["type"]), i["defaultValue"].as_str().map(String::from))).collect()
    }

    fn names(v: &Value) -> Vec<String> {
        v.as_array().into_iter().flatten().map(|x| x["name"].as_str().unwrap().to_string()).collect()
    }

    /// The compact schema `build.rs` makes holds everything labrat reads from the introspection
    /// file, unchanged.
    #[test]
    fn compact_schema_matches_introspection() {
        for (json, schema) in [(&*RAW, &*SCHEMA_CE), (&*STATIC_RAW, &*SCHEMA_EE)] {
            let v: Value = serde_json::from_slice(json).unwrap();
            let root = if v.get("data").is_some() { &v["data"] } else { &v };
            let types = root["__schema"]["types"].as_array().unwrap();
            assert_eq!(schema.types.len(), types.len());
            assert_eq!(schema.query_type, root["__schema"]["queryType"]["name"].as_str().unwrap());
            assert_eq!(schema.mutation_type, root["__schema"]["mutationType"]["name"].as_str().unwrap());
            let order: Vec<(String, String)> = types.iter().map(|t| (t["kind"].as_str().unwrap().to_string(), t["name"].as_str().unwrap().to_string())).collect();
            assert_eq!(schema.order, order);
            for t in types {
                let def = schema.get(t["name"].as_str().unwrap()).unwrap();
                assert_eq!(def.kind, t["kind"].as_str().unwrap());
                let fields: Vec<_> = t["fields"].as_array().into_iter().flatten().map(|f| (f["name"].as_str().unwrap().to_string(), type_ref(&f["type"]), inputs(&f["args"]))).collect();
                let got: Vec<_> = def.fields.iter().map(|f| (f.name.clone(), f.ty.clone(), f.args.iter().map(|a| (a.name.clone(), a.ty.clone(), a.default.clone())).collect::<Vec<_>>())).collect();
                assert_eq!(got, fields, "{}", def.name);
                let got: Vec<_> = def.input_fields.iter().map(|a| (a.name.clone(), a.ty.clone(), a.default.clone())).collect();
                assert_eq!(got, inputs(&t["inputFields"]), "{}", def.name);
                assert_eq!(def.possible_types, names(&t["possibleTypes"]), "{}", def.name);
                assert_eq!(def.enum_values, names(&t["enumValues"]), "{}", def.name);
            }
        }
    }
}
