//! Build identity: the workspace version and the git commit the binaries were
//! built from (embedded by `srv/build.rs`). Surfaced via the service startup
//! log and `GET /version`, so a deployed service can always be attributed to a
//! commit.

pub const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD_GIT_HASH: &str = match option_env!("NVOC_BUILD_GIT_HASH") {
    Some(hash) => hash,
    None => "unknown",
};

/// `{"version":"...","git_hash":"..."}` for the `/version` endpoint. Both
/// values are build-time constants with restricted character sets (SemVer /
/// hex or `"unknown"`), so no JSON escaping is needed.
pub fn version_json() -> String {
    serde_json::json!({ "version": BUILD_VERSION, "git_hash": BUILD_GIT_HASH }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_json_carries_both_build_fields() {
        let parsed: serde_json::Value =
            serde_json::from_str(&version_json()).expect("version_json must be valid JSON");
        assert_eq!(parsed["version"], BUILD_VERSION);
        assert_eq!(parsed["git_hash"], BUILD_GIT_HASH);
        assert_ne!(parsed["git_hash"], "");
    }
}
