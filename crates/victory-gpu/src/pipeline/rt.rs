//! Shared RT policy: pipeline creation and dispatch must agree on the builder.
use std::ffi::OsString;

pub(super) struct Policy {
    pub builder: bool,
    pub resolve: bool,
    pub activate: bool,
    pub quarantine: bool,
    pub sync: bool,
}

impl Policy {
    fn from_env(get: impl Fn(&str) -> Option<OsString>) -> Self {
        let builder = get("VICTORY_BVH_BUILDER").is_none_or(|v| v != "0");
        // Without structures there are no hits to shade. Allow explicit overrides for diagnosis.
        let with_bvh = |name| get(name).map_or(builder, |v| v != "0");
        Self {
            builder,
            resolve: with_bvh("VICTORY_RT_RESOLVE"),
            activate: with_bvh("VICTORY_ACTIVATE_RAYS"),
            quarantine: get("VICTORY_BVH_QUARANTINE").is_some_and(|v| v == "1"),
            sync: get("VICTORY_BVH_BUILDER_SYNC").is_some_and(|v| v == "1"),
        }
    }
}

#[cfg(windows)]
pub(super) fn policy() -> &'static Policy {
    static POLICY: std::sync::OnceLock<Policy> = std::sync::OnceLock::new();
    POLICY.get_or_init(|| Policy::from_env(|key| std::env::var_os(key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_builder_opt_out() {
        for (value, on) in [(None, true), (Some("1"), true), (Some("0"), false), (Some(""), true)] {
            let p = Policy::from_env(|k| value.filter(|_| k == "VICTORY_BVH_BUILDER").map(Into::into));
            assert_eq!((p.builder, p.resolve, p.activate), (on, on, on));
            assert!(!p.quarantine && !p.sync);
        }
    }

    #[test]
    fn per_pass_overrides_and_diagnostics() {
        for (builder, value, on) in [("0", "1", true), ("1", "0", false)] {
            let p = Policy::from_env(|k| Some(if k == "VICTORY_BVH_BUILDER" { builder } else { value }.into()));
            assert_eq!((p.resolve, p.activate, p.quarantine, p.sync), (on, on, on, on));
            assert_ne!(p.builder, on);
        }
    }
}
