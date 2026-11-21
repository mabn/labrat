//! Static validation with graphql-ruby's messages (the common rules integrations hit).

use super::schema::{Schema, TypeRef};
use async_graphql_parser::Positioned;
use async_graphql_parser::types::{ExecutableDocument, OperationDefinition, OperationType, Selection, SelectionSet};
use serde_json::{Map, Value, json};

/// A parse error in graphql-ruby's shape.
pub fn parse_error(query: &str, e: &async_graphql_parser::Error) -> Value {
    use async_graphql_parser::Error as PE;
    let at = |p: &async_graphql_parser::Pos| json!({"line": p.line, "column": p.column});
    match e {
        PE::OperationDuplicated { operation, first, second } => {
            return json!({"errors": [{"message": format!("Operation name \"{operation}\" must be unique"), "locations": [at(first), at(second)], "path": [], "extensions": {"code": "uniquelyNamedOperations", "operationName": operation.as_str()}}]});
        }
        PE::FragmentDuplicated { fragment, first, second } => {
            return json!({"errors": [{"message": format!("Fragment name \"{fragment}\" must be unique"), "locations": [at(first), at(second)], "path": [], "extensions": {"code": "fragmentNotUnique", "fragmentName": fragment.as_str()}}]});
        }
        PE::MultipleOperations { anonymous, .. } => {
            return json!({"errors": [{"message": "Operation name is required when multiple operations are present", "locations": [at(anonymous)], "path": [], "extensions": {"code": "uniquelyNamedOperations"}}]});
        }
        _ => {}
    }
    let positions: Vec<Value> = e.positions().map(|p| json!({"line": p.line, "column": p.column})).collect();
    let _ = query;
    // Truncated documents: graphql-ruby reports the last token's position.
    let trimmed = query.trim_end();
    let at_end = positions.first().is_some_and(|p| {
        let line = p["line"].as_u64().unwrap_or(1) as usize;
        let col = p["column"].as_u64().unwrap_or(1) as usize;
        let text = query.lines().nth(line - 1).unwrap_or("");
        line >= query.lines().count() && text.chars().skip(col - 1).all(char::is_whitespace)
    });
    if at_end {
        let line = trimmed.lines().count().max(1);
        let last = trimmed.lines().last().unwrap_or("");
        let start = last
            .char_indices()
            .rev()
            .find(|(i, c)| !(c.is_alphanumeric() || *c == '_') && *i < last.len())
            .map(|(i, c)| if c.is_alphanumeric() || c == '_' { i } else if last[i + c.len_utf8()..].is_empty() { i } else { i + c.len_utf8() })
            .unwrap_or(0);
        let col = last[..start].chars().count() + 1;
        return json!({"errors": [{"message": format!("Expected NAME, not end of file at [{line}, {col}]"), "locations": [{"line": line, "column": col}]}]});
    }
    let msg = match positions.first() {
        Some(p) => format!("Parse error on {} at [{}, {}]", describe_token(query, p), p["line"], p["column"]),
        None => "Unexpected end of document".to_string(),
    };
    json!({"errors": [{"message": msg, "locations": positions}]})
}

fn describe_token(query: &str, pos: &Value) -> String {
    let line = pos["line"].as_u64().unwrap_or(1) as usize;
    let col = pos["column"].as_u64().unwrap_or(1) as usize;
    let text = query.lines().nth(line - 1).unwrap_or("");
    let rest: String = text.chars().skip(col - 1).collect();
    let tok: String = match rest.chars().next() {
        None => return "end of file".into(),
        Some(c) if c.is_alphanumeric() || c == '_' => rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect(),
        Some(c) => c.to_string(),
    };
    let kind = match tok.as_str() {
        "{" => "LCURLY",
        "}" => "RCURLY",
        "(" => "LPAREN",
        ")" => "RPAREN",
        "[" => "LBRACKET",
        "]" => "RBRACKET",
        ":" => "COLON",
        "$" => "VAR_SIGN",
        "!" => "BANG",
        "=" => "EQUALS",
        "@" => "DIR_SIGN",
        _ if tok.chars().all(|c| c.is_ascii_digit()) => "INT",
        _ => "IDENTIFIER",
    };
    format!("\"{tok}\" ({kind})")
}

/// Literal argument checks: GitLab's GID scalars and plain scalar types.
fn literal_problem(schema: &Schema, ty: &TypeRef, v: &async_graphql_value::Value, field: &str, arg: &str) -> Option<(String, &'static str)> {
    use async_graphql_value::Value as V;
    if matches!(v, V::Variable(_)) {
        return None;
    }
    let named = ty.named();
    let gid_model = named.strip_suffix("ID").filter(|m| !m.is_empty() && schema.get(named).is_some_and(|t| t.kind == "SCALAR"));
    if let Some(model) = gid_model {
        let raw = match v {
            V::String(s) => s.clone(),
            V::Null => return None,
            other => {
                return Some((format!("{other} is not a valid Global ID"), "CoercionError"));
            }
        };
        let Some(rest) = raw.strip_prefix("gid://") else {
            return Some((format!("\"{raw}\" is not a valid Global ID"), "CoercionError"));
        };
        let Some(rest) = rest.strip_prefix("gitlab/") else {
            return Some((format!("\"{raw}\" is not a Gitlab Global ID"), "CoercionError"));
        };
        let Some((kind, id)) = rest.split_once('/') else {
            return Some((format!("\"{raw}\" is not a valid Global ID"), "CoercionError"));
        };
        if id.is_empty() {
            return Some((format!("\"{raw}\" is not a valid Global ID"), "CoercionError"));
        }
        let ok = match model {
            "Noteable" => matches!(kind, "MergeRequest" | "Issue" | "Commit" | "Snippet" | "WorkItem"),
            "Awardable" => matches!(kind, "MergeRequest" | "Note" | "DiffNote" | "DiscussionNote" | "Issue" | "Snippet" | "WorkItem"),
            "Note" => matches!(kind, "Note" | "DiffNote" | "DiscussionNote" | "LegacyDiffNote" | "SyntheticNote"),
            "Discussion" => matches!(kind, "Discussion" | "DiffDiscussion" | "IndividualNoteDiscussion"),
            "Label" => matches!(kind, "Label" | "ProjectLabel" | "GroupLabel"),
            // Namespaced models drop `::` in the scalar name (`Users::SavedReply` -> `UsersSavedReplyID`).
            m => {
                kind == m
                    || kind.replace("::", "") == m
                    // Abstract types (`TodoableID`, `IssuableID`): any of the possible types; an
                    // unknown model isn't checked.
                    || match schema.get(m) {
                        Some(t) if t.kind == "INTERFACE" || t.kind == "UNION" => t.possible_types.iter().any(|x| x == kind),
                        Some(_) => false,
                        None => true,
                    }
            }
        };
        if !ok {
            return Some((format!("\"{raw}\" does not represent an instance of {model}"), "CoercionError"));
        }
        return None;
    }
    let base = match ty {
        TypeRef::NonNull(t) => t.as_ref(),
        t => t,
    };
    let bad = match (base, v) {
        (_, V::Null) => matches!(ty, TypeRef::NonNull(_)),
        (TypeRef::Named(n), V::Number(_)) if n == "String" => true,
        (TypeRef::Named(n), V::String(_)) if n == "Int" || n == "Boolean" || n == "Float" => true,
        (TypeRef::Named(n), V::Boolean(_)) if n == "String" || n == "Int" || n == "ID" => true,
        (TypeRef::Named(n), V::Enum(e)) => match schema.get(n) {
            Some(t) if t.kind == "ENUM" => !t.enum_values.iter().any(|x| x == e.as_str()),
            _ => true,
        },
        _ => false,
    };
    if bad {
        return Some((format!("Argument '{arg}' on Field '{field}' has an invalid value ({v}). Expected type '{}'.", ty.display()), "Field"));
    }
    None
}

