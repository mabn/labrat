//! A small GraphQL executor over GitLab's schema: field collection with fragments and
//! `@skip`/`@include`, argument coercion, value completion with null propagation, and
//! introspection answered from GitLab's own introspection data.

use super::schema::{FieldDef, SCHEMA, Schema, TypeRef};
use crate::ctx::Req;
use async_graphql_parser::Positioned;
use async_graphql_parser::types::{ExecutableDocument, Field, OperationDefinition, Selection, SelectionSet};
use async_graphql_value::{ConstValue, Value as GqlValue};
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use std::cell::RefCell;

/// A field resolution error, reported in `errors` with the field's path.
#[derive(Debug, Clone)]
pub struct FieldError {
    pub message: String,
    pub extensions: Option<Value>,
}

impl FieldError {
    pub fn new(msg: impl Into<String>) -> FieldError {
        FieldError { message: msg.into(), extensions: None }
    }
}

impl From<labrat_core::Error> for FieldError {
    fn from(e: labrat_core::Error) -> Self {
        FieldError::new(format!("{e}"))
    }
}

impl From<labrat_git::Error> for FieldError {
    fn from(e: labrat_git::Error) -> Self {
        FieldError::new(format!("{e}"))
    }
}

impl From<rusqlite::Error> for FieldError {
    fn from(e: rusqlite::Error) -> Self {
        FieldError::new(format!("{e}"))
    }
}

impl From<crate::ctx::ApiError> for FieldError {
    fn from(e: crate::ctx::ApiError) -> Self {
        let msg = e.body["message"].as_str().or(e.body["error"].as_str()).unwrap_or("error").to_string();
        FieldError::new(msg)
    }
}

pub type FResult<T> = Result<T, FieldError>;

/// What a resolver returns.
pub enum Out {
    Null,
    Val(Value),
    Obj(Box<dyn Object>),
    List(Vec<Out>),
}

impl Out {
    pub fn obj(o: impl Object + 'static) -> Out {
        Out::Obj(Box::new(o))
    }

    pub fn opt<T: Object + 'static>(o: Option<T>) -> Out {
        match o {
            Some(o) => Out::obj(o),
            None => Out::Null,
        }
    }

    pub fn val(v: impl Into<Value>) -> Out {
        let v = v.into();
        if v.is_null() { Out::Null } else { Out::Val(v) }
    }
}

pub trait Object {
    fn typename(&self) -> String;
    fn resolve(&self, cx: &Ctx, field: &str, args: &Args) -> FResult<Out>;
}

/// Coerced field arguments.
#[derive(Debug, Default, Clone)]
pub struct Args(pub Map<String, Value>);

impl Args {
    pub fn get(&self, k: &str) -> Option<&Value> {
        self.0.get(k).filter(|v| !v.is_null())
    }
    pub fn str(&self, k: &str) -> Option<String> {
        self.get(k).and_then(|v| v.as_str().map(str::to_string))
    }
    pub fn int(&self, k: &str) -> Option<i64> {
        self.get(k).and_then(Value::as_i64)
    }
    pub fn bool(&self, k: &str) -> Option<bool> {
        self.get(k).and_then(Value::as_bool)
    }
    pub fn has(&self, k: &str) -> bool {
        self.0.contains_key(k)
    }
}

pub struct Ctx<'a> {
    pub c: &'a Connection,
    pub req: &'a Req,
    pub schema: &'static Schema,
    doc: &'a ExecutableDocument,
    vars: Map<String, Value>,
    errors: RefCell<Vec<Value>>,
}

impl Ctx<'_> {
    pub fn user(&self) -> Option<&labrat_core::model::User> {
        self.req.user.as_ref()
    }
}

