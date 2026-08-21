#[derive(Debug, PartialEq)]
pub enum ServerCommand {
    Help,
    Status,
    Host {
        map: String,
        mode: String,
    },
    Stop,
    Exit,
    Maps,
    Save {
        slot: String,
    },
    /// Exports the current world as an official .msav save (v11) that the
    /// desktop client can open (SOL-008).
    SaveMsav {
        slot: String,
    },
    Load {
        slot: String,
    },
    Kick {
        player: String,
    },
    Ban {
        target: String,
        kind: BanKind,
    },
    Pardon {
        target: String,
    },
    Say {
        message: String,
    },
    GameOver,
    Waves {
        wave: Option<u32>,
    },
    Spawn {
        unit: String,
        count: u32,
        x: Option<i16>,
        y: Option<i16>,
    },
    Mode {
        mode: String,
    },
    Time {
        value: f32,
        seconds: bool,
    },
    /// `version` — prints the advertised protocol build and version type.
    Version,
    /// `rules [key [value]]` / `rules remove <key>` — inspect or set global
    /// rules overrides applied on every world load (official `rules`).
    Rules {
        key: Option<String>,
        value: Option<String>,
    },
    /// `nextmap` — immediately rotates to the next map in the rotation.
    NextMap,
    /// `saves` — lists the save slots found next to the active save file.
    Saves,
    /// `loadautosave` — loads the autosave1 slot.
    LoadAutosave,
    /// `reloadmaps` — rescans the maps directory and refreshes the rotation.
    ReloadMaps,
    /// `shuffle [none|all|custom|builtin]` — show or set the map shuffle mode.
    Shuffle {
        mode: Option<String>,
    },
    /// `players` — list connected players with id/uuid/ip (official parity).
    Players,
    /// `bans` — list all banned IPs and UUIDs.
    Bans,
    /// `pause [on|off]` / `resume` — pause or resume the simulation.
    Pause {
        on: bool,
    },
    /// `playerlimit [off|<n>]` — show or change the live player limit.
    PlayerLimit {
        limit: Option<u32>,
    },
    /// `whitelist [add|remove <uuid>|on|off]` — manage the whitelist.
    Whitelist {
        action: Option<WhitelistAction>,
    },
    /// `admin <add|remove> <uuid|name>` — manage persisted admins.
    Admin {
        add: bool,
        target: String,
    },
    /// `admins` — list all admins.
    Admins,
    /// `config [key [value...]]` — show/change runtime config.
    Config {
        key: Option<String>,
        value: Option<String>,
    },
    /// `team <player> <team>` — assign a connected player to a team.
    Team {
        player: String,
        team: String,
    },
    /// `subnet-ban [add|remove <address>]` — ban IP prefixes.
    SubnetBan {
        action: Option<BanListAction>,
    },
    /// `dos-ban [add|remove <ip>]` — manage DOS bans.
    DosBan {
        action: Option<BanListAction>,
    },
    Unknown(String),
}

/// How `ban <target>` should interpret its argument. Official syntax is
/// `ban <type-id/name/ip> <value>`; the shorthand `ban <name|uuid|ip>` is
/// resolved automatically (connected name first, then uuid, then IP).
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BanKind {
    Auto,
    Ip,
    Id,
    Name,
}

#[derive(Debug, PartialEq)]
pub enum WhitelistAction {
    Add(String),
    Remove(String),
    Enable(bool),
}

#[derive(Debug, PartialEq)]
pub enum BanListAction {
    Add(String),
    Remove(String),
}

fn parse_u32(text: &str) -> Option<u32> {
    text.parse::<u32>().ok()
}

fn parse_i16(text: &str) -> Option<i16> {
    text.parse::<i16>().ok()
}

fn parse_f32(text: &str) -> Option<f32> {
    text.parse::<f32>().ok()
}

