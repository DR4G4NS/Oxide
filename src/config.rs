use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(
    author,
    version,
    about = "Oxide — high-performance Mindustry headless server"
)]
pub struct ServerConfig {
    /// Force the interactive terminal dashboard, even when terminal
    /// auto-detection is unavailable (for example through a PTY wrapper).
    #[arg(long, conflicts_with = "no_tui")]
    pub tui: bool,

    /// Disable the terminal dashboard and keep the classic line console.
    #[arg(long)]
    pub no_tui: bool,

    #[arg(short, long, default_value_t = 6567)]
    pub port: u16,

    #[arg(long, default_value_t = 100)]
    pub max_players: u32,

    #[arg(long, default_value_t = 60)]
    pub tps: u32,

    #[arg(long, default_value = "Oxide")]
    pub name: String,

    #[arg(long, default_value = "Oxide, a fast Mindustry server written in Rust")]
    pub description: String,

    #[arg(long, default_value = "survival")]
    pub mode: String,

    #[arg(long, default_value = "maze")]
    pub map_name: String,

    /// Official Mindustry .msav map loaded into the initial network world.
    #[arg(long)]
    pub map_file: Option<PathBuf>,

    /// Round 74d: develop mode dumps real-time runtime diagnostics every
    /// `develop_interval_ms` (tick stats, per-connection ping RTT, outbound
    /// drops, pending builds, power graph health, host events and save
    /// latency) to the log with the `develop` target, for debugging the
    /// hard-to-reproduce multiplayer issues.
    #[arg(long)]
    pub develop: bool,

    /// Interval between develop dumps in milliseconds.
    #[arg(long, default_value_t = 5000)]
    pub develop_interval_ms: u64,

    /// Mindustry protocol build advertised during LAN discovery.
    #[arg(long, default_value_t = 159)]
    pub build: i32,

    #[arg(long, default_value = "official")]
    pub version_type: String,

    /// File used to persist player-built tiles across restarts.
    #[arg(long, default_value = "world-delta.json")]
    pub save_file: PathBuf,
}

impl ServerConfig {
    /// Selects the dashboard without making terminal detection part of the
    /// configuration model.  `--no-tui` always wins for programmatic callers;
    /// clap prevents users from passing it together with `--tui`.
    pub fn should_use_tui(&self, stdin_is_terminal: bool, stdout_is_terminal: bool) -> bool {
        !self.no_tui && (self.tui || (stdin_is_terminal && stdout_is_terminal))
    }
}

#[cfg(test)]
mod tests {
    use super::ServerConfig;
    use clap::Parser;

    fn config(args: &[&str]) -> ServerConfig {
        ServerConfig::try_parse_from(args).expect("valid server arguments")
    }

    #[test]
    fn tui_auto_detection_requires_an_interactive_input_and_output() {
        let config = config(&["server"]);
        assert!(config.should_use_tui(true, true));
        assert!(!config.should_use_tui(true, false));
        assert!(!config.should_use_tui(false, true));
    }

    #[test]
    fn tui_flags_override_auto_detection() {
        assert!(config(&["server", "--tui"]).should_use_tui(false, false));
        assert!(!config(&["server", "--no-tui"]).should_use_tui(true, true));
        assert!(ServerConfig::try_parse_from(["server", "--tui", "--no-tui"]).is_err());
    }
}