thread_local! {
    /// The query being executed, for error locations.
    pub static QUERY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Errors raised during execution: graphql-ruby reports the line, but the column as the
/// position counted from the start of the document (not of the line).
fn location(pos: async_graphql_parser::Pos) -> Value {
    let column = QUERY.with(|q| {
        let q = q.borrow();
        let before: usize = q.split('\n').take(pos.line.saturating_sub(1)).map(|l| l.chars().count() + 1).sum();
        before + pos.column
    });
    json!({"line": pos.line, "column": column})
}

fn const_to_json(v: &ConstValue) -> Value {
    match v {
        ConstValue::Null => Value::Null,
        ConstValue::Number(n) => serde_json::to_value(n).unwrap_or(Value::Null),
        ConstValue::String(s) => json!(s),
        ConstValue::Boolean(b) => json!(b),
        ConstValue::Binary(_) => Value::Null,
        ConstValue::Enum(e) => json!(e.as_str()),
        ConstValue::List(l) => Value::Array(l.iter().map(const_to_json).collect()),
        ConstValue::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.to_string(), const_to_json(v))).collect()),
    }
}

/// Parses a schema default value (GraphQL literal syntax) into JSON.
fn parse_default(lit: &str) -> Value {
    let q = format!("{{ f(a: {lit}) }}");
    let Ok(doc) = async_graphql_parser::parse_query(&q) else { return Value::Null };
    for (_, op) in doc.operations.iter() {
        if let Some(Positioned { node: Selection::Field(f), .. }) = op.node.selection_set.node.items.first()
            && let Some((_, v)) = f.node.arguments.first()
                && let Ok(c) = v.node.clone().into_const_with(|_| Err::<ConstValue, ()>(())) {
                    return const_to_json(&c);
                }
    }
    Value::Null
}

