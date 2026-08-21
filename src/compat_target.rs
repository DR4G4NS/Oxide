//! Compile-time compatibility target. Must match `compat/current.toml`.

/// Advertised Mindustry protocol build (ConnectPacket / LAN).
pub const CURRENT_PROTOCOL_BUILD: i32 = 159;

/// Human-readable Mindustry release targeted by this tree.
pub const CURRENT_BUILD_NAME: &str = "159.7";

/// Official git tag for the current target.
pub const CURRENT_SOURCE_TAG: &str = "v159.7";

/// `git rev-parse v159.7^{commit}`
pub const CURRENT_SOURCE_COMMIT: &str = "c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c";

/// SHA-256 of the official desktop JAR recorded in `compat/current.toml`.
pub const CURRENT_JAR_SHA256: &str =
    "ce1db5b06fe7326b9d0c1d99b1eb1667cf6f0bf97093293f6674ae294981ff05";

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