/// An invalid scalar or enum literal inside an input object: the message and the path of keys.
fn input_object_problem(schema: &Schema, ty: &TypeRef, v: &async_graphql_value::Value) -> Option<(String, Vec<String>)> {
    use async_graphql_value::Value as V;
    let V::Object(fields) = v else { return None };
    let t = schema.get(ty.named()).filter(|t| t.kind == "INPUT_OBJECT")?;
    for (k, fv) in fields {
        let Some(fd) = t.input_fields.iter().find(|f| f.name == k.as_str()) else { continue };
        if let Some((msg, mut path)) = input_object_problem(schema, &fd.ty, fv) {
            path.insert(0, k.to_string());
            return Some((msg, path));
        }
        let base = match &fd.ty {
            TypeRef::NonNull(t) => t.as_ref(),
            t => t,
        };
        let bad = match (base, fv) {
            (_, V::Variable(_)) => false,
            (_, V::Null) => matches!(fd.ty, TypeRef::NonNull(_)),
            (TypeRef::Named(n), V::Number(_)) if n == "String" => true,
            (TypeRef::Named(n), V::String(_)) if n == "Int" || n == "Boolean" || n == "Float" => true,
            (TypeRef::Named(n), V::Boolean(_)) if n == "String" || n == "Int" || n == "ID" => true,
            (TypeRef::Named(n), V::Enum(e)) => match schema.get(n) {
                Some(et) if et.kind == "ENUM" => !et.enum_values.iter().any(|x| x == e.as_str()),
                _ => true,
            },
            _ => false,
        };
        if bad {
            return Some((
                format!("Argument '{k}' on InputObject '{}' has an invalid value ({fv}). Expected type '{}'.", t.name, fd.ty.display()),
                vec![k.to_string()],
            ));
        }
    }
    None
}

/// `RequiredInputObjectAttributesArePresent`: a non-null input field without a default that a
/// literal input object leaves out. Returns the field, its type, the input type and the path.
fn missing_input_field(schema: &Schema, ty: &TypeRef, v: &async_graphql_value::Value) -> Option<(String, String, String, Vec<String>)> {
    use async_graphql_value::Value as V;
    let V::Object(fields) = v else { return None };
    let t = schema.get(ty.named()).filter(|t| t.kind == "INPUT_OBJECT")?;
    for fd in &t.input_fields {
        if matches!(fd.ty, TypeRef::NonNull(_)) && fd.default.is_none() && !fields.contains_key(fd.name.as_str()) {
            return Some((fd.name.clone(), fd.ty.display(), t.name.clone(), vec![fd.name.clone()]));
        }
    }
    for (k, fv) in fields {
        let Some(fd) = t.input_fields.iter().find(|f| f.name == k.as_str()) else { continue };
        if let Some((name, ty, input, mut path)) = missing_input_field(schema, &fd.ty, fv) {
            path.insert(0, k.to_string());
            return Some((name, ty, input, path));
        }
    }
    None
}

struct V<'a> {
    schema: &'a Schema,
    doc: &'a ExecutableDocument,
    errors: Vec<Value>,
    query: &'a str,
    /// Declared variables: type and whether a default is given.
    vars: Vec<(String, TypeRef, bool)>,
    /// Fragments being expanded (cycles are reported separately and never followed).
    expanding: Vec<String>,
    /// How messages name the operation: `anonymous query` or its name.
    op_desc: String,
    /// Fragments whose bodies were validated (once each, not once per spread: a fragment
    /// spreading another twice per level would otherwise take exponential time).
    validated: std::collections::HashSet<String>,
    /// The declared variable names.
    declared: std::collections::HashSet<String>,
    /// Positions of fields the schema doesn't define: variables used only there aren't "used".
    undefined_fields: std::collections::HashSet<(usize, usize)>,
}

/// The directives of GitLab's schema: locations and arguments (name, required).
fn directive_def(name: &str) -> Option<(&'static [&'static str], &'static [(&'static str, bool)])> {
    match name {
        "skip" | "include" => Some((&["FIELD", "FRAGMENT_SPREAD", "INLINE_FRAGMENT"], &[("if", true)])),
        "gl_introduced" => Some((&["FIELD", "INLINE_FRAGMENT"], &[("version", true)])),
        "deprecated" => Some((&["FIELD_DEFINITION", "ENUM_VALUE", "ARGUMENT_DEFINITION", "INPUT_FIELD_DEFINITION"], &[("reason", false)])),
        "granularScope" => Some((&["FIELD_DEFINITION", "OBJECT"], &[("permissions", true), ("boundaryType", true), ("boundary", false), ("boundaryArgument", false)])),
        "oneOf" => Some((&["INPUT_OBJECT"], &[])),
        "specifiedBy" => Some((&["SCALAR"], &[("url", true)])),
        _ => None,
    }
}

/// `DirectivesAreInValidLocations::LOCATION_MESSAGE_NAMES`.
fn location_name(l: &str) -> String {
    match l {
        "QUERY" => "queries".into(),
        "MUTATION" => "mutations".into(),
        "SUBSCRIPTION" => "subscriptions".into(),
        "FIELD" => "fields".into(),
        "FRAGMENT_DEFINITION" => "fragment definitions".into(),
        "FRAGMENT_SPREAD" => "fragment spreads".into(),
        "INLINE_FRAGMENT" => "inline fragments".into(),
        "VARIABLE_DEFINITION" => "variable definitions".into(),
        other => format!("{}s", other.to_lowercase().replace('_', " ")),
    }
}

/// The types an object of type `t` can be (itself, or an abstract type's possible types).
fn possible(schema: &Schema, t: &str) -> Vec<String> {
    match schema.get(t) {
        Some(d) if d.kind == "OBJECT" => vec![t.to_string()],
        Some(d) => d.possible_types.clone(),
        None => vec![],
    }
}