impl<'a> Ctx<'a> {
    fn value_to_json(&self, v: &GqlValue) -> Value {
        match v {
            GqlValue::Variable(name) => self.vars.get(name.as_str()).cloned().unwrap_or(Value::Null),
            GqlValue::Null => Value::Null,
            GqlValue::Number(n) => serde_json::to_value(n).unwrap_or(Value::Null),
            GqlValue::String(s) => json!(s),
            GqlValue::Boolean(b) => json!(b),
            GqlValue::Binary(_) => Value::Null,
            GqlValue::Enum(e) => json!(e.as_str()),
            GqlValue::List(l) => Value::Array(l.iter().map(|x| self.value_to_json(x)).collect()),
            GqlValue::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.to_string(), self.value_to_json(v))).collect()),
        }
    }

    /// Input coercion of one value to a declared type (IDs as strings, input object defaults).
    fn coerce(&self, ty: &TypeRef, v: Value) -> Value {
        match ty {
            TypeRef::NonNull(t) => self.coerce(t, v),
            TypeRef::List(t) => match v {
                Value::Null => Value::Null,
                Value::Array(a) => Value::Array(a.into_iter().map(|x| self.coerce(t, x)).collect()),
                other => Value::Array(vec![self.coerce(t, other)]),
            },
            TypeRef::Named(n) => match (n.as_str(), v) {
                (_, Value::Null) => Value::Null,
                ("ID", Value::Number(num)) => json!(num.to_string()),
                (name, Value::Object(o)) => match self.schema.get(name) {
                    Some(def) if def.kind == "INPUT_OBJECT" => {
                        let mut out = Map::new();
                        for f in &def.input_fields {
                            match o.get(&f.name) {
                                Some(x) => {
                                    out.insert(f.name.clone(), self.coerce(&f.ty, x.clone()));
                                }
                                None => {
                                    if let Some(d) = &f.default {
                                        out.insert(f.name.clone(), parse_default(d));
                                    }
                                }
                            }
                        }
                        Value::Object(out)
                    }
                    _ => Value::Object(o),
                },
                (_, other) => other,
            },
        }
    }

    fn args(&self, def: &FieldDef, f: &Field) -> Args {
        let mut m = Map::new();
        for a in &def.args {
            match f.get_argument(&a.name) {
                Some(v) => {
                    let raw = self.value_to_json(&v.node);
                    m.insert(a.name.clone(), self.coerce(&a.ty, raw));
                }
                None => {
                    if let Some(d) = &a.default {
                        m.insert(a.name.clone(), parse_default(d));
                    }
                }
            }
        }
        Args(m)
    }

    fn error(&self, msg: &str, pos: async_graphql_parser::Pos, path: &[Value], extensions: Option<Value>) {
        let mut e = Map::new();
        e.insert("message".into(), json!(msg));
        e.insert("locations".into(), json!([location(pos)]));
        e.insert("path".into(), Value::Array(path.to_vec()));
        if let Some(x) = extensions {
            e.insert("extensions".into(), x);
        }
        self.errors.borrow_mut().push(Value::Object(e));
    }

    fn directives_skip(&self, dirs: &[Positioned<async_graphql_parser::types::Directive>]) -> bool {
        for d in dirs {
            let name = d.node.name.node.as_str();
            let cond = d.node.get_argument("if").map(|v| self.value_to_json(&v.node)).and_then(|v| v.as_bool());
            match (name, cond) {
                ("skip", Some(true)) | ("include", Some(false)) => return true,
                _ => {}
            }
        }
        false
    }

    /// `CollectFields`: response keys in order, each with the fields merged under it.
    fn collect<'d>(&'d self, typename: &str, sel: &'d SelectionSet, out: &mut Vec<(String, Vec<&'d Positioned<Field>>)>) {
        self.collect_in(typename, sel, out, &mut std::collections::HashSet::new());
    }

    /// `CollectFields`: each fragment is collected once per selection set (`visitedFragments`).
    fn collect_in<'d>(&'d self, typename: &str, sel: &'d SelectionSet, out: &mut Vec<(String, Vec<&'d Positioned<Field>>)>, visited: &mut std::collections::HashSet<String>) {
        for item in &sel.items {
            if self.directives_skip(item.node.directives()) {
                continue;
            }
            match &item.node {
                Selection::Field(f) => {
                    let key = f.node.response_key().node.to_string();
                    match out.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, list)) => list.push(f),
                        None => out.push((key, vec![f])),
                    }
                }
                Selection::InlineFragment(fr) => {
                    let applies = match &fr.node.type_condition {
                        Some(tc) => self.schema.applies(tc.node.on.node.as_str(), typename),
                        None => true,
                    };
                    if applies {
                        self.collect_in(typename, &fr.node.selection_set.node, out, visited);
                    }
                }
                Selection::FragmentSpread(sp) => {
                    if visited.insert(sp.node.fragment_name.node.to_string())
                        && let Some(def) = self.doc.fragments.get(&sp.node.fragment_name.node)
                        && self.schema.applies(def.node.type_condition.node.on.node.as_str(), typename) {
                            self.collect_in(typename, &def.node.selection_set.node, out, visited);
                        }
                }
            }
        }
    }

    /// Executes a selection set on an object. `None` means a non-null field failed, so the
    /// object itself becomes null.
    fn execute_object(&self, obj: &dyn Object, sels: &[&SelectionSet], path: &[Value]) -> Option<Map<String, Value>> {
        let typename = obj.typename();
        let mut fields = vec![];
        for s in sels {
            self.collect(&typename, s, &mut fields);
        }
        let mut out = Map::new();
        for (key, group) in fields {
            let f = &group[0].node;
            let name = f.name.node.as_str();
            let mut fpath = path.to_vec();
            fpath.push(json!(key));
            if name == "__typename" {
                out.insert(key, json!(typename));
                continue;
            }
            let Some(def) = self.schema.get(&typename).and_then(|t| t.field(name)) else {
                self.error(&format!("Field '{name}' doesn't exist on type '{typename}'"), group[0].pos, &fpath, None);
                out.insert(key, Value::Null);
                continue;
            };
            let args = self.args(def, f);
            let subs: Vec<&SelectionSet> = group.iter().map(|g| &g.node.selection_set.node).collect();
            let completed = match obj.resolve(self, name, &args) {
                Ok(o) => self.complete(&typename, &def.ty, o, &subs, &fpath, group[0]),
                Err(e) => {
                    self.error(&e.message, group[0].pos, &fpath, e.extensions);
                    if matches!(def.ty, TypeRef::NonNull(_)) { None } else { Some(Value::Null) }
                }
            };
            match completed {
                Some(v) => {
                    out.insert(key, v);
                }
                None if !matches!(def.ty, TypeRef::NonNull(_)) => {
                    out.insert(key, Value::Null);
                }
                None => return None,
            }
        }
        Some(out)
    }

    /// `CompleteValue`. `None` propagates a non-null violation to the nearest nullable parent.
    fn complete(&self, parent: &str, ty: &TypeRef, out: Out, sels: &[&SelectionSet], path: &[Value], field: &Positioned<Field>) -> Option<Value> {
        match ty {
            TypeRef::NonNull(inner) => match self.complete(parent, inner, out, sels, path, field) {
                Some(Value::Null) => {
                    self.error(
                        &format!("Cannot return null for non-nullable field {parent}.{}", field.node.name.node),
                        field.pos,
                        path,
                        None,
                    );
                    None
                }
                other => other,
            },
            TypeRef::List(inner) => match out {
                Out::Null => Some(Value::Null),
                Out::Val(Value::Array(a)) => self.complete(parent, ty, Out::List(a.into_iter().map(Out::val).collect()), sels, path, field),
                Out::List(items) => {
                    let mut arr = vec![];
                    for (i, it) in items.into_iter().enumerate() {
                        let mut p = path.to_vec();
                        p.push(json!(i));
                        // A null element of a `[T!]` list: graphql-ruby's element message.
                        if let TypeRef::NonNull(elem) = inner.as_ref() {
                            match self.complete(parent, elem, it, sels, &p, field)? {
                                Value::Null => {
                                    let msg = format!("Cannot return null for non-nullable element of type '{}' for {parent}.{}", elem.named(), field.node.name.node);
                                    self.error(&msg, field.pos, &p, None);
                                    return None;
                                }
                                v => arr.push(v),
                            }
                            continue;
                        }
                        arr.push(self.complete(parent, inner, it, sels, &p, field)?);
                    }
                    Some(Value::Array(arr))
                }
                other => self.complete(parent, inner, other, sels, path, field).map(|v| Value::Array(vec![v])),
            },
            TypeRef::Named(_) => match out {
                Out::Null => Some(Value::Null),
                Out::Val(v) => Some(v),
                // A null from a failed non-null child propagates without a second error.
                Out::Obj(o) => self.execute_object(o.as_ref(), sels, path).map(Value::Object),
                Out::List(items) => Some(Value::Array(
                    items
                        .into_iter()
                        .map(|i| self.complete(parent, ty, i, sels, path, field).unwrap_or(Value::Null))
                        .collect(),
                )),
            },
        }
    }
}

