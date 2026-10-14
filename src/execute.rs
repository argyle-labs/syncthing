//! Admin check for this plugin's mutating tools.
//!
//! Mutating tools set `execute_gated = false` and own their `dry_run` flag, so
//! the dry run can report the exact drift. Opting out of the central gate also
//! opts out of the role check orca runs inside it, so [`require_admin`]
//! replaces it, on the dry run too.

use plugin_toolkit::contract::CallerIdentity;
use plugin_toolkit::prelude::*;

pub fn require_admin(tool: &str, ctx: &ToolCtx) -> Result<()> {
    check_admin(tool, ctx.caller().as_ref())
}

fn check_admin(subject: &str, caller: Option<&CallerIdentity>) -> Result<()> {
    match caller {
        Some(c) if c.role == "admin" => Ok(()),
        Some(c) => bail!(
            "{subject} requires role 'admin'; caller '{}' has '{}'",
            c.username,
            c.role
        ),
        None => bail!(
            "{subject} refused: the call carries no caller identity, so admin cannot be verified"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(role: &str) -> CallerIdentity {
        CallerIdentity {
            user_id: "u".into(),
            username: "op".into(),
            role: role.into(),
            can_mutate: true,
        }
    }

    #[test]
    fn needs_an_admin_caller() {
        assert!(check_admin("t", Some(&caller("admin"))).is_ok());
        let err = check_admin("t", Some(&caller("user"))).unwrap_err();
        assert!(err.to_string().contains("requires role 'admin'"), "{err}");
        let err = check_admin("t", None).unwrap_err();
        assert!(err.to_string().contains("no caller identity"), "{err}");
    }
}