fn parse_pause_arg(args: &[&str]) -> Option<bool> {
    match args {
        [] => Some(true),
        [arg] => match arg.to_lowercase().as_str() {
            "on" | "true" | "1" => Some(true),
            "off" | "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

impl ServerCommand {
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return None;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let cmd = parts[0].to_lowercase();
        let args = &parts[1..];

        match cmd.as_str() {
            "help" | "?" => Some(ServerCommand::Help),
            "status" => Some(ServerCommand::Status),
            "host" => {
                let map = args.first().copied().unwrap_or("maze").to_string();
                let mode = args.get(1).copied().unwrap_or("survival").to_string();
                Some(ServerCommand::Host { map, mode })
            }
            "stop" => Some(ServerCommand::Stop),
            "exit" | "quit" => Some(ServerCommand::Exit),
            "maps" => Some(ServerCommand::Maps),
            "save" => {
                let slot = args.first().copied().unwrap_or("1").to_string();
                Some(ServerCommand::Save { slot })
            }
            "save-msav" => {
                let slot = args.first().copied().unwrap_or("1").to_string();
                Some(ServerCommand::SaveMsav { slot })
            }
            "load" => {
                let slot = args.first().copied().unwrap_or("1").to_string();
                Some(ServerCommand::Load { slot })
            }
            "kick" => {
                let player = args.join(" ");
                Some(ServerCommand::Kick { player })
            }
            // Official: `ban <type-id/name/ip> <value>`. Shorthand:
            // `ban <name|uuid|ip>` resolves automatically.
            "ban" => match args {
                [target] => Some(ServerCommand::Ban {
                    target: (*target).to_string(),
                    kind: BanKind::Auto,
                }),
                [kind, target] => {
                    let kind = match kind.to_lowercase().as_str() {
                        "ip" => BanKind::Ip,
                        "id" | "uuid" => BanKind::Id,
                        "name" => BanKind::Name,
                        _ => return Some(ServerCommand::Unknown(trimmed.to_string())),
                    };
                    Some(ServerCommand::Ban {
                        target: (*target).to_string(),
                        kind,
                    })
                }
                _ => Some(ServerCommand::Unknown(trimmed.to_string())),
            },
            "pardon" | "unban" => {
                let target = args.first().copied().unwrap_or("").to_string();
                Some(ServerCommand::Pardon { target })
            }
            "say" => {
                let message = args.join(" ");
                Some(ServerCommand::Say { message })
            }
            "gameover" => Some(ServerCommand::GameOver),
            "players" => Some(ServerCommand::Players),
            "bans" => Some(ServerCommand::Bans),
            "pause" => parse_pause_arg(args)
                .map(|on| ServerCommand::Pause { on })
                .or_else(|| Some(ServerCommand::Unknown(trimmed.to_string()))),
            "resume" => parse_pause_arg(&[])
                .map(|_| ServerCommand::Pause { on: false })
                .or_else(|| Some(ServerCommand::Unknown(trimmed.to_string()))),
            "playerlimit" => match args {
                [] => Some(ServerCommand::PlayerLimit { limit: None }),
                [text] if text.eq_ignore_ascii_case("off") => {
                    Some(ServerCommand::PlayerLimit { limit: Some(0) })
                }
                [text] => parse_u32(text)
                    .map(|limit| ServerCommand::PlayerLimit { limit: Some(limit) })
                    .or_else(|| Some(ServerCommand::Unknown(trimmed.to_string()))),
                _ => Some(ServerCommand::Unknown(trimmed.to_string())),
            },
            "whitelist" => match args {
                [] => Some(ServerCommand::Whitelist { action: None }),
                [action] => match action.to_lowercase().as_str() {
                    "on" => Some(ServerCommand::Whitelist {
                        action: Some(WhitelistAction::Enable(true)),
                    }),
                    "off" => Some(ServerCommand::Whitelist {
                        action: Some(WhitelistAction::Enable(false)),
                    }),
                    _ => Some(ServerCommand::Unknown(trimmed.to_string())),
                },
                [action, uuid] => match action.to_lowercase().as_str() {
                    "add" => Some(ServerCommand::Whitelist {
                        action: Some(WhitelistAction::Add((*uuid).to_string())),
                    }),
                    "remove" => Some(ServerCommand::Whitelist {
                        action: Some(WhitelistAction::Remove((*uuid).to_string())),
                    }),
                    _ => Some(ServerCommand::Unknown(trimmed.to_string())),
                },
                _ => Some(ServerCommand::Unknown(trimmed.to_string())),
            },
            "admin" => match args {
                [action, target] => match action.to_lowercase().as_str() {
                    "add" => Some(ServerCommand::Admin {
                        add: true,
                        target: (*target).to_string(),
                    }),
                    "remove" => Some(ServerCommand::Admin {
                        add: false,
                        target: (*target).to_string(),
                    }),
                    _ => Some(ServerCommand::Unknown(trimmed.to_string())),
                },
                _ => Some(ServerCommand::Unknown(trimmed.to_string())),
            },
            "admins" => Some(ServerCommand::Admins),
            "config" => match args {
                [] => Some(ServerCommand::Config {
                    key: None,
                    value: None,
                }),
                [key] => Some(ServerCommand::Config {
                    key: Some((*key).to_string()),
                    value: None,
                }),
                [key, value @ ..] => Some(ServerCommand::Config {
                    key: Some((*key).to_string()),
                    value: Some(value.join(" ")),
                }),
            },
            // `team <name...> <team>`: the team is the last argument, so names
            // with spaces work (runtime resolves names/uuid case-insensitively).
            "team" => match args.split_last() {
                Some((team, player)) if !player.is_empty() => Some(ServerCommand::Team {
                    player: player.join(" "),
                    team: (*team).to_string(),
                }),
                _ => Some(ServerCommand::Unknown(trimmed.to_string())),
            },
            "subnet-ban" => match args {
                [] => Some(ServerCommand::SubnetBan { action: None }),
                [action, address] => match action.to_lowercase().as_str() {
                    "add" => Some(ServerCommand::SubnetBan {
                        action: Some(BanListAction::Add((*address).to_string())),
                    }),
                    "remove" => Some(ServerCommand::SubnetBan {
                        action: Some(BanListAction::Remove((*address).to_string())),
                    }),
                    _ => Some(ServerCommand::Unknown(trimmed.to_string())),
                },
                _ => Some(ServerCommand::Unknown(trimmed.to_string())),
            },
            "dos-ban" => match args {
                [] => Some(ServerCommand::DosBan { action: None }),
                [action, ip] => match action.to_lowercase().as_str() {
                    "add" => Some(ServerCommand::DosBan {
                        action: Some(BanListAction::Add((*ip).to_string())),
                    }),
                    "remove" => Some(ServerCommand::DosBan {
                        action: Some(BanListAction::Remove((*ip).to_string())),
                    }),
                    _ => Some(ServerCommand::Unknown(trimmed.to_string())),
                },
                _ => Some(ServerCommand::Unknown(trimmed.to_string())),
            },
            // `waves` alone dispatches the next wave (wave_time = 0);
            // `waves <n>` re-anchors the wave counter to n.
            "waves" => match args {
                [] => Some(ServerCommand::Waves { wave: None }),
                [text] => parse_u32(text).map(|wave| ServerCommand::Waves { wave: Some(wave) }),
                _ => None,
            },
            // `spawn <unit> <count> [x y]` spawns enemy (team 2) units.
            "spawn" => match args {
                [unit, count] => parse_u32(count).map(|count| ServerCommand::Spawn {
                    unit: (*unit).to_string(),
                    count,
                    x: None,
                    y: None,
                }),
                [unit, count, x, y] => match (parse_u32(count), parse_i16(x), parse_i16(y)) {
                    (Some(count), Some(x), Some(y)) => Some(ServerCommand::Spawn {
                        unit: (*unit).to_string(),
                        count,
                        x: Some(x),
                        y: Some(y),
                    }),
                    _ => None,
                },
                _ => None,
            },
            // `mode <survival|sandbox|pvp|attack>` switches GameMode in vivo.
            "mode" => args.first().map(|mode| ServerCommand::Mode {
                mode: (*mode).to_string(),
            }),
            // `version` prints the advertised build/version type.
            "version" => Some(ServerCommand::Version),
            // `rules` / `rules <key> [value]` / `rules remove <key>`.
            "rules" => match args {
                [] => Some(ServerCommand::Rules {
                    key: None,
                    value: None,
                }),
                [key] => Some(ServerCommand::Rules {
                    key: Some((*key).to_string()),
                    value: None,
                }),
                [key, value @ ..] => Some(ServerCommand::Rules {
                    key: Some((*key).to_string()),
                    value: Some(value.join(" ")),
                }),
            },
            "nextmap" => Some(ServerCommand::NextMap),
            "saves" => Some(ServerCommand::Saves),
            "loadautosave" => Some(ServerCommand::LoadAutosave),
            "reloadmaps" => Some(ServerCommand::ReloadMaps),
            "shuffle" => match args {
                [] => Some(ServerCommand::Shuffle { mode: None }),
                [mode] => Some(ServerCommand::Shuffle {
                    mode: Some((*mode).to_string()),
                }),
                _ => Some(ServerCommand::Unknown(trimmed.to_string())),
            },
            // `time <n>` sets wave_time in ticks; `time <n> s|sec|seconds` in seconds.
            "time" => match args {
                [value] => parse_f32(value).map(|value| ServerCommand::Time {
                    value,
                    seconds: false,
                }),
                [value, unit] if matches!(*unit, "s" | "sec" | "seconds") => {
                    parse_f32(value).map(|value| ServerCommand::Time {
                        value,
                        seconds: true,
                    })
                }
                _ => None,
            },
            _ => Some(ServerCommand::Unknown(trimmed.to_string())),
        }
        .or_else(|| Some(ServerCommand::Unknown(trimmed.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_p2_operational_commands() {
        assert!(matches!(
            ServerCommand::parse("version"),
            Some(ServerCommand::Version)
        ));
        match ServerCommand::parse("rules") {
            Some(ServerCommand::Rules {
                key: None,
                value: None,
            }) => {}
            other => panic!("rules bare: {other:?}"),
        }
        match ServerCommand::parse("rules infiniteResources true") {
            Some(ServerCommand::Rules {
                key: Some(key),
                value: Some(value),
            }) => {
                assert_eq!(key, "infiniteResources");
                assert_eq!(value, "true");
            }
            other => panic!("rules set: {other:?}"),
        }
        match ServerCommand::parse("rules remove buildSpeedMultiplier") {
            Some(ServerCommand::Rules {
                key: Some(key),
                value: Some(value),
            }) => {
                assert_eq!(key, "remove");
                assert_eq!(value, "buildSpeedMultiplier");
            }
            other => panic!("rules remove: {other:?}"),
        }
        assert!(matches!(
            ServerCommand::parse("nextmap"),
            Some(ServerCommand::NextMap)
        ));
        assert!(matches!(
            ServerCommand::parse("saves"),
            Some(ServerCommand::Saves)
        ));
        assert!(matches!(
            ServerCommand::parse("loadautosave"),
            Some(ServerCommand::LoadAutosave)
        ));
        assert!(matches!(
            ServerCommand::parse("reloadmaps"),
            Some(ServerCommand::ReloadMaps)
        ));
        match ServerCommand::parse("shuffle all") {
            Some(ServerCommand::Shuffle { mode: Some(mode) }) => assert_eq!(mode, "all"),
            other => panic!("shuffle: {other:?}"),
        }
        assert!(matches!(
            ServerCommand::parse("shuffle"),
            Some(ServerCommand::Shuffle { mode: None })
        ));
    }
}
