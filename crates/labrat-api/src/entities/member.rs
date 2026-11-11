//! `API::Entities::Member`.

use crate::entities::user::{self, Ctx};
use labrat_core::model::{Member, User};
use labrat_core::time;
use serde_json::{Value, json};

pub fn member(
    m: &Member,
    u: &User,
    ctx: &Ctx,
    created_by: Option<&User>,
    source_visible: bool,
) -> Value {
    let mut out = user::basic(u, ctx);
    out.insert("access_level".into(), json!(m.access_level));
    out.insert("created_at".into(), json!(time::iso(m.created_at)));
    if source_visible
        && let Some(cb) = created_by {
            out.insert("created_by".into(), user::basic_value(cb, ctx));
        }
    out.insert("expires_at".into(), json!(m.expires_at));
    Value::Object(out)
}