/// Runs one operation; returns the response body (`data` and/or `errors`).
pub fn run(
    c: &Connection,
    req: &Req,
    doc: &ExecutableDocument,
    op: &Positioned<OperationDefinition>,
    variables: Map<String, Value>,
    root: &dyn Object,
) -> Value {
    let schema: &'static Schema = &SCHEMA;
    let mut cx = Ctx { c, req, schema, doc, vars: Map::new(), errors: RefCell::new(vec![]) };
    let mut vars = Map::new();
    for vd in &op.node.variable_definitions {
        let name = vd.node.name.node.to_string();
        let ty = TypeRef::from_ast(&vd.node.var_type.node);
        let v = match variables.get(&name) {
            Some(v) => v.clone(),
            None => vd.node.default_value.as_ref().map(|d| const_to_json(&d.node)).unwrap_or(Value::Null),
        };
        vars.insert(name, cx.coerce(&ty, v));
    }
    cx.vars = vars;
    let data = cx.execute_object(root, &[&op.node.selection_set.node], &[]);
    let errors = cx.errors.into_inner();
    // graphql-ruby serializes `errors` before `data`.
    let mut body = Map::new();
    if !errors.is_empty() {
        body.insert("errors".into(), Value::Array(errors));
    }
    body.insert("data".into(), data.map(Value::Object).unwrap_or(Value::Null));
    Value::Object(body)
}
