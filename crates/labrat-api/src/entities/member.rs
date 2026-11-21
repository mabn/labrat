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
    if ctx.ee {
        // EE adds the email (for admins) and the seat state, as the EE reference answers.
        if ctx.current_user.is_some_and(|c| c.admin) {
            out.insert("email".into(), json!(u.email));
        }
        out.insert("membership_state".into(), json!("active"));
    }
    Value::Object(out)
}
