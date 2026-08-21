// The binary is a thin launcher: it consumes the library crate instead of
// re-declaring modules, so unit tests compile and run exactly once
// (SOL-AUDIT SOL-013).
mod tui;

use clap::Parser;
use oxide::config::ServerConfig;
use oxide::console::handler::ConsoleHandler;
use oxide::engine::tick::TickEngine;
use oxide::network::listener::NetworkListener;
use oxide::state::administration::Administration;
use oxide::state::game_state::{GameMode, GameState};
use std::io::IsTerminal;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ServerConfig::parse();
    let use_tui = config.should_use_tui(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    );
    let force_tui = config.tui;
    let tui_logs = use_tui.then(tui::LogBuffer::new);

    let filter = || {
        tracing_subscriber::EnvFilter::builder()
            .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
            .from_env_lossy()
    };
    if let Some(logs) = &tui_logs {
        let subscriber = FmtSubscriber::builder()
            .with_env_filter(filter())
            .with_ansi(false)
            .with_writer(logs.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("setting TUI subscriber failed");
    } else {
        let subscriber = FmtSubscriber::builder().with_env_filter(filter()).finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("setting default subscriber failed");
    }

    info!("Starting Mindustry High-Performance Rust Server v0.1.0");
    info!(
        "Configuration: Port={}, MaxPlayers={}, TargetTPS={}",
        config.port, config.max_players, config.tps
    );

    let state = GameState::new();
    state.develop_mode.store(config.develop, Ordering::Relaxed);
    state
        .develop_interval_ms
        .store(config.develop_interval_ms, Ordering::Relaxed);
    if config.develop {
        info!(target: "develop", "develop mode enabled: dump every {} ms", config.develop_interval_ms);
    }
    let admin = Administration::new();
    admin.set_player_limit(config.max_players);

    // Auto-start hosting on configured map and game mode
    let mode = match config.mode.as_str() {
        "pvp" => GameMode::Pvp,
        "attack" => GameMode::Attack,
        "sandbox" => GameMode::Sandbox,
        _ => GameMode::Survival,
    };
    state.start_hosting(config.map_name, mode);

    // Initialize Multithreaded Fixed Tick Engine
    let tick_engine = Arc::new(TickEngine::new(config.tps, state.clone()));
    let _tick_handle = tick_engine.clone().start_loop();

    // Start Tokio TCP Listener on port 6567 (accepts incoming Mindustry clients)
    let net_listener = NetworkListener::new(config.port, state.clone(), admin.clone())
        .with_server_info(
            config.name,
            config.description,
            config.build,
            config.version_type,
        )
        .with_save_path(config.save_file)
        .with_map_path(config.map_file)
        .with_tps(config.tps);
    let network_control = net_listener.control();
    net_listener.start().await?;

    // Launch interactive server console
    let console = ConsoleHandler::new(state, admin, tick_engine, network_control);
    if let Some(logs) = tui_logs {
        if let Err(error) = tui::run(&console, logs.clone(), force_tui).await {
            // Terminal capabilities can differ from TTY detection (notably
            // under minimal PTY wrappers). Keep the server operable.
            logs.enable_stderr();
            warn!("Could not start terminal dashboard: {error}; using classic console");
            console.run_interactive_loop().await;
        }
    } else {
        console.run_interactive_loop().await;
    }

    Ok(())
}