/// The type a variable's type must be compatible with (`VariableUsagesAreAllowed`).
fn list_depth(t: &TypeRef) -> usize {
    match t {
        TypeRef::Named(_) => 0,
        TypeRef::List(i) => 1 + list_depth(i),
        TypeRef::NonNull(i) => list_depth(i),
    }
}

/// Line and column (1-based) of a byte offset.
fn line_col(q: &str, off: usize) -> (usize, usize) {
    let before = &q[..off.min(q.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map(|l| l.chars().count()).unwrap_or(0) + 1;
    (line, col)
}

impl V<'_> {
    fn err(&mut self, msg: String, pos: async_graphql_parser::Pos, path: &[String], ext: Value) {
        let mut m = Map::new();
        m.insert("message".into(), json!(msg));
        m.insert("locations".into(), json!([{"line": pos.line, "column": pos.column}]));
        m.insert("path".into(), json!(path));
        m.insert("extensions".into(), ext);
        self.errors.push(Value::Object(m));
    }

    /// Checks variables used in an argument value against the expected types.
    fn var_usages(&mut self, ty: &TypeRef, has_default: bool, v: &async_graphql_value::Value, key: &str, from: async_graphql_parser::Pos, path: &[String]) {
        use async_graphql_value::Value as GV;
        match v {
            GV::Variable(name) => {
                let Some((_, vty, vdefault)) = self.vars.iter().find(|(n, _, _)| n == name.as_str()).cloned() else { return };
                let problem = if vty.named() != ty.named() {
                    Some("Type mismatch")
                } else if list_depth(&vty) != list_depth(ty) {
                    Some("List dimension mismatch")
                } else if matches!(ty, TypeRef::NonNull(_)) && !matches!(vty, TypeRef::NonNull(_)) && !vdefault && !has_default {
                    Some("Nullability mismatch")
                } else {
                    None
                };
                if let Some(kind) = problem {
                    // Nested keys have no positions in the AST: find `key: $name` after the argument.
                    let start = line_offset(self.query, from);
                    let pat = format!("${name}");
                    let pos = self.query[start..]
                        .match_indices(&pat)
                        .map(|(i, _)| start + i)
                        .find_map(|i| {
                            let before = self.query[..i].trim_end().strip_suffix(':')?.trim_end();
                            before.ends_with(key).then(|| before.len() - key.len())
                        })
                        .unwrap_or(start);
                    let (line, column) = line_col(self.query, pos);
                    let mut m = Map::new();
                    m.insert("message".into(), json!(format!("{kind} on variable ${name} and argument {key} ({} / {})", vty.display(), ty.display())));
                    m.insert("locations".into(), json!([{"line": line, "column": column}]));
                    m.insert("path".into(), json!(path));
                    m.insert(
                        "extensions".into(),
                        json!({"code": "variableMismatch", "variableName": name.as_str(), "typeName": vty.display(), "argumentName": key, "errorMessage": kind}),
                    );
                    self.errors.push(Value::Object(m));
                }
            }
            GV::Object(fields) => {
                let Some(t) = self.schema.get(ty.named()).cloned() else { return };
                for (k, fv) in fields {
                    if let Some(fd) = t.input_fields.iter().find(|f| f.name == k.as_str()) {
                        let mut p = path.to_vec();
                        p.push(k.to_string());
                        self.var_usages(&fd.ty, fd.default.is_some(), fv, k.as_str(), from, &p);
                    }
                }
            }
            GV::List(items) => {
                let inner = match ty {
                    TypeRef::NonNull(t) => match t.as_ref() {
                        TypeRef::List(i) => i.as_ref().clone(),
                        o => o.clone(),
                    },
                    TypeRef::List(i) => i.as_ref().clone(),
                    o => o.clone(),
                };
                for it in items {
                    self.var_usages(&inner, false, it, key, from, path);
                }
            }
            _ => {}
        }
    }

    /// `DirectivesAreDefined`, `DirectivesAreInValidLocations`, `UniqueDirectivesPerLocation`
    /// and the directives' arguments.
    fn directives(&mut self, ds: &[Positioned<async_graphql_parser::types::Directive>], location: &str, path: &[String]) {
        let mut seen: std::collections::HashMap<String, async_graphql_parser::Pos> = Default::default();
        for d in ds {
            let name = d.node.name.node.to_string();
            let Some((locations, args)) = directive_def(&name) else {
                let names = ["deprecated", "gl_introduced", "granularScope", "include", "oneOf", "skip", "specifiedBy"];
                let hint = super::did_you_mean::suggestion(&name, &names);
                self.err(format!("Directive @{name} is not defined{hint}"), d.pos, path, json!({"code": "undefinedDirective", "directiveName": name}));
                continue;
            };
            if !locations.contains(&location) {
                let allowed: Vec<String> = locations.iter().map(|l| location_name(l)).collect();
                let target = location_name(location);
                self.err(
                    format!("'@{name}' can't be applied to {target} (allowed: {})", allowed.join(", ")),
                    d.pos,
                    path,
                    json!({"code": "directiveCannotBeApplied", "targetName": target, "name": name}),
                );
                continue;
            }
            if let Some(first) = seen.get(&name) {
                let first = *first;
                let mut m = Map::new();
                m.insert("message".into(), json!(format!("The directive \"{name}\" can only be used once at this location.")));
                m.insert("locations".into(), json!([{"line": first.line, "column": first.column}, {"line": d.pos.line, "column": d.pos.column}]));
                m.insert("path".into(), json!(path));
                m.insert("extensions".into(), json!({"code": "directiveNotUniqueForLocation", "directiveName": name}));
                self.errors.push(Value::Object(m));
                continue;
            }
            seen.insert(name.clone(), d.pos);
            let arg_errors_start = self.errors.len();
            for (an, _) in &d.node.arguments {
                if !args.iter().any(|(a, _)| *a == an.node.as_str()) {
                    let mut q = path.to_vec();
                    q.push(an.node.to_string());
                    self.err(
                        format!("Directive '{name}' doesn't accept argument '{}'{}", an.node, super::did_you_mean::suggestion(an.node.as_str(), &args.iter().map(|(a, _)| *a).collect::<Vec<_>>())),
                        an.pos,
                        &q,
                        json!({"code": "argumentNotAccepted", "name": name, "typeName": "Directive", "argumentName": an.node.as_str()}),
                    );
                }
            }
            let missing: Vec<&str> = args.iter().filter(|(a, req)| *req && d.node.get_argument(a).is_none()).map(|(a, _)| *a).collect();
            if !missing.is_empty() {
                let list = missing.join(", ");
                self.err(
                    format!("Directive '{name}' is missing required arguments: {list}"),
                    d.pos,
                    path,
                    json!({"code": "missingRequiredArguments", "className": "Directive", "name": name, "arguments": list}),
                );
                let e = self.errors.pop().unwrap();
                self.errors.insert(arg_errors_start, e);
            }
        }
    }

    /// `VariablesAreUsedAndDefined`: variables in arguments that the operation doesn't declare.
    fn undeclared(&mut self, args: &[(Positioned<async_graphql_value::Name>, Positioned<async_graphql_value::Value>)], path: &[String]) {
        for (an, av) in args {
            let mut used = std::collections::HashSet::new();
            value_variables(&av.node, &mut used);
            let mut used: Vec<String> = used.into_iter().collect();
            used.sort();
            for name in used {
                if !self.declared.contains(&name) {
                    let mut q = path.to_vec();
                    q.push(an.node.to_string());
                    let msg = format!("Variable ${name} is used by {} but not declared", self.op_desc);
                    self.err(msg, av.pos, &q, json!({"code": "variableNotDefined", "variableName": name}));
                }
            }
        }
    }

    /// `ArgumentNamesAreUnique`.
    fn unique_arguments(&mut self, args: &[(Positioned<async_graphql_value::Name>, Positioned<async_graphql_value::Value>)], path: &[String]) {
        let mut seen: std::collections::HashMap<&str, (async_graphql_parser::Pos, usize)> = Default::default();
        for (an, _) in args {
            let e = seen.entry(an.node.as_str()).or_insert((an.pos, 0));
            e.1 += 1;
            if e.1 != 2 {
                continue;
            }
            let first = Positioned::new((), e.0);
            let mut m = Map::new();
            m.insert("message".into(), json!(format!("There can be only one argument named \"{}\"", an.node)));
            m.insert("locations".into(), json!([{"line": first.pos.line, "column": first.pos.column}, {"line": an.pos.line, "column": an.pos.column}]));
            m.insert("path".into(), json!(path));
            m.insert("extensions".into(), json!({"code": "argumentNotUnique", "name": an.node.as_str()}));
            self.errors.push(Value::Object(m));
        }
    }

    /// Fields of a selection set with fragments flattened: (response key, name, args, pos).
    fn flatten(&self, sel: &SelectionSet, out: &mut Vec<(String, String, String, async_graphql_parser::Pos)>, seen: &mut Vec<String>) {
        for item in &sel.items {
            match &item.node {
                Selection::Field(f) => {
                    let args: Vec<String> = f.node.arguments.iter().map(|(n, v)| format!("{}:\"{}\"", n.node, v.node)).collect();
                    out.push((f.node.response_key().node.to_string(), f.node.name.node.to_string(), format!("{{{}}}", args.join(", ")), f.pos));
                }
                Selection::InlineFragment(fr) => self.flatten(&fr.node.selection_set.node, out, seen),
                Selection::FragmentSpread(sp) => {
                    let n = sp.node.fragment_name.node.to_string();
                    if !seen.contains(&n)
                        && let Some(d) = self.doc.fragments.get(&sp.node.fragment_name.node)
                    {
                        seen.push(n);
                        self.flatten(&d.node.selection_set.node, out, seen);
                    }
                }
            }
        }
    }

    /// `FieldsWillMerge` for one level: one response key, different fields or arguments.
    fn field_conflicts(&mut self, sel: &SelectionSet) {
        let mut fields = vec![];
        let mut seen = self.expanding.clone();
        self.flatten(sel, &mut fields, &mut seen);
        // Each response key's fields are compared with its first one.
        let mut first: std::collections::HashMap<&str, usize> = Default::default();
        let mut reported: std::collections::HashSet<&str> = Default::default();
        for (j, (k2, n2, a2, p2)) in fields.iter().enumerate() {
            let i = *first.entry(k2.as_str()).or_insert(j);
            if i == j || reported.contains(k2.as_str()) {
                continue;
            }
            let (key, name, args, pos) = &fields[i];
            {
                let (kind, conflicts) = if n2 != name {
                    ("a field", format!("{name} or {n2}"))
                } else if a2 != args {
                    ("an argument", format!("{args} or {a2}"))
                } else {
                    continue;
                };
                let mut m = Map::new();
                m.insert("message".into(), json!(format!("Field '{key}' has {kind} conflict: {conflicts}?")));
                m.insert("locations".into(), json!([{"line": pos.line, "column": pos.column}, {"line": p2.line, "column": p2.column}]));
                m.insert("path".into(), json!([]));
                m.insert("extensions".into(), json!({"code": "fieldConflict", "fieldName": key, "conflicts": conflicts}));
                self.errors.push(Value::Object(m));
                reported.insert(k2.as_str());
            }
        }
    }

    fn selection_set(&mut self, typename: &str, sel: &SelectionSet, path: &[String]) {
        self.field_conflicts(sel);
        for item in &sel.items {
            match &item.node {
                Selection::Field(f) => {
                    let name = f.node.name.node.as_str();
                    let mut p = path.to_vec();
                    p.push(f.node.response_key().node.to_string());
                    self.unique_arguments(&f.node.arguments, &p);
                    self.undeclared(&f.node.arguments, &p);
                    self.directives(&f.node.directives, "FIELD", &p);
                    if name == "__typename" {
                        continue;
                    }
                    // Dynamic introspection is disabled in GitLab's production schema.
                    let def = {
                        match self.schema.get(typename).and_then(|t| t.field(name)) {
                            Some(d) => Some(d.clone()),
                            None => {
                                self.undefined_fields.insert((f.pos.line, f.pos.column));
                                let fields: Vec<&str> = match self.schema.get(typename) {
                                    Some(t) if t.kind != "SCALAR" && t.kind != "ENUM" => t.fields.iter().map(|f| f.name.as_str()).collect(),
                                    _ => vec![],
                                };
                                let hint = super::did_you_mean::suggestion(name, &fields);
                                self.err(
                                    format!("Field '{name}' doesn't exist on type '{typename}'{hint}"),
                                    f.pos,
                                    &p,
                                    json!({"code": "undefinedField", "typeName": typename, "fieldName": name}),
                                );
                                continue;
                            }
                        }
                    };
                    let (ret, sub_type) = match (&def, name) {
                        (Some(d), _) => (d.ty.clone(), d.ty.named().to_string()),
                        (None, "__schema") => (TypeRef::Named("__Schema".into()), "__Schema".to_string()),
                        _ => (TypeRef::Named("__Type".into()), "__Type".to_string()),
                    };
                    if let Some(d) = &def {
                        let arg_errors_start = self.errors.len();
                        for (an, av) in &f.node.arguments {
                            if let Some(adef) = d.args.iter().find(|a| a.name == an.node.as_str()) {
                                let mut q = p.clone();
                                q.push(an.node.to_string());
                                // `RequiredInputObjectAttributesArePresent` comes before
                                // `ArgumentsAreDefined`: a missing key is reported first, both are.
                                let missing_first = missing_input_field(self.schema, &adef.ty, &av.node);
                                if let Some((input, key, sub)) = unknown_input_field(self.schema, &adef.ty, &av.node) {
                                    if let Some((field, fty, minput, msub)) = &missing_first {
                                        let mut mq = q.clone();
                                        mq.extend(msub.clone());
                                        self.err(
                                            format!("Argument '{field}' on InputObject '{minput}' is required. Expected type {fty}"),
                                            av.pos,
                                            &mq,
                                            json!({"code": "missingRequiredInputObjectAttribute", "argumentName": field, "argumentType": fty, "inputObjectType": minput}),
                                        );
                                    }
                                    q.extend(sub);
                                    // Keys have no positions in the AST: find `key:` after the argument.
                                    let start = line_offset(self.query, av.pos);
                                    let pos = self.query[start..]
                                        .match_indices(key.as_str())
                                        .map(|(i, _)| start + i)
                                        .find(|&i| self.query[i + key.len()..].trim_start().starts_with(':'))
                                        .map(|i| {
                                            let (line, column) = line_col(self.query, i);
                                            async_graphql_parser::Pos { line, column }
                                        })
                                        .unwrap_or(av.pos);
                                    self.err(
                                        {
                                            let names: Vec<String> = self.schema.get(&input).map(|t| t.input_fields.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
                                            let names: Vec<&str> = names.iter().map(String::as_str).collect();
                                            format!("InputObject '{input}' doesn't accept argument '{key}'{}", super::did_you_mean::suggestion(&key, &names))
                                        },
                                        pos,
                                        &q,
                                        json!({"code": "argumentNotAccepted", "name": input, "typeName": "InputObject", "argumentName": key}),
                                    );
                                    continue;
                                }
                                if let Some((field, fty, input, sub)) = missing_input_field(self.schema, &adef.ty, &av.node) {
                                    q.extend(sub);
                                    self.err(
                                        format!("Argument '{field}' on InputObject '{input}' is required. Expected type {fty}"),
                                        av.pos,
                                        &q,
                                        json!({"code": "missingRequiredInputObjectAttribute", "argumentName": field, "argumentType": fty, "inputObjectType": input}),
                                    );
                                    continue;
                                }
                                if let Some((msg, sub)) = input_object_problem(self.schema, &adef.ty, &av.node) {
                                    let arg_name = sub.last().cloned().unwrap_or_default();
                                    q.extend(sub);
                                    self.err(msg, av.pos, &q, json!({"code": "argumentLiteralsIncompatible", "typeName": "InputObject", "argumentName": arg_name}));
                                    continue;
                                }
                                let before = self.errors.len();
                                self.var_usages(&adef.ty.clone(), adef.default.is_some(), &av.node, an.node.as_str(), an.pos, &q);
                                if self.errors.len() > before {
                                    continue;
                                }
                                if let Some((msg, code_type)) = literal_problem(self.schema, &adef.ty, &av.node, name, an.node.as_str()) {
                                    // Coercion errors point at the field, type mismatches at the argument.
                                    let pos = if code_type == "CoercionError" { f.pos } else { an.pos };
                                    let ext = if code_type == "CoercionError" {
                                        json!({"code": "argumentLiteralsIncompatible", "typeName": code_type})
                                    } else {
                                        json!({"code": "argumentLiteralsIncompatible", "typeName": code_type, "argumentName": an.node.as_str()})
                                    };
                                    self.err(msg, pos, &q, ext);
                                    continue;
                                }
                            }
                            if !d.args.iter().any(|a| a.name == an.node.as_str()) {
                                self.err(
                                    {
                                        let names: Vec<&str> = d.args.iter().map(|a| a.name.as_str()).collect();
                                        format!("Field '{name}' doesn't accept argument '{}'{}", an.node, super::did_you_mean::suggestion(an.node.as_str(), &names))
                                    },
                                    an.pos,
                                    &{
                                        let mut q = p.clone();
                                        q.push(an.node.to_string());
                                        q
                                    },
                                    json!({"code": "argumentNotAccepted", "name": name, "typeName": "Field", "argumentName": an.node.as_str()}),
                                );
                            }
                        }
                        let missing: Vec<String> = d
                            .args
                            .iter()
                            .filter(|a| matches!(a.ty, TypeRef::NonNull(_)) && a.default.is_none())
                            .filter(|a| f.node.get_argument(&a.name).is_none())
                            .map(|a| a.name.clone())
                            .collect();
                        if !missing.is_empty() {
                            let list = missing.join(", ");
                            self.err(
                                format!("Field '{name}' is missing required arguments: {list}"),
                                f.pos,
                                &p,
                                json!({"code": "missingRequiredArguments", "className": "Field", "name": name, "arguments": list}),
                            );
                            // `RequiredArgumentsArePresent` runs on the field, before its arguments.
                            let e = self.errors.pop().unwrap();
                            self.errors.insert(arg_errors_start, e);
                        }
                    }
                    let kind = self.schema.get(&sub_type).map(|t| t.kind.clone()).unwrap_or_default();
                    let leaf = matches!(kind.as_str(), "SCALAR" | "ENUM");
                    let has_sel = !f.node.selection_set.node.items.is_empty();
                    if leaf && has_sel {
                        let names: Vec<String> = f
                            .node
                            .selection_set
                            .node
                            .items
                            .iter()
                            .filter_map(|s| match &s.node {
                                Selection::Field(x) => Some(format!("\"{}\"", x.node.name.node)),
                                Selection::FragmentSpread(x) => Some(format!("\"{}\"", x.node.fragment_name.node)),
                                _ => None,
                            })
                            .collect();
                        self.err(
                            format!(
                                "Selections can't be made on {} (field '{name}' returns {} but has selections [{}])",
                                if kind == "ENUM" { "enums" } else { "scalars" },
                                ret.named(),
                                names.join(", ")
                            ),
                            f.pos,
                            &p,
                            json!({"code": "selectionMismatch", "nodeName": format!("field '{name}'"), "typeName": ret.named()}),
                        );
                    } else if !leaf && !has_sel {
                        self.err(
                            format!(
                                "Field must have selections (field '{name}' returns {} but has no selections. Did you mean '{name} {{ ... }}'?)",
                                ret.named()
                            ),
                            f.pos,
                            &p,
                            json!({"code": "selectionMismatch", "nodeName": format!("field '{name}'"), "typeName": ret.named()}),
                        );
                    } else if has_sel {
                        self.selection_set(&sub_type, &f.node.selection_set.node, &p);
                    }
                }
                Selection::InlineFragment(fr) => {
                    let t = fr.node.type_condition.as_ref().map(|t| t.node.on.node.to_string()).unwrap_or(typename.to_string());
                    let mut p = path.to_vec();
                    p.push(format!("... on {t}"));
                    self.directives(&fr.node.directives, "INLINE_FRAGMENT", path);
                    if self.schema.get(&t).is_none() {
                        let names = self.schema.type_names(&["OBJECT", "INTERFACE"]);
                        let names: Vec<&str> = names.iter().map(String::as_str).collect();
                        self.err(
                            format!("No such type {t}, so it can't be a fragment condition{}", super::did_you_mean::suggestion(&t, &names)),
                            fr.pos,
                            &p,
                            json!({"code": "undefinedType", "typeName": t}),
                        );
                        continue;
                    }
                    self.selection_set(&t, &fr.node.selection_set.node, &p);
                }
                Selection::FragmentSpread(sp) => {
                    let n = sp.node.fragment_name.node.to_string();
                    let mut p = path.to_vec();
                    p.push(format!("... {n}"));
                    self.directives(&sp.node.directives, "FRAGMENT_SPREAD", path);
                    match self.doc.fragments.get(&sp.node.fragment_name.node) {
                        // A fragment on an unknown type is reported at its definition.
                        Some(def) if self.schema.get(def.node.type_condition.node.on.node.as_str()).is_none() => {}
                        Some(def) => {
                            let t = def.node.type_condition.node.on.node.to_string();
                            // `FragmentSpreadsArePossible`: checked at every spread.
                            let (inner, outer) = (possible(self.schema, &t), possible(self.schema, typename));
                            if !inner.is_empty() && !outer.is_empty() && !inner.iter().any(|x| outer.contains(x)) {
                                self.err(
                                    format!("Fragment {n} on {t} can't be spread inside {typename}"),
                                    sp.pos,
                                    &p,
                                    json!({"code": "cannotSpreadFragment", "typeName": t, "fragmentName": format!(" {n}"), "parentName": typename}),
                                );
                                continue;
                            }
                            if self.expanding.contains(&n) || self.validated.contains(&n) {
                                continue;
                            }
                            let sel = def.node.selection_set.node.clone();
                            self.expanding.push(n.clone());
                            self.validated.insert(n.clone());
                            self.selection_set(&t, &sel, &p);
                            self.expanding.pop();
                        }
                        None => self.err(
                            format!("Fragment {n} was used, but not defined"),
                            sp.pos,
                            &p,
                            json!({"code": "useAndDefineFragment", "fragmentName": n}),
                        ),
                    }
                }
            }
        }
    }
}

/// Validates the chosen operation; `Some(errors)` if it must not run.
pub fn validate(query: &str, doc: &ExecutableDocument, op: &Positioned<OperationDefinition>, variables: &Map<String, Value>) -> Option<Vec<Value>> {
    let schema: &Schema = super::schema::schema();
    let vars = op
        .node
        .variable_definitions
        .iter()
        .map(|vd| (vd.node.name.node.to_string(), TypeRef::from_ast(&vd.node.var_type.node), vd.node.default_value.is_some()))
        .collect();
    let op_desc = match doc.operations.iter().find(|(_, o)| std::ptr::eq(*o, op)).and_then(|(n, _)| n.map(|n| n.to_string())) {
        Some(n) => n,
        None => format!(
            "anonymous {}",
            match op.node.ty {
                OperationType::Mutation => "mutation",
                OperationType::Subscription => "subscription",
                OperationType::Query => "query",
            }
        ),
    };
    let mut v = V { schema, doc, errors: vec![], query, vars, expanding: vec![], op_desc, validated: Default::default(), declared: Default::default(), undefined_fields: Default::default() };
    v.declared = v.vars.iter().map(|(n, _, _)| n.clone()).collect();
    // `FragmentsAreFinite`: every fragment that can reach itself, in document order.
    let mut frags: Vec<_> = doc.fragments.iter().collect();
    frags.sort_by_key(|(_, f)| (f.pos.line, f.pos.column));
    for (name, f) in &frags {
        if reaches(doc, &f.node.selection_set.node, name.as_str(), &mut std::collections::HashSet::new()) {
            v.errors.push(json!({
                "message": format!("Fragment {name} contains an infinite loop"),
                "locations": [{"line": f.pos.line, "column": f.pos.column}],
                "path": [format!("fragment {name}")],
                "extensions": {"code": "infiniteLoop", "fragmentName": name.as_str()},
            }));
        }
    }
    let root = match op.node.ty {
        OperationType::Mutation => schema.mutation_type.clone(),
        OperationType::Subscription => "Subscription".to_string(),
        OperationType::Query => schema.query_type.clone(),
    };
    let op_label = match op.node.ty {
        OperationType::Mutation => "mutation",
        OperationType::Subscription => "subscription",
        OperationType::Query => "query",
    };
    let name = doc.operations.iter().find(|(_, o)| std::ptr::eq(*o, op)).and_then(|(n, _)| n.map(|n| n.to_string()));
    let root_path = match name.clone() {
        Some(n) => format!("{op_label} {n}"),
        None => op_label.to_string(),
    };
    for vd in &op.node.variable_definitions {
        let ty = TypeRef::from_ast(&vd.node.var_type.node);
        let vname = vd.node.name.node.to_string();
        let given = variables.get(&vname);
        let problems = if given.is_none_or(Value::is_null) {
            // A non-null variable needs a value even when it declares a default.
            if matches!(ty, TypeRef::NonNull(_)) {
                vec![json!({"path": [], "explanation": "Expected value to not be null"})]
            } else {
                vec![]
            }
        } else {
            let mut out = vec![];
            variable_problems(schema, &ty, given.unwrap(), &mut vec![], &mut out);
            out
        };
        if !problems.is_empty() {
            // `VariableValidationError#problem_fields`: problems below the top level are listed.
            let fields: Vec<String> = problems
                .iter()
                .filter(|p| p["path"].as_array().is_some_and(|a| !a.is_empty()))
                .map(|p| {
                    let path: Vec<String> = p["path"].as_array().unwrap().iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).collect();
                    format!("{} ({})", path.join("."), p["explanation"].as_str().unwrap_or(""))
                })
                .collect();
            let suffix = if fields.is_empty() { String::new() } else { format!(" for {}", fields.join(", ")) };
            v.errors.push(json!({
                "message": format!("Variable ${vname} of type {} was provided invalid value{suffix}", ty.display()),
                "locations": [{"line": vd.pos.line, "column": vd.pos.column}],
                "extensions": {"value": given.cloned().unwrap_or(Value::Null), "problems": problems},
            }));
        }
    }
    // `VariableNamesAreUnique` and `VariablesAreInputTypes`.
    let mut first_def: std::collections::HashMap<String, async_graphql_parser::Pos> = Default::default();
    for vd in op.node.variable_definitions.iter() {
        let vname = vd.node.name.node.to_string();
        let prior = first_def.get(&vname).copied();
        first_def.entry(vname.clone()).or_insert(vd.pos);
        if let Some(first) = prior.map(|pos| Positioned::new((), pos)) {
            v.errors.push(json!({
                "message": format!("There can only be one variable named \"{vname}\""),
                "locations": [{"line": first.pos.line, "column": first.pos.column}, {"line": vd.pos.line, "column": vd.pos.column}],
                "path": [root_path],
                "extensions": {"code": "variableNotUnique", "variableName": vname},
            }));
        }
        let ty = TypeRef::from_ast(&vd.node.var_type.node);
        if schema.get(ty.named()).is_none() {
            let names = schema.type_names(&["SCALAR", "ENUM", "INPUT_OBJECT"]);
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            v.errors.push(json!({
                "message": format!("{} isn't a defined input type (on ${vname}){}", ty.named(), super::did_you_mean::suggestion(ty.named(), &names)),
                "locations": [{"line": vd.pos.line, "column": vd.pos.column}],
                "path": [root_path],
                "extensions": {"code": "variableRequiresValidType", "typeName": ty.named(), "variableName": vname},
            }));
        }
        if let Some(t) = schema.get(ty.named())
            && !matches!(t.kind.as_str(), "SCALAR" | "ENUM" | "INPUT_OBJECT")
        {
            v.errors.push(json!({
                "message": format!("{} isn't a valid input type (on ${vname})", ty.named()),
                "locations": [{"line": vd.pos.line, "column": vd.pos.column}],
                "path": [root_path],
                "extensions": {"code": "variableRequiresValidType", "typeName": ty.named(), "variableName": vname},
            }));
        }
    }
    let op_location = match op.node.ty {
        OperationType::Mutation => "MUTATION",
        OperationType::Subscription => "SUBSCRIPTION",
        OperationType::Query => "QUERY",
    };
    v.directives(&op.node.directives, op_location, std::slice::from_ref(&root_path));
    v.selection_set(&root, &op.node.selection_set.node, std::slice::from_ref(&root_path));
    // Fragment definitions come after the operation in the document.
    // `FragmentsAreUsed` (over every operation) and `FragmentsAreOnCompositeTypes`.
    let mut spread = std::collections::HashSet::new();
    for (_, o) in doc.operations.iter() {
        let mut seen = std::collections::HashSet::new();
        spread_names(doc, &o.node.selection_set.node, &mut spread, &mut seen);
    }
    for (name, f) in &frags {
        let t = f.node.type_condition.node.on.node.to_string();
        if schema.get(&t).is_none() {
            let names = schema.type_names(&["OBJECT", "INTERFACE"]);
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            v.errors.push(json!({
                "message": format!("No such type {t}, so it can't be a fragment condition{}", super::did_you_mean::suggestion(&t, &names)),
                "locations": [{"line": f.pos.line, "column": f.pos.column}],
                "path": [format!("fragment {name}")],
                "extensions": {"code": "undefinedType", "typeName": t},
            }));
            continue;
        }
        if let Some(td) = schema.get(&t)
            && !matches!(td.kind.as_str(), "OBJECT" | "INTERFACE" | "UNION")
        {
            v.errors.push(json!({
                "message": format!("Invalid fragment on type {t} (must be Union, Interface or Object)"),
                "locations": [{"line": f.pos.line, "column": f.pos.column}],
                "path": [format!("fragment {name}")],
                "extensions": {"code": "fragmentOnNonCompositeType", "typeName": t},
            }));
        }
        if !spread.contains(name.as_str()) {
            v.errors.push(json!({
                "message": format!("Fragment {name} was defined, but not used"),
                "locations": [{"line": f.pos.line, "column": f.pos.column}],
                "path": [format!("fragment {name}")],
                "extensions": {"code": "useAndDefineFragment", "fragmentName": name.as_str()},
            }));
        }
    }
    // `VariablesAreUsedAndDefined`: a declared variable nothing refers to.
    let mut used = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    used_variables(doc, &op.node.selection_set.node, &mut used, &mut seen, &v.undefined_fields);
    for d in &op.node.directives {
        for (_, a) in &d.node.arguments {
            value_variables(&a.node, &mut used);
        }
    }
    let declared_by = match name {
        Some(n) => n.to_string(),
        None => format!("anonymous {op_label}"),
    };
    for vd in &op.node.variable_definitions {
        let vname = vd.node.name.node.to_string();
        if !used.contains(&vname) {
            v.errors.push(json!({
                "message": format!("Variable ${vname} is declared by {declared_by} but not used"),
                // graphql-ruby points at the operation.
                "locations": [{"line": op.pos.line, "column": op.pos.column}],
                "path": [root_path],
                "extensions": {"code": "variableNotUsed", "variableName": vname},
            }));
        }
    }
    // Fragments are checked again where they are spread: report each error once.
    let mut seen = std::collections::HashSet::new();
    let mut unique: Vec<Value> = v.errors.into_iter().filter(|e| seen.insert(e.to_string())).collect();
    // `GitlabSchema.validate_max_errors 5`: validation stops at the fifth error.
    unique.truncate(MAX_ERRORS);
    if unique.is_empty() { None } else { Some(unique) }
}

const MAX_ERRORS: usize = 5;

/// Byte offset of a parser position.
fn line_offset(q: &str, pos: async_graphql_parser::Pos) -> usize {
    let mut off = 0;
    for (i, line) in q.split_inclusive('\n').enumerate() {
        if i + 1 == pos.line {
            return off + line.char_indices().nth(pos.column.saturating_sub(1)).map(|(b, _)| b).unwrap_or(0);
        }
        off += line.len();
    }
    off
}

/// Ruby's `inspect` of a JSON value, as graphql-ruby puts it into coercion explanations.
fn ruby_inspect(v: &Value) -> String {
    match v {
        Value::Null => "nil".into(),
        Value::Array(a) => format!("[{}]", a.iter().map(ruby_inspect).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter().map(|(k, x)| format!("{:?}=>{}", k, ruby_inspect(x))).collect::<Vec<_>>().join(", ")
        ),
        x => x.to_string(),
    }
}

/// graphql-ruby's `validate_input` for a variable value: every problem with its path.
fn variable_problems(schema: &Schema, ty: &TypeRef, v: &Value, path: &mut Vec<Value>, out: &mut Vec<Value>) {
    let mut problem = |path: &Vec<Value>, e: String| out.push(json!({"path": path, "explanation": e}));
    match ty {
        TypeRef::NonNull(inner) => {
            if v.is_null() {
                problem(path, "Expected value to not be null".into());
            } else {
                variable_problems(schema, inner, v, path, out);
            }
        }
        _ if v.is_null() => {}
        TypeRef::List(inner) => match v {
            Value::Array(items) => {
                for (i, x) in items.iter().enumerate() {
                    path.push(json!(i));
                    variable_problems(schema, inner, x, path, out);
                    path.pop();
                }
            }
            // A single value stands for a list of one.
            x => variable_problems(schema, inner, x, path, out),
        },
        TypeRef::Named(n) => {
            let bad = |what: &str| format!("Could not coerce value {} to {what}", ruby_inspect(v));
            match n.as_str() {
                "String" if !v.is_string() => problem(path, bad("String")),
                "Int" if !(v.is_i64() || v.is_u64()) => problem(path, bad("Int")),
                "Float" if !v.is_number() => problem(path, bad("Float")),
                "Boolean" if !v.is_boolean() => problem(path, bad("Boolean")),
                "ID" if !(v.is_string() || v.is_i64() || v.is_u64()) => problem(path, bad("ID")),
                _ => match schema.get(n) {
                    // GitLab's GlobalID scalars (`UserID`, `NoteableID`, ...).
                    Some(t) if t.kind == "SCALAR" && n.len() > 2 && n.ends_with("ID") => {
                        let lit = async_graphql_value::ConstValue::from_json(v.clone()).map(|c| c.into_value());
                        if let Ok(lit) = lit
                            && let Some((msg, "CoercionError")) = literal_problem(schema, ty, &lit, "", "")
                        {
                            out.push(json!({"path": path, "explanation": msg, "message": msg}));
                        }
                    }
                    Some(t) if t.kind == "ENUM" => {
                        if !v.as_str().is_some_and(|e| t.enum_values.iter().any(|x| x == e)) {
                            problem(path, format!("Expected {} to be one of: {}", ruby_inspect(v), t.enum_values.join(", ")));
                        }
                    }
                    Some(t) if t.kind == "INPUT_OBJECT" => {
                        let Value::Object(fields) = v else {
                            problem(path, format!("Expected {} to be a key-value object.", ruby_inspect(v)));
                            return;
                        };
                        for k in fields.keys() {
                            if !t.input_fields.iter().any(|f| &f.name == k) {
                                path.push(json!(k));
                                problem(path, format!("Field is not defined on {}", t.name));
                                path.pop();
                            }
                        }
                        for fd in &t.input_fields {
                            path.push(json!(fd.name));
                            match fields.get(&fd.name) {
                                Some(x) => variable_problems(schema, &fd.ty, x, path, out),
                                None if matches!(fd.ty, TypeRef::NonNull(_)) && fd.default.is_none() => {
                                    out.push(json!({"path": path, "explanation": "Expected value to not be null"}));
                                }
                                None => {}
                            }
                            path.pop();
                        }
                    }
                    _ => {}
                },
            }
        }
    }
}

fn value_variables(v: &async_graphql_value::Value, out: &mut std::collections::HashSet<String>) {
    use async_graphql_value::Value as GV;
    match v {
        GV::Variable(n) => {
            out.insert(n.to_string());
        }
        GV::List(items) => items.iter().for_each(|x| value_variables(x, out)),
        GV::Object(fields) => fields.values().for_each(|x| value_variables(x, out)),
        _ => {}
    }
}

/// Every variable referenced in a selection set, its directives and the fragments it spreads.
fn used_variables(
    doc: &ExecutableDocument,
    sel: &SelectionSet,
    out: &mut std::collections::HashSet<String>,
    seen: &mut std::collections::HashSet<String>,
    skip: &std::collections::HashSet<(usize, usize)>,
) {
    let dirs = |ds: &[Positioned<async_graphql_parser::types::Directive>], out: &mut std::collections::HashSet<String>| {
        for d in ds {
            for (_, a) in &d.node.arguments {
                value_variables(&a.node, out);
            }
        }
    };
    for item in &sel.items {
        match &item.node {
            Selection::Field(f) => {
                if skip.contains(&(f.pos.line, f.pos.column)) {
                    continue;
                }
                for (_, a) in &f.node.arguments {
                    value_variables(&a.node, out);
                }
                dirs(&f.node.directives, out);
                used_variables(doc, &f.node.selection_set.node, out, seen, skip);
            }
            Selection::InlineFragment(f) => {
                dirs(&f.node.directives, out);
                used_variables(doc, &f.node.selection_set.node, out, seen, skip);
            }
            Selection::FragmentSpread(sp) => {
                dirs(&sp.node.directives, out);
                let name = sp.node.fragment_name.node.to_string();
                if seen.insert(name.clone())
                    && let Some(fd) = doc.fragments.get(&sp.node.fragment_name.node)
                {
                    dirs(&fd.node.directives, out);
                    used_variables(doc, &fd.node.selection_set.node, out, seen, skip);
                }
            }
        }
    }
}

/// Whether a selection set spreads `target`, directly or through other fragments.
fn reaches(doc: &ExecutableDocument, sel: &SelectionSet, target: &str, seen: &mut std::collections::HashSet<String>) -> bool {
    for item in &sel.items {
        let found = match &item.node {
            Selection::Field(f) => reaches(doc, &f.node.selection_set.node, target, seen),
            Selection::InlineFragment(f) => reaches(doc, &f.node.selection_set.node, target, seen),
            Selection::FragmentSpread(sp) => {
                let n = sp.node.fragment_name.node.as_str();
                n == target
                    || (seen.insert(n.to_string())
                        && doc.fragments.get(&sp.node.fragment_name.node).is_some_and(|f| reaches(doc, &f.node.selection_set.node, target, seen)))
            }
        };
        if found {
            return true;
        }
    }
    false
}

/// The names of the fragments a selection set spreads, directly or through other fragments.
fn spread_names(doc: &ExecutableDocument, sel: &SelectionSet, out: &mut std::collections::HashSet<String>, seen: &mut std::collections::HashSet<String>) {
    for item in &sel.items {
        match &item.node {
            Selection::Field(f) => spread_names(doc, &f.node.selection_set.node, out, seen),
            Selection::InlineFragment(f) => spread_names(doc, &f.node.selection_set.node, out, seen),
            Selection::FragmentSpread(sp) => {
                let n = sp.node.fragment_name.node.to_string();
                out.insert(n.clone());
                if seen.insert(n)
                    && let Some(f) = doc.fragments.get(&sp.node.fragment_name.node)
                {
                    spread_names(doc, &f.node.selection_set.node, out, seen);
                }
            }
        }
    }
}

/// A key of a literal input object that its type doesn't define: the input type, the key and
/// the path of keys to it.
fn unknown_input_field(schema: &Schema, ty: &TypeRef, v: &async_graphql_value::Value) -> Option<(String, String, Vec<String>)> {
    use async_graphql_value::Value as V;
    match v {
        V::List(items) => items.iter().find_map(|x| unknown_input_field(schema, ty, x)),
        V::Object(fields) => {
            let t = schema.get(ty.named()).filter(|t| t.kind == "INPUT_OBJECT")?;
            for (k, fv) in fields {
                match t.input_fields.iter().find(|f| f.name == k.as_str()) {
                    None => return Some((t.name.clone(), k.to_string(), vec![k.to_string()])),
                    Some(fd) => {
                        if let Some((i, key, mut path)) = unknown_input_field(schema, &fd.ty, fv) {
                            path.insert(0, k.to_string());
                            return Some((i, key, path));
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}
