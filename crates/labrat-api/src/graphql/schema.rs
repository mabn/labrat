//! GitLab's 18.11 GraphQL schema, loaded from its introspection result (embedded, gzipped).

use serde_json::Value;
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

static SCHEMA_CE: LazyLock<Schema> = LazyLock::new(|| {
    let v: Value = serde_json::from_slice(&RAW).expect("introspection json");
    Schema::from_introspection(v)
});

static SCHEMA_EE: LazyLock<Schema> = LazyLock::new(|| {
    let v: Value = serde_json::from_slice(&STATIC_RAW).expect("introspection json");
    Schema::from_introspection(v)
});

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
    fn from_json(v: &Value) -> TypeRef {
        match v["kind"].as_str() {
            Some("NON_NULL") => TypeRef::NonNull(Box::new(TypeRef::from_json(&v["ofType"]))),
            Some("LIST") => TypeRef::List(Box::new(TypeRef::from_json(&v["ofType"]))),
            _ => TypeRef::Named(v["name"].as_str().unwrap_or_default().to_string()),
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
    pub raw: Value,
    pub types: HashMap<String, TypeDef>,
    pub query_type: String,
    pub mutation_type: String,
}

fn inputs(v: &Value) -> Vec<InputDef> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| InputDef {
                    name: x["name"].as_str().unwrap_or_default().to_string(),
                    ty: TypeRef::from_json(&x["type"]),
                    default: x["defaultValue"].as_str().map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Schema {
    /// Type names of the given kinds, in introspection order.
    pub fn type_names(&self, kinds: &[&str]) -> Vec<String> {
        self.raw["__schema"]["types"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|t| kinds.contains(&t["kind"].as_str().unwrap_or_default()))
                    .filter_map(|t| t["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn from_introspection(v: Value) -> Schema {
        let root = v.get("data").cloned().unwrap_or(v);
        let schema = &root["__schema"];
        let mut types = HashMap::new();
        for t in schema["types"].as_array().into_iter().flatten() {
            let name = t["name"].as_str().unwrap_or_default().to_string();
            let fields = t["fields"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|f| FieldDef {
                            name: f["name"].as_str().unwrap_or_default().to_string(),
                            args: inputs(&f["args"]),
                            ty: TypeRef::from_json(&f["type"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let def = TypeDef {
                kind: t["kind"].as_str().unwrap_or_default().to_string(),
                name: name.clone(),
                fields,
                input_fields: inputs(&t["inputFields"]),
                possible_types: t["possibleTypes"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
                enum_values: t["enumValues"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
            };
            types.insert(name, def);
        }
        let query_type = schema["queryType"]["name"].as_str().unwrap_or("Query").to_string();
        let mutation_type = schema["mutationType"]["name"].as_str().unwrap_or("Mutation").to_string();
        Schema { raw: root, types, query_type, mutation_type }
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

    /// The raw introspection object of a type (for `__type(name:)`).
    pub fn raw_type(&self, name: &str) -> Option<&Value> {
        self.raw["__schema"]["types"].as_array()?.iter().find(|t| t["name"] == name)
    }
}
