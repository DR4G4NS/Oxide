//! Compile-time compatibility target. Must match `compat/current.toml`.

/// Advertised Mindustry protocol build (ConnectPacket / LAN).
pub const CURRENT_PROTOCOL_BUILD: i32 = 160;

/// Human-readable Mindustry release targeted by this tree.
pub const CURRENT_BUILD_NAME: &str = "160.5";

/// Official git tag for the current target.
pub const CURRENT_SOURCE_TAG: &str = "v160.5";

/// `git rev-parse v160.5^{commit}`
pub const CURRENT_SOURCE_COMMIT: &str = "067c720a8817c1c9fb586c03898a7d948caaed56";

/// SHA-256 of the official desktop JAR recorded in `compat/current.toml`.
pub const CURRENT_JAR_SHA256: &str =
    "c2fd5a5dcb8d306525bb47ff28121237d4d25f53868562946491f2ef261dc272";

/// Official `SaveIO.getVersion()` writer for the current target (Save13).
pub const CURRENT_SAVE_VERSION: i32 = 13;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_time_target_matches_current_toml() {
        let toml = include_str!("../compat/current.toml");
        assert!(
            toml.contains(&format!("build = \"{CURRENT_BUILD_NAME}\"")),
            "compat/current.toml build must be {CURRENT_BUILD_NAME}"
        );
        assert!(toml.contains(&format!("source_tag = \"{CURRENT_SOURCE_TAG}\"")));
        assert!(toml.contains(&format!("source_commit = \"{CURRENT_SOURCE_COMMIT}\"")));
        assert!(toml.contains(&format!("jar_sha256 = \"{CURRENT_JAR_SHA256}\"")));
    }
}
