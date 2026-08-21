use super::view::{
    mathf_range, mathf_range_from_unit, spawn_world_position, SPAWN_POSITION_JITTER,
};
use super::*;

fn var_num(state: &ExecutorState, name: &str) -> f64 {
    state
        .program
        .var_index(name)
        .map(|idx| state.vars[idx].num())
        .unwrap_or(f64::NAN)
}

fn run(source: &str, ticks: usize, budget: usize) -> ExecutorState {
    let program = compile(source).expect("compile");
    let mut state = ExecutorState::new(program, vec![]);
    for _ in 0..ticks {
        state.run_tick(None, budget);
    }
    state
}

/// Empty 32×32 world for logic tests that only need tiles/units.
fn logic_test_world(save_name: &str) -> crate::network::world::DynamicWorld {
    use crate::network::world::DynamicWorld;
    use dashmap::DashMap;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
    use std::sync::Arc;
    let width = 32i32;
    let height = 32i32;
    let cells = (width * height) as usize;
    let state = crate::state::game_state::GameState::new();
    *state.map_name.write() = save_name.to_string();
    DynamicWorld {
        game_state: state,
        width,
        height,
        sharded_unit_cap: 8,
        core_position: 0,
        core_max_health: 0.0,
        cores: DashMap::new(),
        team_core_lists: DashMap::new(),
        base_blocks: vec![0i16; cells],
        base_centers: vec![false; cells],
        tile_data: Vec::new(),
        base_building_templates: Vec::new(),
        base_buildings: DashMap::new(),
        floors: vec![0i16; cells],
        overlays: vec![0i16; cells],
        enemy_spawns: Vec::new(),
        enemies: DashMap::new(),
        players: DashMap::new(),
        player_sessions: DashMap::new(),
        player_profiles: DashMap::new(),
        building_commands: DashMap::new(),
        unit_orders: DashMap::new(),
        next_player_unit_id: AtomicI32::new(2_500_000),
        next_enemy_id: AtomicI32::new(3_000_000),
        unit_group_order: parking_lot::Mutex::new(Vec::new()),
        projectiles: DashMap::new(),
        next_projectile_id: AtomicI32::new(4_000_000),
        overdrive_boosts: DashMap::new(),
        heal_suppression: DashMap::new(),
        force_fields: DashMap::new(),
        tiles: DashMap::new(),
        pending_builds: DashMap::new(),
        pending_breaks: DashMap::new(),
        mineable_ore: std::sync::OnceLock::new(),
        mono_mining_targets: DashMap::new(),
        tile_footprint: DashMap::new(),
        navigation_revision: AtomicU64::new(0),
        ground_navigation: parking_lot::Mutex::new(None),
        leg_navigation: parking_lot::Mutex::new(None),
        save_path: std::env::temp_dir()
            .join(format!("logic-{save_name}-{}.json", std::process::id())),
        network_template: Arc::new(Vec::new()),
        persistence_dirty: AtomicBool::new(false),
        persistence_lock: parking_lot::Mutex::new(()),
        logic_flags: DashMap::new(),
        logic_executors: DashMap::new(),
        logic_display_commands: DashMap::new(),
        base_drill_progress: DashMap::new(),
        base_factory_progress: DashMap::new(),
        base_turret_progress: DashMap::new(),
        base_mender_progress: DashMap::new(),
        team_build_plans: parking_lot::RwLock::new(crate::engine::typeio::TeamBlocks::default()),
        wave_rules: parking_lot::RwLock::new(crate::network::units::WaveRules::default()),
        votekick_target: parking_lot::RwLock::new(None),
        votekick_votes: AtomicI32::new(0),
        votekick_voters: DashMap::new(),
        votekick_cooldowns: DashMap::new(),
        puddles: crate::network::buildings::puddles::PuddleSystem::new(),
    }
}

#[test]
fn set_and_arithmetic_ops() {
    let state = run(
        "set x 3\nop mul y x 4\nop idiv z 7 2\nop equal e 2 2\nop pow p 2 3\nop mod m 7 3",
        1,
        8,
    );
    assert_eq!(var_num(&state, "x"), 3.0);
    assert_eq!(var_num(&state, "y"), 12.0);
    assert_eq!(var_num(&state, "z"), 3.0); // idiv 7/2 = 3
    assert_eq!(var_num(&state, "e"), 1.0); // equal 2 2
    assert_eq!(var_num(&state, "p"), 8.0); // 2^3
    assert_eq!(var_num(&state, "m"), 1.0); // 7 % 3
}

#[test]
fn op_unary_and_degrees() {
    let state = run(
        "op sin s 30\nop cos c 60\nop abs a -5\nop floor f 3.7\nop sqrt r 16",
        1,
        8,
    );
    assert!((var_num(&state, "s") - 0.5).abs() < 1e-6);
    assert!((var_num(&state, "c") - 0.5).abs() < 1e-6);
    assert_eq!(var_num(&state, "a"), 5.0);
    assert_eq!(var_num(&state, "f"), 3.0);
    assert_eq!(var_num(&state, "r"), 4.0);
}

#[test]
fn counter_increments_across_ticks() {
    // The program loops from the start every tick (like the official
    // executor); with budget 2 and 1 instruction, x increments once/tick.
    let state = run("op add x x 1", 5, 1);
    assert_eq!(var_num(&state, "x"), 5.0);
}

#[test]
fn jump_loop_with_named_label_stops() {
    // Loop until x >= 5, then fall through (counter resets at end and the
    // program just re-runs with x stuck at 5).
    let state = run(
        "set x 0\nlabel loop\nop add x x 1\njump loop lessThan x 5",
        20,
        8,
    );
    assert_eq!(var_num(&state, "x"), 5.0);
}

#[test]
fn numeric_jump_and_expressions() {
    let state = run(
        "set x 0\nset x x + 2\njump 2 greaterThan x 10\nset x 999",
        10,
        8,
    );
    // jump 2 skips `set x 999` when x > 10; with x = 2 it does NOT jump,
    // so x becomes 999 on the first tick... then loops.
    assert_eq!(var_num(&state, "x"), 999.0);
}

#[test]
fn wait_yields_and_resumes() {
    // wait 1 = 60 game ticks; each run adds 1/60. After ~60 runs the
    // wait clears and `set x 1` executes.
    let state = run("wait 1\nset x 1", 70, 8);
    assert_eq!(var_num(&state, "x"), 1.0);
}

#[test]
fn end_stops_execution() {
    let state = run("set x 1\nend\nset x 2", 3, 8);
    assert_eq!(var_num(&state, "x"), 1.0);
}

#[test]
fn setrate_limits_budget() {
    // Tick 1 runs at the block default (8): setrate + 3 adds -> c = 3.
    // Tick 2 applies rate 60/s -> 1 instruction: only setrate runs.
    let state = run("setrate 60\nop add c c 1\nop add c c 1\nop add c c 1", 2, 8);
    // tick1: budget 8 -> 2 loops -> c = 6; tick2: rate 60 -> 1 instr.
    assert_eq!(var_num(&state, "c"), 6.0);
}

fn display_fields(command: u64) -> [i32; 7] {
    [
        (command & 0xf) as i32,
        ((command >> 4) & 0x3ff) as i32,
        ((command >> 14) & 0x3ff) as i32,
        ((command >> 24) & 0x3ff) as i32,
        ((command >> 34) & 0x3ff) as i32,
        ((command >> 44) & 0x3ff) as i32,
        ((command >> 54) & 0x3ff) as i32,
    ]
}

#[test]
fn draw_compiles_all_operand_slots_and_packs_signed_fields() {
    let program =
        compile("set x 1\ndraw line -1 2 513 -513 3 4\ndrawflush display1\nset y 2").unwrap();
    assert_eq!(program.instructions.len(), 4);
    assert!(matches!(program.instructions[0], Instr::Set(_, _)));
    assert!(matches!(program.instructions[1], Instr::Draw(_)));
    assert!(matches!(program.instructions[2], Instr::DrawFlush(_)));
    assert!(matches!(program.instructions[3], Instr::Set(_, _)));

    let mut state = ExecutorState::new(program, vec![]);
    state.run_tick(None, 1);
    state.run_tick(None, 1);
    let fields = display_fields(state.graphics_buffer[0]);
    // DisplayCmd fields use sign-magnitude (negative values carry bit 9)
    // and are then masked to ten bits by the generated Java struct.
    assert_eq!(fields, [4, 0x201, 2, 1, 0x201, 3, 4]);
}

#[test]
fn draw_print_expands_text_and_col_unpacks_color() {
    let state = run(
        "packcolor c 1 0.5 0.25 1\nprint \"AB\"\ndraw print 10 20 @topLeft\ndraw col c",
        4,
        1,
    );
    assert!(state.text_buffer.is_empty());
    assert_eq!(state.graphics_buffer.len(), 3); // A, B, then color
    let text = display_fields(state.graphics_buffer[0]);
    assert_eq!(text[0], DrawType::Print as i32);
    assert_eq!(text[3], 'A' as i32);
    let color = display_fields(state.graphics_buffer[2]);
    assert_eq!(color[0], DrawType::Color as i32);
    assert_eq!(color[1..5], [255, 127, 63, 255]);
}

#[test]
fn draw_buffer_and_display_bounds_match_executor_constants() {
    let source = (0..300)
        .map(|_| "draw line 0 0 1 1")
        .collect::<Vec<_>>()
        .join("\n");
    let state = run(&source, 1, 300);
    assert_eq!(state.graphics_buffer.len(), 256);

    // drawflush always clears the local queue, even without a world
    // target, matching DrawFlushI's unconditional clear().
    let state = run("draw line 0 0 1 1\ndrawflush display1", 2, 1);
    assert!(state.graphics_buffer.is_empty());
}

#[test]
fn print_and_strings_append() {
    // budget 1 so each print runs on its own tick without looping.
    let state = run("print \"hello \"\nprint 42", 2, 1);
    assert_eq!(state.text_buffer, "hello 42");
}

#[test]
fn printchar_appends_floor_char() {
    // budget 1: one statement per tick; 67 = 'C', 65.9 floors to 65 = 'A'.
    let state = run("print \"AB\"\nprintchar 67\nprintchar 65.9", 3, 1);
    assert_eq!(state.text_buffer, "ABCA");
    // char cast is modulo 2^16 like Java: 65536 -> 0 (NUL), 55296 is a
    // lone surrogate and is dropped (not representable in UTF-8).
    let state = run("printchar 65536", 1, 1);
    assert_eq!(state.text_buffer, "\0");
    let state = run("printchar 55296", 1, 1);
    assert_eq!(state.text_buffer, "");
}

#[test]
fn printchar_ignores_object_values() {
    // Official PrintCharI appends emoji only for UnlockableContent
    // objects; strings/other objects append nothing.
    let state = run("printchar \"x\"", 1, 1);
    assert_eq!(state.text_buffer, "");
}

#[test]
fn stop_halts_processor_permanently() {
    // stop rewinds the counter onto itself and yields; x never reaches 2.
    let state = run("set x 1\nstop\nset x 2", 5, 8);
    assert_eq!(var_num(&state, "x"), 1.0);
}

#[test]
fn packcolor_packs_rgba_bits() {
    // r=1 -> 255 -> 0xFF000000, a=1 -> 0xFF: bits 0xFF0000FF.
    let state = run("packcolor c 1 0 0 1", 1, 1);
    let idx = state.program.var_index("c").unwrap();
    assert_eq!(state.vars[idx].numval.to_bits(), 0xFF00_00FF);
    // clamping and float truncation: 2 -> 1, -1 -> 0, 0.25 -> 63, 0.5 -> 127.
    let state = run("packcolor c 2 -1 0.25 0.5", 1, 1);
    let idx = state.program.var_index("c").unwrap();
    assert_eq!(state.vars[idx].numval.to_bits(), 0xFF00_3F7F);
}

#[test]
fn unpackcolor_unpacks_rgba_bits() {
    // Round trip: 1 -> 255 -> 1.0; 0.5 -> 127 -> 127/255f (float math);
    // 0.25 -> 63 -> 63/255f.
    let state = run("packcolor c 1 0.5 0.25 1\nunpackcolor r g b a c", 2, 1);
    assert_eq!(var_num(&state, "r"), 1.0);
    assert_eq!(var_num(&state, "g"), (127f32 / 255f32) as f64);
    assert_eq!(var_num(&state, "b"), (63f32 / 255f32) as f64);
    assert_eq!(var_num(&state, "a"), 1.0);
    // An arbitrary double only contributes its low 32 bits: 1.5 -> 0.
    let state = run("unpackcolor r g b a 1.5", 1, 1);
    assert_eq!(var_num(&state, "r"), 0.0);
    assert_eq!(var_num(&state, "g"), 0.0);
    assert_eq!(var_num(&state, "b"), 0.0);
    assert_eq!(var_num(&state, "a"), 0.0);
}

#[test]
fn printchar_stop_packcolor_unpackcolor_compile() {
    let program =
        compile("printchar 65\nstop\npackcolor c 1 0 0 1\nunpackcolor r g b a c").unwrap();
    assert!(matches!(program.instructions[0], Instr::PrintChar(_)));
    assert!(matches!(program.instructions[1], Instr::Stop));
    assert!(matches!(program.instructions[2], Instr::PackColor(..)));
    assert!(matches!(program.instructions[3], Instr::UnpackColor(..)));
    // Malformed statements still compile to NoOp (indices preserved).
    let program = compile("set x 1\npackcolor c 1 0\nstop 1 2 3").unwrap();
    assert!(matches!(program.instructions[0], Instr::Set(_, _)));
    assert!(matches!(program.instructions[1], Instr::NoOp));
    assert!(matches!(program.instructions[2], Instr::Stop));
}

#[test]
fn unsupported_statements_compile_to_noop() {
    // Statements without a supported form compile to NoOp without
    // killing the program; ubind executes for real (P0-02) and must
    // not disturb surrounding statements.
    let state = run("set x 1\nubind @dagger\nset y 2", 1, 8);
    assert_eq!(var_num(&state, "x"), 1.0);
    assert_eq!(var_num(&state, "y"), 2.0);
}

#[test]
fn config_container_round_trip() {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;
    let source = "set x 1";
    let mut container = Vec::new();
    container.push(1);
    container.extend_from_slice(&(source.len() as i32).to_be_bytes());
    container.extend_from_slice(source.as_bytes());
    container.extend_from_slice(&1i32.to_be_bytes());
    container.extend_from_slice(&4u16.to_be_bytes());
    container.extend_from_slice("cell".as_bytes());
    container.extend_from_slice(&(-3i16).to_be_bytes());
    container.extend_from_slice(&4i16.to_be_bytes());
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&container).unwrap();
    let config = encoder.finish().unwrap();

    assert_eq!(source_from_config(&config).as_deref(), Some(source));
    assert_eq!(parse_links(&config), vec![(-3, 4)]);
}

#[test]
fn getlink_assigns_link_object() {
    let program = compile("getlink l 0").unwrap();
    let mut state = ExecutorState::new(program, vec![12345]);
    state.run_tick(None, 8);
    let idx = state.program.var_index("l").unwrap();
    assert_eq!(state.vars[idx].objval, LObject::Building(12345));
}

#[test]
fn ubind_compiles_unit_type() {
    let program = compile("ubind @dagger").unwrap();
    assert!(matches!(program.instructions[0], Instr::Ubind(0)));
    let program = compile("ubind @mega").unwrap();
    assert!(matches!(program.instructions[0], Instr::Ubind(22)));
    let program = compile("ubind 5").unwrap();
    assert!(matches!(program.instructions[0], Instr::Ubind(5)));
}

#[test]
fn ucontrol_subcommands_compile() {
    let program = compile("ucontrol move 100 200").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Move(_, _))
    ));
    let program = compile("ucontrol stop").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Stop)
    ));
    let program = compile("ucontrol within 1 2 3 r").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Within(_, _, _, _))
    ));
    let program = compile("ucontrol getBlock 1 2 b f").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::GetBlock(_, _, _, _))
    ));
    // Unsupported subcommands compile to NoOp without killing the program.
    let program = compile("set x 1\nucontrol approach 1 2 3").unwrap();
    assert!(matches!(program.instructions[0], Instr::Set(_, _)));
    assert!(matches!(program.instructions[1], Instr::NoOp));
}

#[test]
fn radar_compiles_spec() {
    let program = compile("radar turret1 enemy any any 1 distance result").unwrap();
    match &program.instructions[0] {
        Instr::Radar(spec) => {
            assert_eq!(spec.targets[0], RadarTarget::Enemy);
            assert_eq!(spec.sort, RadarSort::Distance);
        }
        _ => panic!("expected Radar"),
    }
    let program = compile("radar turret1 ally flying ground -1 health result").unwrap();
    match &program.instructions[0] {
        Instr::Radar(spec) => {
            assert_eq!(spec.targets[1], RadarTarget::Flying);
            assert_eq!(spec.sort, RadarSort::Health);
        }
        _ => panic!("expected Radar"),
    }
    // Bad syntax -> NoOp without killing the program.
    let program = compile("set x 1\nradar nope").unwrap();
    assert!(matches!(program.instructions[1], Instr::NoOp));
}

#[test]
fn ulocate_compiles_spec() {
    let program = compile("ulocate building core 1 null bx by").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ulocate(UlocSpec {
            kind: UlocKind::Building,
            group: UlocGroup::Core,
            ..
        })
    ));
    let program = compile("ulocate ore copper 1 @copper null ox oy").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ulocate(UlocSpec {
            kind: UlocKind::Ore,
            ..
        })
    ));
    let program = compile("ulocate spawn 0 1 null sx sy").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ulocate(UlocSpec {
            kind: UlocKind::Spawn,
            ..
        })
    ));
}

#[test]
fn fetch_and_format_compile() {
    // JAR 158.1 grammar (LogicIO.read/write + FetchStatement):
    // fetch <type> <result> <team> <index> [extra].
    let program = compile("fetch unit u @sharded 0").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Fetch(crate::logic::ops::FetchKind::Unit, _, _, _, _)
    ));
    let program = compile("fetch build b @crux 2").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Fetch(crate::logic::ops::FetchKind::Build, _, _, _, _)
    ));
    let program = compile("fetch player p @sharded 1").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Fetch(crate::logic::ops::FetchKind::Player, _, _, _, _)
    ));
    let program = compile("fetch unitCount c @sharded").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Fetch(crate::logic::ops::FetchKind::UnitCount, _, _, _, _)
    ));
    let program = compile("fetch core c @sharded").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Fetch(crate::logic::ops::FetchKind::Core, _, _, _, _)
    ));
    // format takes exactly ONE value operand (FormatStatement.value).
    let program = compile("format amount").unwrap();
    assert!(matches!(program.instructions[0], Instr::Format(_)));
    // Malformed fetch/format degrade to NoOp without killing the program.
    let program = compile("set x 1\nfetch nope y 0\nformat").unwrap();
    assert!(matches!(program.instructions[0], Instr::Set(_, _)));
    assert!(matches!(program.instructions[1], Instr::NoOp));
    assert!(matches!(program.instructions[2], Instr::NoOp));
}

#[test]
fn format_substitutes_the_last_placeholder_in_the_runtime_buffer() {
    // JAR 158.1 FormatI.run: the value is substituted into the runtime
    // textBuffer (LAST `{d}` wins), never into a compile-time template.
    // Budget 2 = exactly one pass over the two-instruction program
    // (the executor loops the program every tick like the official).
    // The LAST `{d}` in the buffer wins: `{1}` here.
    let state = run("print \"hp {0} / {1}\"\nformat 100", 1, 2);
    assert_eq!(state.text_buffer, "hp {0} / 100");
    // The last `{d}` in the buffer wins; the value formats like Java
    // (round path for integer values).
    let state = run("print \"a {0} b {0}\"\nformat 7", 1, 2);
    assert_eq!(state.text_buffer, "a {0} b 7");
    // Non-integer values use Java Double.toString.
    let state = run("print \"v {0}\"\nformat 1.5", 1, 2);
    assert_eq!(state.text_buffer, "v 1.5");
    let state = run("print \"v {0}\"\nformat 100000.5", 1, 2);
    assert_eq!(state.text_buffer, "v 100000.5");
    // No `{d}` in the buffer -> no-op (official early return).
    let state = run("print \"no markers\"\nformat 3", 1, 2);
    assert_eq!(state.text_buffer, "no markers");
    // format does not write any variable (official FormatStatement has
    // no result operand).
    let state = run("format 5", 1, 2);
    assert!(state.program.var_index("f").is_none());
}

#[test]
fn fetch_resolves_units_buildings_and_count_in_world() {
    use crate::network::world::{EnemyUnit, PlayerCombatState, UnitAuthority};
    use std::sync::Arc;
    let world = logic_test_world("fetch-test");
    // A team-1 enemy unit and a team-1 player, plus one enemy of team 2.
    world.enemies.insert(
        3_000_001,
        EnemyUnit {
            id: 3_000_001,
            unit_type: 0,
            entity_class: 0,
            team: 1,
            x: 0.0,
            y: 0.0,
            rotation: 0.0,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: 0.0,
            payloads: Vec::new(),
            flag: 0.0,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: 1.0,
            attack_damage: 1.0,
            attack_reload_time: 1.0,
            attack_range: 1.0,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    world.enemies.insert(
        3_000_002,
        EnemyUnit {
            id: 3_000_002,
            unit_type: 0,
            entity_class: 0,
            team: 2,
            x: 0.0,
            y: 0.0,
            rotation: 0.0,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: 0.0,
            payloads: Vec::new(),
            flag: 0.0,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: 1.0,
            attack_damage: 1.0,
            attack_reload_time: 1.0,
            attack_range: 1.0,
            authority: UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    world.players.insert(
        2_000_001,
        PlayerCombatState {
            uuid: "p".into(),
            player_id: 1_000_001,
            unit_id: 2_000_001,
            x: 0.0,
            y: 0.0,
            health: 150.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: 0.0,
            statuses: Vec::new(),
            dead: false,
            respawn_timer: 0.0,
            team: 1,
        },
    );
    assert_eq!(world.enemies.len(), 2, "enemies inserted");
    assert_eq!(world.players.len(), 1, "player inserted");
    let world = Arc::new(world);
    let program =
        compile("fetch unit u @sharded 0\nfetch unitCount c @sharded\nfetch player p @sharded 0")
            .unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = true; // fetch is privileged-only in 158.1
    let view = WorldView {
        world: &world,
        processor_pos: (1 << 16) | 1,
        out: &crate::network::outbound::NOOP,
    };
    state.run_tick(Some(&view), 8);
    let unit_idx = state.program.var_index("u").unwrap();
    assert_eq!(
        state.vars[unit_idx].objval,
        LObject::Unit(3_000_001),
        "first team unit as object"
    );
    let count_idx = state.program.var_index("c").unwrap();
    assert_eq!(state.vars[count_idx].num(), 2.0, "team 1 unit + player");
    let player_idx = state.program.var_index("p").unwrap();
    assert_eq!(
        state.vars[player_idx].objval,
        LObject::Unit(2_000_001),
        "first player unit as object"
    );
    // An unprivileged executor no-ops fetch (LParser replaces
    // privileged statements with InvalidStatement).
    let program = compile("fetch unit u @sharded 0").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = false;
    state.run_tick(Some(&view), 8);
    let unit_idx = state.program.var_index("u").unwrap();
    assert_eq!(state.vars[unit_idx].objval, LObject::Null);
}

#[test]
fn getblock_and_setblock_compile_and_execute() {
    // DashMap shard locks are not reentrant. GitHub runners advertise 2
    // CPUs so two nearby tile keys often share a shard; a live `tiles.get`
    // guard across `setblock` (insert/get_mut) deadlocks there even when
    // a many-core desktop does not. Every Ref must drop before run_tick.
    use crate::network::world::DynamicTile;
    use std::sync::Arc;
    let world = logic_test_world("setblock-test");
    // A building at tile (10,10).
    let pos = (10 << 16) | 10;
    world.tiles.insert(
        pos,
        DynamicTile {
            position: pos,
            block: 349,
            team: 1,
            ..Default::default()
        },
    );
    let world = Arc::new(world);
    // JAR 158.1 grammar: getblock <layer> <result> <x> <y> — the
    // coordinates are TILE coordinates (GetBlockI uses Mathf.round
    // directly, unlike ucontrol getBlock which takes pixels).
    let program = compile("getblock block b 10 10").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = true;
    let view = WorldView {
        world: &world,
        processor_pos: (1 << 16) | 1,
        out: &crate::network::outbound::NOOP,
    };
    state.run_tick(Some(&view), 8);
    let b_idx = state.program.var_index("b").unwrap();
    assert_eq!(
        state.vars[b_idx].objval,
        LObject::Building(pos),
        "getblock finds the building"
    );
    // getblock is privileged-only: an unprivileged processor no-ops.
    let program = compile("getblock block b 10 10").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = false;
    state.run_tick(Some(&view), 8);
    let b_idx = state.program.var_index("b").unwrap();
    assert_eq!(state.vars[b_idx].objval, LObject::Null);
    // setblock with a privileged processor places the block (official
    // grammar: setblock <layer> <block> <x> <y> <team> <rotation>;
    // @sharded resolves to team 1).
    let program = compile("setblock block @conveyor 12 12 @sharded 0").unwrap();
    let mut state = ExecutorState::new(program.clone(), vec![]);
    state.privileged = true;
    state.run_tick(Some(&view), 8);
    let placed = (12 << 16) | 12;
    {
        let tile = world.tiles.get(&placed).unwrap();
        assert_eq!(tile.block, 257, "setblock placed a conveyor");
        assert_eq!(tile.team, 1);
    }
    // Re-execution with the same block/team preserves the tile
    // (official setNet only fires when block/team change): a configured
    // conveyor keeps its config.
    world.tiles.get_mut(&placed).unwrap().config = vec![1, 2, 3];
    state.run_tick(Some(&view), 8);
    {
        let tile = world.tiles.get(&placed).unwrap();
        assert_eq!(tile.block, 257);
        assert_eq!(tile.config, vec![1, 2, 3], "config survives same setblock");
    }
    // An ordinary (unprivileged) processor cannot place.
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = false;
    state.run_tick(Some(&view), 8);
    // tile is unchanged (still the conveyor from the privileged call).
    {
        let tile = world.tiles.get(&placed).unwrap();
        assert_eq!(tile.block, 257);
    }
    // Malformed getblock/setblock degrade to NoOp.
    let program = compile("set x 1\ngetblock a b\nsetblock a b c").unwrap();
    assert!(matches!(program.instructions[0], Instr::Set(_, _)));
    assert!(matches!(program.instructions[1], Instr::NoOp));
    assert!(matches!(program.instructions[2], Instr::NoOp));
    // The bare-name official form (`setblock block conveyor ...`) also
    // resolves to the conveyor content id.
    let program = compile("setblock block conveyor 12 13 @sharded 0").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = true;
    state.run_tick(Some(&view), 8);
    let placed = (12 << 16) | 13;
    assert_eq!(world.tiles.get(&placed).unwrap().block, 257);
    // Rotation is clamped 0..3 (SetBlockI.run 206-242).
    let program = compile("setblock block @conveyor 13 13 @sharded 9").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = true;
    state.run_tick(Some(&view), 8);
    let placed = (13 << 16) | 13;
    {
        let tile = world.tiles.get(&placed).unwrap();
        assert_eq!(tile.rotation, 3, "rotation clamped to 3");
    }
}

#[test]
fn flags_and_spawn_compile() {
    let program = compile("setflag \"enemies\" 1").unwrap();
    assert!(matches!(program.instructions[0], Instr::SetFlag(_, _)));
    let program = compile("getflag f \"enemies\"").unwrap();
    assert!(matches!(program.instructions[0], Instr::GetFlag(_, _)));
    let program = compile("spawn @dagger 100 200 0 u").unwrap();
    match &program.instructions[0] {
        Instr::Spawn(spec) => match &spec.unit_type {
            Expr::UnitType(v) => assert_eq!(*v, 0),
            other => panic!("expected @dagger type expr, got {other:?}"),
        },
        _ => panic!("expected Spawn"),
    }
    let program = compile("set x 1\nspawn nope 0 0 0 u").unwrap();
    assert!(matches!(program.instructions[0], Instr::Set(_, _)));
    match &program.instructions[1] {
        Instr::Spawn(spec) => match &spec.unit_type {
            Expr::Var(_) => {}
            other => panic!("expected runtime spawn type expr, got {other:?}"),
        },
        other => panic!("expected Spawn, got {other:?}"),
    }
}

#[test]
fn flag_key_strips_quotes_and_at() {
    let asm = Assembler::new();
    match asm.flag_key("\"my flag\"") {
        Expr::Str(s) => assert_eq!(s, "my flag"),
        _ => panic!("expected string"),
    }
    match asm.flag_key("@wave") {
        Expr::Str(s) => assert_eq!(s, "wave"),
        _ => panic!("expected string"),
    }
}

#[test]
fn ucontrol_inventory_subcommands_compile() {
    let program = compile("ucontrol flag 42").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Flag(_))
    ));
    let program = compile("ucontrol boost 1").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Boost(_))
    ));
    let program = compile("ucontrol mine 100 200").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Mine(_, _))
    ));
    let program = compile("ucontrol itemDrop core 1").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::ItemDrop(_, _))
    ));
    let program = compile("ucontrol itemTake core @copper 1").unwrap();
    match &program.instructions[0] {
        Instr::Ucontrol(UcOp::ItemTake(_, item, _)) => assert_eq!(*item, 0),
        _ => panic!("expected ItemTake"),
    }
    let program = compile("ucontrol itemTake core @thorium 1").unwrap();
    match &program.instructions[0] {
        Instr::Ucontrol(UcOp::ItemTake(_, item, _)) => assert_eq!(*item, 7),
        _ => panic!("expected ItemTake"),
    }
    // Unsupported subcommands still compile to NoOp.
    let program = compile("set x 1\nucontrol approach 1 2 3").unwrap();
    assert!(matches!(program.instructions[1], Instr::NoOp));
}

#[test]
fn item_id_registry_matches_official() {
    assert_eq!(item_id_from_name("copper"), 0);
    assert_eq!(item_id_from_name("titanium"), 6);
    assert_eq!(item_id_from_name("thorium"), 7);
    assert_eq!(item_id_from_name("surge-alloy"), 12);
    assert_eq!(item_id_from_name("dormant-cyst"), 21);
    assert_eq!(ore_item_id(73), Some(0)); // oreCopper
    assert_eq!(ore_item_id(80), Some(17)); // oreTungsten
    assert_eq!(ore_item_id(999), None);
}

#[test]
fn ucontrol_shoot_and_unbind_compile() {
    let program = compile("ucontrol shoot 100 200 1").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Shoot(_, _, _))
    ));
    let program = compile("ucontrol target 100 200 0").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Target(_, _, _))
    ));
    let program = compile("ucontrol unbind").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Ucontrol(UcOp::Unbind)
    ));
}

#[test]
fn ucontrol_build_and_lookup_compile() {
    let program = compile("ucontrol build 100 200 @conveyor 0").unwrap();
    match &program.instructions[0] {
        Instr::Ucontrol(UcOp::Build(_, _, block, _)) => assert_eq!(*block, 257),
        _ => panic!("expected Build"),
    }
    let program = compile("ucontrol build 100 200 \"duo\" 1").unwrap();
    match &program.instructions[0] {
        Instr::Ucontrol(UcOp::Build(_, _, block, _)) => assert_eq!(*block, 349),
        _ => panic!("expected Build"),
    }
    let program = compile("lookup block 257 result").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Lookup(LookupKind::Block, _, _)
    ));
    let program = compile("lookup unit 20 result").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::Lookup(LookupKind::Unit, _, _)
    ));
    let program = compile("set x 1\nucontrol build 1 2 @nope 0").unwrap();
    assert!(matches!(program.instructions[0], Instr::Set(_, _)));
    assert!(matches!(program.instructions[1], Instr::NoOp));
}

#[test]
fn block_names_match_official_registry() {
    assert_eq!(
        crate::game::block_names::block_id_from_name("conveyor"),
        Some(257)
    );
    assert_eq!(
        crate::game::block_names::block_id_from_name("duo"),
        Some(349)
    );
    assert_eq!(
        crate::game::block_names::block_id_from_name("core-shard"),
        Some(339)
    );
    assert_eq!(
        crate::game::block_names::block_id_from_name("mechanical-drill"),
        Some(325)
    );
    assert_eq!(
        crate::game::block_names::block_id_from_name("micro-processor"),
        Some(431)
    );
    assert_eq!(
        crate::game::block_names::block_id_from_name("router"),
        Some(266)
    );
    assert_eq!(crate::game::block_names::block_id_from_name("nope"), None);
    assert_eq!(
        crate::game::block_names::block_name_from_id(257),
        Some("conveyor")
    );
    assert_eq!(
        crate::game::block_names::block_name_from_id(349),
        Some("duo")
    );
    assert_eq!(crate::game::block_names::block_name_from_id(999), None);
    assert_eq!(unit_name_from_id(0), Some("dagger"));
    assert_eq!(unit_name_from_id(35), Some("alpha"));
    assert_eq!(item_name_from_id(0), Some("copper"));
    assert_eq!(liquid_name_from_id(3), Some("cryofluid"));
}

#[test]
fn unit_objects_work_in_sensor_without_world() {
    // @unit is null without a bound unit; sensor returns 0.
    let program = compile("ubind @dagger\nsensor h @unit @health").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.run_tick(None, 8);
    let idx = state.program.var_index("h").unwrap();
    assert_eq!(state.vars[idx].num(), 0.0);
}

// ------------------------------------------------------------------
// P0-02 — ubind round-robin (official UnitBindI semantics)
// ------------------------------------------------------------------

/// A minimal world with one micro processor (431) of `team` at (10,10).
fn ubind_world(team: u8) -> (std::sync::Arc<crate::network::world::DynamicWorld>, i32) {
    let world = logic_test_world("ubind-test");
    let pos = (10 << 16) | 10;
    world.tiles.insert(
        pos,
        crate::network::world::DynamicTile {
            position: pos,
            block: 431,
            team,
            ..Default::default()
        },
    );
    (std::sync::Arc::new(world), pos)
}

/// Inserts a full EnemyUnit (alive, DefaultAi) with the given fields.
fn ubind_insert_unit(
    world: &crate::network::world::DynamicWorld,
    id: i32,
    team: u8,
    unit_type: i16,
    flag: f64,
) {
    world.enemies.insert(
        id,
        crate::network::world::EnemyUnit {
            id,
            unit_type,
            entity_class: 0,
            team,
            x: 80.0,
            y: 80.0,
            rotation: 0.0,
            health: 100.0,
            shield: 0.0,
            status_effect: -1,
            status_duration: f32::MAX,
            statuses: Vec::new(),
            velocity_x: 0.0,
            velocity_y: 0.0,
            elevation: 0.0,
            payloads: Vec::new(),
            flag,
            items: Vec::new(),
            mine_progress: 0.0,
            attack_reload: 0.0,
            secondary_attack_reload: 0.0,
            tertiary_attack_reload: 0.0,
            quaternary_attack_reload: 0.0,
            move_speed: 1.0,
            attack_damage: 1.0,
            attack_reload_time: 1.0,
            attack_range: 1.0,
            authority: crate::network::world::UnitAuthority::DefaultAi,
            build_plans: Vec::new(),
            update_building: true,
            status_agg: None,
        },
    );
    world.register_unit_group(id);
}

fn ubind_remove_unit(world: &crate::network::world::DynamicWorld, id: i32) {
    world.unregister_unit_group(id);
    world.enemies.remove(&id);
}

/// The @unit VARIABLE (not just state.bound_unit): every executed ubind
/// must setconst it immediately.
fn unit_var_value(state: &ExecutorState) -> LObject {
    state.vars[state.program.unit_var].objval.clone()
}

/// Runs `ubind u` (then optional extra lines) with `u` holding `unit_id`.
fn run_ubind_object(
    world: &crate::network::world::DynamicWorld,
    pos: i32,
    unit_id: i32,
    privileged: bool,
    extra_source: &str,
    ticks: usize,
) -> ExecutorState {
    let source = if extra_source.is_empty() {
        "ubind u".to_string()
    } else {
        format!("ubind u\n{extra_source}")
    };
    let program = compile(&source).unwrap();
    let u_idx = program.var_index("u").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = privileged;
    state.vars[u_idx].isobj = true;
    state.vars[u_idx].objval = LObject::Unit(unit_id);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world,
        processor_pos: pos,
        out: &connections,
    };
    for _ in 0..ticks {
        state.run_tick(Some(&view), 1);
    }
    state
}

/// CommandAI with an active RTS position target (`hasCommand()` true).
fn ubind_give_active_command(world: &crate::network::world::DynamicWorld, id: i32) {
    world.enemies.get_mut(&id).unwrap().authority = crate::network::world::UnitAuthority::Command;
    world.unit_orders.insert(
        id,
        crate::network::world::UnitOrder {
            unit_id: id,
            command: 0,
            target_kind: 0,
            target_x: Some(100.0),
            target_y: Some(100.0),
            target_id: -1,
            stances: 0,
            payload_cooldown: 0.0,
            logic_control: 0,
            queue: Vec::new(),
        },
    );
}

#[test]
fn ubind_round_robins_five_units_in_creation_order() {
    let (world, pos) = ubind_world(1);
    for i in 1..=5 {
        ubind_insert_unit(&world, 3_000_000 + i, 1, 0, f64::from(i));
    }
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    let expected = [1, 2, 3, 4, 5, 1, 2, 3, 4, 5];
    for (step, expected_id) in expected.iter().enumerate() {
        state.run_tick(Some(&view), 1);
        let expected_unit = 3_000_000 + *expected_id;
        assert_eq!(
            state.bound_unit,
            Some(expected_unit),
            "bind {} must select unit {expected_unit}",
            step + 1
        );
        assert_eq!(unit_var_value(&state), LObject::Unit(expected_unit));
    }
    // ubind alone never acquires Logic authority (that is ucontrol's
    // checkLogicAI): every unit keeps its controller.
    for i in 1..=5 {
        let unit = world.enemies.get(&(3_000_000 + i)).unwrap();
        assert_eq!(
            unit.authority,
            crate::network::world::UnitAuthority::DefaultAi
        );
    }
}

#[test]
fn ubind_without_candidates_binds_null() {
    let (world, pos) = ubind_world(1);
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    for bind in 1..=3 {
        state.run_tick(Some(&view), 1);
        assert_eq!(state.bound_unit, None, "bind {bind} with no units");
        assert_eq!(unit_var_value(&state), LObject::Null);
        assert!(!state.vars[state.program.unit_var].isobj);
    }
}

#[test]
fn ubind_refuses_non_logic_controllable_types() {
    // Official type gate: `type.logicControllable` is false for missiles
    // (anthicus-missile id 46) — @unit is null even with candidates.
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_001, 1, 46, 1.0);
    let program = compile("ubind @anthicus-missile").unwrap();
    assert!(matches!(program.instructions[0], Instr::Ubind(46)));
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, None);
    assert_eq!(unit_var_value(&state), LObject::Null);
}

#[test]
fn team1_processor_does_not_bind_team2_units() {
    let (world, pos) = ubind_world(1);
    for i in 1..=3 {
        ubind_insert_unit(&world, 3_000_000 + i, 2, 0, f64::from(i));
    }
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    assert_eq!(
        state.bound_unit, None,
        "enemy-team units are not candidates"
    );
    assert_eq!(unit_var_value(&state), LObject::Null);
}

#[test]
fn team2_processor_binds_its_own_units_only() {
    // The team comes from the EXECUTOR (processor tile), not a constant:
    // a team-2 processor binds team-2 daggers and skips team-1 daggers.
    let (world, pos) = ubind_world(2);
    for i in 1..=3 {
        ubind_insert_unit(&world, 3_000_000 + i, 2, 0, f64::from(i));
    }
    ubind_insert_unit(&world, 3_000_100, 1, 0, 99.0); // must be skipped
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    for expected in [3_000_001, 3_000_002, 3_000_003, 3_000_001] {
        state.run_tick(Some(&view), 1);
        assert_eq!(state.bound_unit, Some(expected));
    }
}

#[test]
fn ubind_binds_unit_object_operand() {
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_007, 1, 0, 7.0);
    let program = compile("ubind u").unwrap();
    let u_idx = program.var_index("u").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.vars[u_idx].isobj = true;
    state.vars[u_idx].objval = LObject::Unit(3_000_007);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_007));
    assert_eq!(unit_var_value(&state), LObject::Unit(3_000_007));
    // A non-object operand (numbers, strings, null) binds nothing,
    // matching the official instanceof dispatch on `type.obj()`.
    state.vars[u_idx].isobj = false;
    state.vars[u_idx].objval = LObject::Null;
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, None);
    assert_eq!(unit_var_value(&state), LObject::Null);
}

#[test]
fn ubind_object_operand_privileged_binds_any_team() {
    // `(u.team == exec.team || exec.privileged)`: a world processor
    // (privileged) may bind an enemy-team unit object.
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_009, 2, 0, 9.0);
    let program = compile("ubind u").unwrap();
    let u_idx = program.var_index("u").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = true;
    state.vars[u_idx].isobj = true;
    state.vars[u_idx].objval = LObject::Unit(3_000_009);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_009));
}

#[test]
fn ubind_object_same_team_command_ai_active_binds() {
    // P0-B1: CommandAI.hasCommand() blocks ucontrol, not ubind.
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_012, 1, 0, 12.0);
    ubind_give_active_command(&world, 3_000_012);
    assert!(!crate::network::units::unit_is_logic_controllable(
        &world, 3_000_012
    ));
    let state = run_ubind_object(&world, pos, 3_000_012, false, "", 1);
    assert_eq!(state.bound_unit, Some(3_000_012));
    assert_eq!(unit_var_value(&state), LObject::Unit(3_000_012));
    assert_eq!(
        world.enemies.get(&3_000_012).unwrap().authority,
        crate::network::world::UnitAuthority::Command,
        "ubind must not acquire Logic authority"
    );
}

#[test]
fn ubind_object_same_team_player_binds() {
    // P0-B1: Player.isLogicControllable() == false blocks ucontrol, not ubind.
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_011, 1, 0, 11.0);
    world.enemies.get_mut(&3_000_011).unwrap().authority =
        crate::network::world::UnitAuthority::Player {
            player_id: 1_000_001,
        };
    assert!(!crate::network::units::unit_is_logic_controllable(
        &world, 3_000_011
    ));
    let state = run_ubind_object(&world, pos, 3_000_011, false, "", 1);
    assert_eq!(state.bound_unit, Some(3_000_011));
    assert_eq!(unit_var_value(&state), LObject::Unit(3_000_011));
    assert_eq!(
        world.enemies.get(&3_000_011).unwrap().authority,
        crate::network::world::UnitAuthority::Player {
            player_id: 1_000_001
        },
        "ubind must not acquire Logic authority"
    );
}

#[test]
fn ubind_object_enemy_nonprivileged_binds_null() {
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_014, 2, 0, 14.0);
    let state = run_ubind_object(&world, pos, 3_000_014, false, "", 1);
    assert_eq!(state.bound_unit, None);
    assert_eq!(unit_var_value(&state), LObject::Null);
}

#[test]
fn ubind_object_not_logic_controllable_type_binds_null() {
    // assembly-drone (id 63): type.logicControllable == false.
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_016, 1, 63, 16.0);
    let state = run_ubind_object(&world, pos, 3_000_016, false, "", 1);
    assert_eq!(state.bound_unit, None);
    assert_eq!(unit_var_value(&state), LObject::Null);
}

#[test]
fn ubind_object_does_not_takeover_ucontrol_still_does() {
    // DoD: ubind never acquires Logic; a later ucontrol is what takes over
    // (DefaultAI) or still refuses (Player / active CommandAI).
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_020, 1, 0, 20.0);
    ubind_insert_unit(&world, 3_000_021, 1, 0, 21.0);
    world.enemies.get_mut(&3_000_021).unwrap().authority =
        crate::network::world::UnitAuthority::Player { player_id: 7 };
    ubind_insert_unit(&world, 3_000_022, 1, 0, 22.0);
    ubind_give_active_command(&world, 3_000_022);

    let default_ai = run_ubind_object(&world, pos, 3_000_020, false, "ucontrol flag 99", 2);
    assert_eq!(default_ai.bound_unit, Some(3_000_020));
    assert!(matches!(
        world.enemies.get(&3_000_020).unwrap().authority,
        crate::network::world::UnitAuthority::Logic { .. }
    ));
    assert_eq!(world.enemies.get(&3_000_020).unwrap().flag, 99.0);

    let player = run_ubind_object(&world, pos, 3_000_021, false, "ucontrol flag 99", 2);
    assert_eq!(player.bound_unit, Some(3_000_021));
    assert_eq!(
        world.enemies.get(&3_000_021).unwrap().authority,
        crate::network::world::UnitAuthority::Player { player_id: 7 }
    );
    assert_eq!(world.enemies.get(&3_000_021).unwrap().flag, 21.0);

    let commanded = run_ubind_object(&world, pos, 3_000_022, false, "ucontrol flag 99", 2);
    assert_eq!(commanded.bound_unit, Some(3_000_022));
    assert_eq!(
        world.enemies.get(&3_000_022).unwrap().authority,
        crate::network::world::UnitAuthority::Command
    );
    assert_eq!(world.enemies.get(&3_000_022).unwrap().flag, 22.0);
}

#[test]
fn ubind_rebinds_when_bound_unit_dies() {
    // Adversarial: the bound unit dies mid-cycle; the dead unit leaves
    // the candidate list (Java's unit cache only holds live units) and
    // the cursor stays consistent: cursor 1 over [2,3,4,5] -> unit 3.
    let (world, pos) = ubind_world(1);
    for i in 1..=5 {
        ubind_insert_unit(&world, 3_000_000 + i, 1, 0, f64::from(i));
    }
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_001));
    world.enemies.get_mut(&3_000_001).unwrap().health = 0.0;
    state.run_tick(Some(&view), 1);
    assert_eq!(
        state.bound_unit,
        Some(3_000_003),
        "dead unit skipped: cursor 1 over [2,3,4,5]"
    );
    assert_eq!(unit_var_value(&state), LObject::Unit(3_000_003));
}

#[test]
fn ubind_sequence_when_earlier_unit_is_removed() {
    // Adversarial: a unit EARLIER in Groups.unit is removed (swap-remove
    // of unordered Seq): after binding A and B (cursor 2), removing A
    // from [1,2,3,4,5] yields [5,2,3,4]; 2 % 4 = 2 -> unit 3.
    let (world, pos) = ubind_world(1);
    for i in 1..=5 {
        ubind_insert_unit(&world, 3_000_000 + i, 1, 0, f64::from(i));
    }
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_001));
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_002));
    ubind_remove_unit(&world, 3_000_001);
    state.run_tick(Some(&view), 1);
    assert_eq!(
        state.bound_unit,
        Some(3_000_003),
        "cursor 2 over swap-removed [5,2,3,4]"
    );
}

#[test]
fn ubind_readd_middle_unit_appends_not_id_order() {
    // P0-B2: B.remove()+B.add() must not restore id order. Groups.unit
    // swap-removes B with C then appends B: [A,C,B].
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_001, 1, 0, 11.0);
    ubind_insert_unit(&world, 3_000_002, 1, 0, 12.0);
    ubind_insert_unit(&world, 3_000_003, 1, 0, 13.0);
    let b = world.enemies.get(&3_000_002).map(|u| u.clone()).unwrap();
    ubind_remove_unit(&world, 3_000_002);
    world.register_unit_group(b.id);
    world.enemies.insert(b.id, b);
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    let mut flags = Vec::new();
    for _ in 0..3 {
        state.run_tick(Some(&view), 1);
        flags.push(world.enemies.get(&state.bound_unit.unwrap()).unwrap().flag as i32);
    }
    assert_eq!(flags, vec![11, 13, 12]);
}

#[test]
fn ubind_four_remove_b_swap_removes_with_last() {
    // [A,B,C,D] remove B -> [A,D,C], not id order [A,C,D].
    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_001, 1, 0, 11.0);
    ubind_insert_unit(&world, 3_000_002, 1, 0, 12.0);
    ubind_insert_unit(&world, 3_000_003, 1, 0, 13.0);
    ubind_insert_unit(&world, 3_000_004, 1, 0, 14.0);
    ubind_remove_unit(&world, 3_000_002);
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    let mut flags = Vec::new();
    for _ in 0..3 {
        state.run_tick(Some(&view), 1);
        flags.push(world.enemies.get(&state.bound_unit.unwrap()).unwrap().flag as i32);
    }
    assert_eq!(flags, vec![11, 14, 13]);
}

#[test]
fn ubind_new_unit_added_mid_cycle() {
    // Adversarial: a new unit joins after a full cycle: cursor 5 over
    // [1,2,3,4,5,6] -> unit 6 (Java: the cache append extends the cycle).
    let (world, pos) = ubind_world(1);
    for i in 1..=5 {
        ubind_insert_unit(&world, 3_000_000 + i, 1, 0, f64::from(i));
    }
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    for _ in 0..5 {
        state.run_tick(Some(&view), 1);
    }
    assert_eq!(state.bound_unit, Some(3_000_005));
    ubind_insert_unit(&world, 3_000_006, 1, 0, 6.0);
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_006));
}

#[test]
fn ubind_cursor_stays_valid_across_len_changes() {
    // Adversarial: shrink and grow the candidate list in one session;
    // every bind must land on a live candidate (no panic, no dead id).
    let (world, pos) = ubind_world(1);
    for i in 1..=3 {
        ubind_insert_unit(&world, 3_000_000 + i, 1, 0, f64::from(i));
    }
    let program = compile("ubind @dagger").unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1); // 1 (cursor -> 1)
    state.run_tick(Some(&view), 1); // 2 (cursor -> 2)
    ubind_remove_unit(&world, 3_000_001);
    ubind_remove_unit(&world, 3_000_003);
    // Only unit 2 remains: 2 % 1 = 0 -> unit 2, cursor -> 3.
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_002));
    ubind_insert_unit(&world, 3_000_004, 1, 0, 4.0);
    ubind_insert_unit(&world, 3_000_005, 1, 0, 5.0);
    // Candidates [2,4,5]: 3 % 3 = 0 -> unit 2, cursor -> 4; then 4 % 3
    // = 1 -> unit 4.
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_002));
    state.run_tick(Some(&view), 1);
    assert_eq!(state.bound_unit, Some(3_000_004));
}

#[test]
fn ubind_sequence_is_deterministic_across_identical_runs() {
    fn run_scenario() -> Vec<i32> {
        let (world, pos) = ubind_world(1);
        for i in 1..=5 {
            ubind_insert_unit(&world, 3_000_000 + i, 1, 0, f64::from(i));
        }
        let program = compile("ubind @dagger").unwrap();
        let mut state = ExecutorState::new(program, vec![]);
        let connections = dashmap::DashMap::new();
        let view = WorldView {
            world: &world,
            processor_pos: pos,
            out: &connections,
        };
        let mut ids = Vec::new();
        for _ in 0..10 {
            state.run_tick(Some(&view), 1);
            ids.push(state.bound_unit.unwrap());
        }
        ids
    }
    let first = run_scenario();
    let second = run_scenario();
    assert_eq!(
        first, second,
        "the same creation order must produce the same bind sequence"
    );
    assert_eq!(
        first,
        vec![
            3_000_001, 3_000_002, 3_000_003, 3_000_004, 3_000_005, 3_000_001, 3_000_002, 3_000_003,
            3_000_004, 3_000_005
        ]
    );
}

// ------------------------------------------------------------------
// P0-03 — logic control acquisition, lease and ucontrol lifecycle
// (official checkLogicAI / LogicAI.controlTimer / resetController)
// ------------------------------------------------------------------

use crate::network::world::{UnitAuthority, UnitOrder};
use std::sync::Arc;

/// One logic-controlled-candidate world: micro processor (431) of `team`
/// at (10,10) plus one alive dagger with an (optional) order.
fn ucontrol_world() -> (Arc<crate::network::world::DynamicWorld>, i32) {
    ubind_world(1)
}

fn insert_logic_unit(
    world: &crate::network::world::DynamicWorld,
    id: i32,
    order_kind: Option<(u8, f32, f32)>,
) {
    ubind_insert_unit(world, id, 1, 0, 0.0);
    if let Some((kind, x, y)) = order_kind {
        world.unit_orders.insert(
            id,
            UnitOrder {
                unit_id: id,
                command: 0,
                stances: 0,
                payload_cooldown: 0.0,
                target_kind: kind,
                target_id: -1,
                target_x: Some(x),
                target_y: Some(y),
                logic_control: 0,
                queue: Vec::new(),
            },
        );
    }
}

/// Copper-wall `BuilderComp.plans` entry used as the first-takeover wipe target.
fn dirty_build_plan() -> crate::network::world::UnitBuildPlan {
    crate::network::world::UnitBuildPlan {
        breaking: false,
        position: (2 << 16) | 2,
        block: 216,
        rotation: 0,
        config: Vec::new(),
    }
}

/// Poly (type 21) with active mining leftover, a non-empty plans queue and
/// a logic-build order (kind 9) — the four pieces first LogicAI takeover
/// must drop (desktop 158.1 `mineTile = null` + `clearBuilding()`).
fn insert_dirty_builder(world: &crate::network::world::DynamicWorld, id: i32) {
    ubind_insert_unit(world, id, 1, 21, 0.0);
    world.unit_orders.insert(
        id,
        UnitOrder {
            unit_id: id,
            command: 2,
            stances: 0,
            payload_cooldown: 0.0,
            target_kind: 9,
            target_id: 216,
            target_x: Some(16.0),
            target_y: Some(16.0),
            logic_control: 0,
            queue: Vec::new(),
        },
    );
    let mut unit = world.enemies.get_mut(&id).unwrap();
    unit.mine_progress = 42.0;
    unit.build_plans = vec![dirty_build_plan()];
}

/// Runs `source` one instruction per tick against the processor at `pos`.
fn run_logic(
    world: &Arc<crate::network::world::DynamicWorld>,
    pos: i32,
    source: &str,
    ticks: usize,
) -> ExecutorState {
    let program = compile(source).unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world,
        processor_pos: pos,
        out: &connections,
    };
    for _ in 0..ticks {
        state.run_tick(Some(&view), 1);
    }
    state
}

fn authority_of(world: &crate::network::world::DynamicWorld, id: i32) -> UnitAuthority {
    world
        .enemies
        .get(&id)
        .map(|unit| unit.authority)
        .unwrap_or(UnitAuthority::DefaultAi)
}

fn remaining_of(world: &crate::network::world::DynamicWorld, id: i32) -> Option<f32> {
    match authority_of(world, id) {
        UnitAuthority::Logic {
            remaining_ticks, ..
        } => Some(remaining_ticks),
        _ => None,
    }
}

/// One authoritative lease-tick pass (the world-tick lease clock).
fn lease_tick(world: &crate::network::world::DynamicWorld, delta: f32) -> bool {
    crate::network::simulation::simulate_logic_control_leases(world, delta)
}

const UNIT: i32 = 3_000_001;

#[test]
fn p003_ubind_alone_does_not_change_authority() {
    // (1) ubind never controls: only a valid ucontrol/ulocate acquires.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    let state = run_logic(&world, pos, "ubind @dagger\nstop", 2);
    assert_eq!(state.bound_unit, Some(UNIT));
    assert_eq!(authority_of(&world, UNIT), UnitAuthority::DefaultAi);
}

#[test]
fn p003_first_ucontrol_move_acquires_logic_authority() {
    // (2) first valid ucontrol -> Logic { processor, 600 }.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, Some((0, 0.0, 0.0)));
    let state = run_logic(&world, pos, "ubind @dagger\nucontrol move 100 200\nstop", 3);
    assert_eq!(state.bound_unit, Some(UNIT));
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic {
            processor_pos: pos,
            remaining_ticks: 600.0,
            processor_generation: 0,
        }
    );
    // The gate ran BEFORE the effect: the move order was applied.
    // UnitControlI stores World.unconv(tile) = tiles * 8 world units.
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 0);
    assert_eq!(order.target_x, Some(800.0));
    assert_eq!(order.target_y, Some(1600.0));
}

#[test]
fn p003_first_takeover_clears_mining_and_build_state() {
    // (3) official "clear old state": mineTile = null (kind 6 + mine
    // progress) and clearBuilding() (kind 9) on the FIRST takeover only.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, Some((6, 40.0, 48.0)));
    world.enemies.get_mut(&UNIT).unwrap().mine_progress = 42.0;
    world.enemies.get_mut(&UNIT).unwrap().build_plans =
        vec![crate::network::world::UnitBuildPlan {
            breaking: false,
            position: (2 << 16) | 2,
            block: 216,
            rotation: 0,
            config: Vec::new(),
        }];
    // ucontrol flag writes no order state, so the cleared kinds are
    // directly observable.
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 5\nstop", 3);
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 0);
    assert_eq!(order.target_x, None);
    assert_eq!(order.target_y, None);
    assert_eq!(world.enemies.get(&UNIT).unwrap().mine_progress, 0.0);
    assert!(world.enemies.get(&UNIT).unwrap().build_plans.is_empty());

    // Kind 9 (logic build) is cleared the same way; the already-Logic
    // unit's re-control (not a first takeover) does NOT clear again.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, Some((9, 40.0, 48.0)));
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 5\nstop", 3);
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 0);
    assert_eq!(order.target_x, None);
}

#[test]
fn p0c2_first_ucontrol_move_clears_mining_and_builder_plans() {
    // P0-C2: first controller → Logic transition via `ucontrol move`
    // wipes mining + BuilderComp.plans + the kind-9 logic build order.
    // A later move on the same LogicAI only refreshes the lease and
    // writes the new destination — Java's move case does not call
    // mineTile=null / clearBuilding() (only `ucontrol stop` does).
    let (world, pos) = ucontrol_world();
    insert_dirty_builder(&world, UNIT);
    run_logic(&world, pos, "ubind @poly\nucontrol move 100 200\nstop", 3);
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic {
            processor_pos: pos,
            remaining_ticks: 600.0,
            processor_generation: 0,
        }
    );
    {
        let unit = world.enemies.get(&UNIT).unwrap();
        assert_eq!(unit.mine_progress, 0.0);
        assert!(unit.build_plans.is_empty());
        assert_eq!(unit.flag, 0.0);
    }
    {
        let order = world.unit_orders.get(&UNIT).unwrap();
        assert_eq!(order.target_kind, 0);
        assert_eq!(order.target_x, Some(800.0));
        assert_eq!(order.target_y, Some(1600.0));
        assert_eq!(order.target_id, -1);
    }

    // Re-seed mining leftover + plans after the unit is already Logic.
    world.enemies.get_mut(&UNIT).unwrap().mine_progress = 42.0;
    world.enemies.get_mut(&UNIT).unwrap().build_plans = vec![dirty_build_plan()];
    if let Some(mut order) = world.unit_orders.get_mut(&UNIT) {
        order.target_kind = 9;
        order.target_id = 216;
        order.target_x = Some(16.0);
        order.target_y = Some(16.0);
        order.command = 2;
    }
    run_logic(&world, pos, "ubind @poly\nucontrol move 300 400\nstop", 3);
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic {
            processor_pos: pos,
            remaining_ticks: 600.0,
            processor_generation: 0,
        }
    );
    let unit = world.enemies.get(&UNIT).unwrap();
    assert_eq!(unit.mine_progress, 42.0);
    assert_eq!(unit.build_plans, vec![dirty_build_plan()]);
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 0);
    assert_eq!(order.target_x, Some(2400.0));
    assert_eq!(order.target_y, Some(3200.0));
}

#[test]
fn p0c2_failed_check_logic_ai_leaves_mining_and_plans() {
    // Failed gate (player possession, or CommandAI.hasCommand) is a
    // complete no-op: mine progress, Builder plans and the kind-9 order
    // stay put, and the controller does not become LogicAI.
    let (world, pos) = ucontrol_world();
    insert_dirty_builder(&world, UNIT);
    world.enemies.get_mut(&UNIT).unwrap().authority = UnitAuthority::Player { player_id: 7 };
    run_logic(&world, pos, "ubind @poly\nucontrol move 100 200\nstop", 3);
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Player { player_id: 7 }
    );
    assert_eq!(world.enemies.get(&UNIT).unwrap().mine_progress, 42.0);
    assert_eq!(
        world.enemies.get(&UNIT).unwrap().build_plans,
        vec![dirty_build_plan()]
    );
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 9);
    assert_eq!(order.target_x, Some(16.0));
    assert_eq!(order.target_id, 216);

    let (world, pos) = ucontrol_world();
    insert_dirty_builder(&world, UNIT);
    world.enemies.get_mut(&UNIT).unwrap().authority = UnitAuthority::Command;
    if let Some(mut order) = world.unit_orders.get_mut(&UNIT) {
        order.target_kind = 0;
        order.target_x = Some(500.0);
        order.target_y = Some(500.0);
        order.target_id = -1;
    }
    world.enemies.get_mut(&UNIT).unwrap().mine_progress = 42.0;
    world.enemies.get_mut(&UNIT).unwrap().build_plans = vec![dirty_build_plan()];
    run_logic(&world, pos, "ubind @poly\nucontrol move 100 200\nstop", 3);
    assert_eq!(authority_of(&world, UNIT), UnitAuthority::Command);
    assert_eq!(world.enemies.get(&UNIT).unwrap().mine_progress, 42.0);
    assert_eq!(
        world.enemies.get(&UNIT).unwrap().build_plans,
        vec![dirty_build_plan()]
    );
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 0);
    assert_eq!(order.target_x, Some(500.0));
}

#[test]
fn p003_refresh_at_tick_500_returns_lease_to_600() {
    // (4) a valid ucontrol at lease=500 resets it to 600.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    for _ in 0..100 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(500.0));
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 7\nstop", 3);
    assert_eq!(remaining_of(&world, UNIT), Some(600.0));
}

#[test]
fn p003_lease_expires_after_600_ticks_without_refresh() {
    // (5) Java `controlTimer > 0` decrements while positive: through tick
    // 600 the timer lands on exactly 0.0; tick 601 releases the unit.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, Some((6, 40.0, 48.0)));
    run_logic(&world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    for _ in 0..600 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(0.0));
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    assert!(lease_tick(&world, 1.0));
    let expected =
        crate::network::units::default_unit_authority(&world, &world.enemies.get(&UNIT).unwrap());
    assert_eq!(authority_of(&world, UNIT), expected);
    // Release drops the transient logic order kinds too.
    assert!(!crate::network::units::unit_bound_to_logic(&world, UNIT));
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 0);
}

#[test]
fn p003_processor_destroyed_releases_unit_next_lease_pass() {
    // (6) tile gone -> controller invalid -> immediate release.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    world.tiles.remove(&pos);
    assert!(lease_tick(&world, 1.0));
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

#[test]
fn p003_ulocate_refreshes_the_timeout() {
    // (7) UnitLocateI refreshes the lease exactly like ucontrol.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    for _ in 0..100 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(500.0));
    run_logic(
        &world,
        pos,
        "ubind @dagger\nulocate building core 0 b x y\nstop",
        3,
    );
    assert_eq!(remaining_of(&world, UNIT), Some(600.0));
}

#[test]
fn p003_ucontrol_unbind_resets_authority_and_keeps_unit_bound() {
    // (8) + (9): unbind resets the controller but does NOT clear @unit,
    // and drops the transient logic order state (the LogicAI object
    // disappears with all of it).
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, Some((0, 0.0, 0.0)));
    let state = run_logic(
        &world,
        pos,
        "ubind @dagger\nucontrol mine 10 10\nucontrol unbind\nstop",
        4,
    );
    let expected =
        crate::network::units::default_unit_authority(&world, &world.enemies.get(&UNIT).unwrap());
    assert_eq!(authority_of(&world, UNIT), expected);
    // @unit still points at the same unit (state AND executor variable).
    assert_eq!(state.bound_unit, Some(UNIT));
    assert_eq!(unit_var_value(&state), LObject::Unit(UNIT));
    // The kind-6 mine order the processor issued was transient logic
    // state; resetController drops it.
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 0);
    assert_eq!(order.target_x, None);
}

#[test]
fn p003_second_processor_takes_over_with_its_position() {
    // (10) checkLogicAI re-points la.controller at the CURRENT processor:
    // B's valid ucontrol legally steals the unit with B's position.
    let (world, _pos_a) = ucontrol_world();
    let pos_b = (20 << 16) | 20;
    world.tiles.insert(
        pos_b,
        crate::network::world::DynamicTile {
            position: pos_b,
            block: 431,
            team: 1,
            ..Default::default()
        },
    );
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, _pos_a, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic {
            processor_pos: _pos_a,
            remaining_ticks: 600.0,
            processor_generation: 0,
        }
    );
    // B controls the already-bound unit (no ubind needed: @unit persists
    // per executor; a fresh state binds the same single unit anyway).
    let state = run_logic(&world, pos_b, "ubind @dagger\nucontrol flag 2\nstop", 3);
    assert_eq!(state.bound_unit, Some(UNIT));
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic {
            processor_pos: pos_b,
            remaining_ticks: 600.0,
            processor_generation: 0,
        }
    );
    assert_eq!(world.enemies.get(&UNIT).unwrap().flag, 2.0);
}

#[test]
fn p003_bound_unit_death_leaves_no_stale_logic_authority() {
    // (11) the P0-01 cleanup removes unit + order together; a lease pass
    // over the dead unit neither panics nor resurrects authority.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    world.enemies.remove(&UNIT);
    crate::network::units::detach_unit_control(&world, UNIT);
    assert!(!lease_tick(&world, 1.0));
    assert!(world
        .enemies
        .iter()
        .all(|unit| { !matches!(unit.authority, UnitAuthority::Logic { .. }) }));
}

#[test]
fn p003_team_change_blocks_refresh_and_expires_on_schedule() {
    // (12) Java's `Unit.team(Team)` is a plain field setter (no
    // controller reset): the unit KEEPS its LogicAI until the lease runs
    // out, but the now enemy processor can no longer refresh it
    // (`unit.team == exec.team` gate in checkLogicAI).
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    for _ in 0..100 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(500.0));
    world.enemies.get_mut(&UNIT).unwrap().team = 2;
    // Direct ucontrol on the bound unit (no ubind so the team change
    // cannot hide behind the bind's candidate filter).
    let mut state = run_logic(&world, pos, "ucontrol flag 9\nstop", 0);
    state.bound_unit = Some(UNIT);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    // Gate failed: no refresh, no effect, authority untouched.
    assert_eq!(remaining_of(&world, UNIT), Some(500.0));
    assert_eq!(world.enemies.get(&UNIT).unwrap().flag, 0.0);
    // The lease then runs out on the original schedule (the refresh
    // count is unchanged by the failed attempts): 500 decrements to 0.0,
    // the next pass releases.
    for _ in 0..500 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(0.0));
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    lease_tick(&world, 1.0);
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

#[test]
fn p003_processor_position_reused_by_other_building_releases() {
    // (13) the same tile holding a non-processor block is NOT a valid
    // controller (Building.isValid: tile.build == this fails).
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    world.tiles.get_mut(&pos).unwrap().block = 257; // conveyor
    assert!(lease_tick(&world, 1.0));
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

/// Stamps a new micro-processor instance at `pos` (same block id 431).
/// Distinct from merely writing `block = 431`: Java's setBlock creates
/// a new Building, so generation must change.
fn place_processor_instance(
    world: &crate::network::world::DynamicWorld,
    pos: i32,
    team: u8,
) -> u64 {
    let mut tile = crate::network::world::DynamicTile {
        position: pos,
        block: 431,
        team,
        ..Default::default()
    };
    crate::network::world::stamp_new_building(world, &mut tile);
    let generation = tile.generation;
    world.tiles.insert(pos, tile);
    generation
}

fn logic_generation(world: &crate::network::world::DynamicWorld, id: i32) -> Option<u64> {
    match authority_of(world, id) {
        UnitAuthority::Logic {
            processor_generation,
            ..
        } => Some(processor_generation),
        _ => None,
    }
}

#[test]
fn p0c1_live_processor_keeps_lease() {
    let (world, pos) = ucontrol_world();
    let gen = place_processor_instance(&world, pos, 1);
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 1\nstop", 3);
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic {
            processor_pos: pos,
            remaining_ticks: 600.0,
            processor_generation: gen,
        }
    );
    assert!(!lease_tick(&world, 1.0));
    assert_eq!(logic_generation(&world, UNIT), Some(gen));
    assert_eq!(remaining_of(&world, UNIT), Some(599.0));
}

#[test]
fn p0c1_destroy_processor_releases_lease() {
    let (world, pos) = ucontrol_world();
    place_processor_instance(&world, pos, 1);
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 1\nstop", 3);
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    world.tiles.remove(&pos);
    assert!(lease_tick(&world, 1.0));
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

#[test]
fn p0c1_replace_processor_with_wall_releases_lease() {
    let (world, pos) = ucontrol_world();
    place_processor_instance(&world, pos, 1);
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 1\nstop", 3);
    let mut wall = crate::network::world::DynamicTile {
        position: pos,
        block: 216, // copper-wall
        team: 1,
        ..Default::default()
    };
    crate::network::world::stamp_new_building(&world, &mut wall);
    world.tiles.insert(pos, wall);
    assert!(lease_tick(&world, 1.0));
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

#[test]
fn p0c1_same_tile_processor_replacement_releases_old_lease() {
    // A@tile then B@tile (same block id) must not keep A's Logic lease:
    // Java holds the Building object, not the position.
    let (world, pos) = ucontrol_world();
    let gen_a = place_processor_instance(&world, pos, 1);
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 1\nstop", 3);
    assert_eq!(logic_generation(&world, UNIT), Some(gen_a));
    let gen_b = place_processor_instance(&world, pos, 1);
    assert_ne!(gen_a, gen_b, "B must be a distinct instance");
    assert_eq!(world.tiles.get(&pos).unwrap().block, 431);
    assert!(lease_tick(&world, 1.0));
    assert!(
        !matches!(authority_of(&world, UNIT), UnitAuthority::Logic { .. }),
        "same-tile processor B must not inherit A's lease"
    );
}

#[test]
fn p0c1_new_processor_ucontrol_acquires_fresh_lease() {
    let (world, pos) = ucontrol_world();
    place_processor_instance(&world, pos, 1);
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 1\nstop", 3);
    let gen_b = place_processor_instance(&world, pos, 1);
    assert!(lease_tick(&world, 1.0));
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 2\nstop", 3);
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic {
            processor_pos: pos,
            remaining_ticks: 600.0,
            processor_generation: gen_b,
        }
    );
    assert_eq!(world.enemies.get(&UNIT).unwrap().flag, 2.0);
}

#[test]
fn p0c1_second_same_tile_replacement_releases_again() {
    let (world, pos) = ucontrol_world();
    place_processor_instance(&world, pos, 1);
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 1\nstop", 3);
    let gen_b = place_processor_instance(&world, pos, 1);
    assert!(lease_tick(&world, 1.0));
    run_logic(&world, pos, "ubind @dagger\nucontrol flag 2\nstop", 3);
    assert_eq!(logic_generation(&world, UNIT), Some(gen_b));
    let gen_c = place_processor_instance(&world, pos, 1);
    assert_ne!(gen_b, gen_c);
    assert!(lease_tick(&world, 1.0));
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

#[test]
fn p003_fractional_delta_at_boundary_expires_on_next_tick() {
    // (14) delta != 1.0: 599.5 remaining still passes the pre-decrement
    // `> 0` check on the 600th tick (going to -0.5, still controlled);
    // the NEXT tick releases. Total elapsed 600.5 ticks.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    run_logic(&world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    lease_tick(&world, 0.5);
    assert_eq!(remaining_of(&world, UNIT), Some(599.5));
    for _ in 0..599 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(0.5));
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    lease_tick(&world, 1.0); // 600.0 elapsed
    assert_eq!(remaining_of(&world, UNIT), Some(-0.5));
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    lease_tick(&world, 1.0); // 600.5 elapsed
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

// ------------------------------------------------------------------
// P0-C3 — exact lease boundary, refresh points and `ucontrol unbind`
// ------------------------------------------------------------------

/// Acquires the lease with one `ucontrol` and returns the processor tile.
/// After this the unit holds a full 600-tick lease and no further
/// instruction runs, so the lease clock is the only thing moving.
fn acquire_full_lease(world: &Arc<crate::network::world::DynamicWorld>, pos: i32) {
    run_logic(world, pos, "ubind @dagger\nucontrol move 1 1\nstop", 3);
    assert_eq!(remaining_of(world, UNIT), Some(600.0));
}

/// Runs ONE instruction against `unit` without a preceding `ubind`, so a
/// gate that only the bound unit can fail (team change) is observable.
fn run_bound_instruction(
    world: &Arc<crate::network::world::DynamicWorld>,
    pos: i32,
    source: &str,
    unit_id: i32,
) -> ExecutorState {
    let program = compile(&format!("{source}\nstop")).unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.bound_unit = Some(unit_id);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 1);
    state
}

#[test]
fn p0c3_lease_boundary_is_exactly_599_600_601() {
    // `controlTimer > 0` is checked BEFORE the decrement (LogicAI.java:
    // 59-64), so a 600.0 lease survives exactly 600 clock ticks: after
    // 599 it holds 1.0, after 600 it holds 0.0 and is still controlled,
    // and the 601st pass fails the check and resets the controller.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    acquire_full_lease(&world, pos);

    for _ in 0..599 {
        assert!(!lease_tick(&world, 1.0));
    }
    assert_eq!(remaining_of(&world, UNIT), Some(1.0), "tick 599");

    assert!(!lease_tick(&world, 1.0));
    assert_eq!(remaining_of(&world, UNIT), Some(0.0), "tick 600");
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));

    assert!(lease_tick(&world, 1.0), "tick 601 releases");
    let expected =
        crate::network::units::default_unit_authority(&world, &world.enemies.get(&UNIT).unwrap());
    assert_eq!(authority_of(&world, UNIT), expected);
}

#[test]
fn p0c3_ucontrol_refresh_on_tick_599_extends_the_lease() {
    // The last tick before expiry still refreshes to a FULL 600, so the
    // boundary restarts from there: 599/600 controlled, 601 released.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    acquire_full_lease(&world, pos);
    for _ in 0..599 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(1.0));

    run_logic(&world, pos, "ubind @dagger\nucontrol flag 7\nstop", 3);
    assert_eq!(remaining_of(&world, UNIT), Some(600.0));

    for _ in 0..600 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(0.0));
    assert!(lease_tick(&world, 1.0));
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

#[test]
fn p0c3_ulocate_refresh_on_tick_599_extends_the_lease() {
    // UnitLocateI sets `ai.controlTimer = logicControlTimeout` on the
    // same line as ucontrol (LExecutor.java:238), so it refreshes the
    // boundary identically — even when the locate itself finds nothing.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    acquire_full_lease(&world, pos);
    for _ in 0..599 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(1.0));

    run_logic(
        &world,
        pos,
        "ubind @dagger\nulocate building core 0 b x y\nstop",
        3,
    );
    assert_eq!(remaining_of(&world, UNIT), Some(600.0));

    for _ in 0..600 {
        lease_tick(&world, 1.0);
    }
    assert!(matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
    assert!(lease_tick(&world, 1.0));
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

#[test]
fn p0c3_failed_gate_on_tick_599_does_not_refresh() {
    // `ai.controlTimer = logicControlTimeout` lives INSIDE the
    // `ai != null` branch: a ucontrol whose checkLogicAI fails leaves the
    // timer alone, so the original boundary still expires on schedule.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    acquire_full_lease(&world, pos);
    for _ in 0..599 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(1.0));

    // Enemy team now: `unit.team == exec.team` fails for this processor.
    world.enemies.get_mut(&UNIT).unwrap().team = 2;
    run_bound_instruction(&world, pos, "ucontrol flag 7", UNIT);
    assert_eq!(remaining_of(&world, UNIT), Some(1.0), "no refresh");
    assert_eq!(world.enemies.get(&UNIT).unwrap().flag, 0.0, "no effect");
    run_bound_instruction(&world, pos, "ulocate building core 0 b x y", UNIT);
    assert_eq!(remaining_of(&world, UNIT), Some(1.0), "no refresh");

    assert!(!lease_tick(&world, 1.0));
    assert_eq!(remaining_of(&world, UNIT), Some(0.0), "tick 600");
    assert!(lease_tick(&world, 1.0), "tick 601 releases on schedule");
    assert!(!matches!(
        authority_of(&world, UNIT),
        UnitAuthority::Logic { .. }
    ));
}

#[test]
fn p0c3_invalid_processor_releases_before_the_lease_expires() {
    // `controller != null && controller.isValid()` is the other half of
    // the same guard: destroying the processor mid-lease releases on the
    // very next pass, hundreds of ticks early.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    acquire_full_lease(&world, pos);
    for _ in 0..300 {
        lease_tick(&world, 1.0);
    }
    assert_eq!(remaining_of(&world, UNIT), Some(300.0));

    world.tiles.remove(&pos);
    assert!(lease_tick(&world, 1.0));
    let expected =
        crate::network::units::default_unit_authority(&world, &world.enemies.get(&UNIT).unwrap());
    assert_eq!(authority_of(&world, UNIT), expected);
}

#[test]
fn p0c3_unbind_releases_the_controller_but_keeps_bound_unit() {
    // `case unbind -> unit.resetController()` (LExecutor.java:372-375)
    // touches the UNIT's controller only. `exec.unit` is never written,
    // so @unit still points at the same unit and a following ucontrol
    // legally takes it over again with a fresh 600-tick lease.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    acquire_full_lease(&world, pos);
    for _ in 0..100 {
        lease_tick(&world, 1.0);
    }

    let state = run_bound_instruction(&world, pos, "ucontrol unbind", UNIT);
    assert_eq!(remaining_of(&world, UNIT), None, "lease dropped");
    assert_eq!(state.bound_unit, Some(UNIT), "@unit survives unbind");
    assert_eq!(unit_var_value(&state), LObject::Unit(UNIT));

    let state = run_bound_instruction(&world, pos, "ucontrol flag 3", UNIT);
    assert_eq!(state.bound_unit, Some(UNIT));
    assert_eq!(remaining_of(&world, UNIT), Some(600.0), "re-acquired");
    assert_eq!(world.enemies.get(&UNIT).unwrap().flag, 3.0);
}

#[test]
fn p0c3_unbind_with_an_active_rts_command_is_a_complete_noop() {
    // checkLogicAI runs BEFORE the switch, and `CommandAI
    // .isLogicControllable()` is `!hasCommand()` (CommandAI.java:
    // 106-109). A unit following an RTS order therefore fails the gate,
    // `ai` is null and the whole instruction — resetController included
    // — is skipped: the RTS command keeps driving the unit.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, Some((0, 500.0, 500.0)));
    world.enemies.get_mut(&UNIT).unwrap().authority = UnitAuthority::Command;

    let state = run_bound_instruction(&world, pos, "ucontrol unbind", UNIT);
    assert_eq!(authority_of(&world, UNIT), UnitAuthority::Command);
    let order = world.unit_orders.get(&UNIT).unwrap();
    assert_eq!(order.target_kind, 0);
    assert_eq!(order.target_x, Some(500.0));
    assert_eq!(order.target_y, Some(500.0));
    drop(order);
    // @unit is untouched on the failed path too.
    assert_eq!(state.bound_unit, Some(UNIT));
    assert_eq!(unit_var_value(&state), LObject::Unit(UNIT));
}

#[test]
fn p003_ucontrol_on_player_controlled_unit_is_noop() {
    // Extra gate coverage: a player-possessed unit is not
    // logic-controllable (PlayerComp.isLogicControllable == false), so
    // every ucontrol — unbind included — is a complete no-op.
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    world.enemies.get_mut(&UNIT).unwrap().authority = UnitAuthority::Player { player_id: 7 };
    run_logic(
        &world,
        pos,
        "ubind @dagger\nucontrol move 10 20\nucontrol unbind\nstop",
        4,
    );
    assert_eq!(
        authority_of(&world, UNIT),
        UnitAuthority::Player { player_id: 7 }
    );
}

#[test]
fn p106_ucontrol_move_takes_effect_on_the_next_unit_tick() {
    // Java Logic.updateEntities: Groups.unit.update() then Groups.build.
    // A ucontrol issued while buildings run is first consumed on N+1.
    use crate::network::units::unit_orders::apply_ordered_unit_movement;
    let (world, pos) = ucontrol_world();
    insert_logic_unit(&world, UNIT, None);
    {
        let mut unit = world.enemies.get_mut(&UNIT).unwrap();
        unit.x = 40.0;
        unit.y = 40.0;
    }
    let start = {
        let unit = world.enemies.get(&UNIT).unwrap();
        (unit.x, unit.y)
    };

    // N-1: no order, movement is a no-op.
    let snapshot = world.enemies.get(&UNIT).unwrap().clone();
    assert!(!apply_ordered_unit_movement(&world, &snapshot, 1.0));
    assert_eq!(
        {
            let unit = world.enemies.get(&UNIT).unwrap();
            (unit.x, unit.y)
        },
        start
    );

    // N: processors assign the destination after units have already
    // updated, so position is unchanged at end N.
    run_logic(&world, pos, "ubind @dagger\nucontrol move 100 80\nstop", 3);
    let order = world.unit_orders.get(&UNIT).expect("order exists at end N");
    assert_eq!(order.command, 0);
    assert_eq!(order.target_x, Some(800.0));
    assert_eq!(order.target_y, Some(640.0));
    drop(order);
    assert_eq!(
        {
            let unit = world.enemies.get(&UNIT).unwrap();
            (unit.x, unit.y)
        },
        start,
        "end N: unit has not moved yet"
    );

    // N+1 / N+2: the next Groups.unit.update consumes the order. On a
    // blocked test map the pathfinder may stand still; the assigned
    // destination is still the observable that Java would apply then.
    let snapshot = world.enemies.get(&UNIT).unwrap().clone();
    apply_ordered_unit_movement(&world, &snapshot, 1.0);
    let snapshot = world.enemies.get(&UNIT).unwrap().clone();
    apply_ordered_unit_movement(&world, &snapshot, 1.0);
    let live = world.unit_orders.get(&UNIT);
    assert!(
        live.as_ref().is_some_and(|order| {
            order.command == 0 && order.target_x == Some(800.0) && order.target_y == Some(640.0)
        }) || {
            let unit = world.enemies.get(&UNIT).unwrap();
            (unit.x, unit.y) != start
        },
        "end N+2: either the unit moved or the move order is still live"
    );
}

fn privileged_tick(
    world: &std::sync::Arc<crate::network::world::DynamicWorld>,
    pos: i32,
    source: &str,
    privileged: bool,
) -> ExecutorState {
    privileged_tick_bound(world, pos, source, privileged, None)
}

fn privileged_tick_bound(
    world: &std::sync::Arc<crate::network::world::DynamicWorld>,
    pos: i32,
    source: &str,
    privileged: bool,
    bound: Option<i32>,
) -> ExecutorState {
    let source = if source.trim_end().ends_with("stop") {
        source.to_string()
    } else {
        format!("{source}\nstop")
    };
    let program = compile(&source).unwrap();
    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = privileged;
    state.bound_unit = bound;
    if let Some(id) = bound {
        if let Some(idx) = state.program.var_index("u") {
            state.vars[idx].isobj = true;
            state.vars[idx].constant = false;
            state.vars[idx].objval = LObject::Unit(id);
        }
    }
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 16);
    state
}

#[test]
fn uradar_compiles_official_mlog_and_scans_from_bound_unit() {
    let program = compile("uradar enemy any any distance 0 1 result").unwrap();
    match &program.instructions[0] {
        Instr::Radar(spec) => {
            assert_eq!(spec.targets[0], RadarTarget::Enemy);
            assert_eq!(spec.sort, RadarSort::Distance);
        }
        other => panic!("expected Radar, got {other:?}"),
    }
    let (program, diagnostics) = compile_report("set x 1\nuradar nope");
    assert!(matches!(program.unwrap().instructions[1], Instr::NoOp));
    assert!(diagnostics.iter().any(|d| d.contains("uradar")));

    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_001, 1, 0, 0.0);
    ubind_insert_unit(&world, 3_000_002, 2, 0, 0.0);
    let state = privileged_tick_bound(
        &world,
        pos,
        "uradar enemy any any distance 0 1 result",
        false,
        Some(3_000_001),
    );
    let result = state.program.var_index("result").unwrap();
    assert_eq!(
        state.vars[result].objval,
        LObject::Unit(3_000_002),
        "uradar from @unit finds the enemy"
    );
}

#[test]
fn status_apply_and_clear_are_privileged() {
    let program = compile("status false wet unit 10").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::ApplyStatus(ApplyStatusSpec { clear: false, .. })
    ));
    let program = compile("status true wet unit 10").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::ApplyStatus(ApplyStatusSpec { clear: true, .. })
    ));
    let (program, diagnostics) = compile_report("set x 1\nstatus");
    assert!(matches!(program.unwrap().instructions[1], Instr::NoOp));
    assert!(diagnostics.iter().any(|d| d.contains("status")));

    let (world, pos) = ubind_world(1);
    ubind_insert_unit(&world, 3_000_001, 1, 0, 0.0);
    privileged_tick_bound(&world, pos, "status false wet u 10", true, Some(3_000_001));
    let unit = world.enemies.get(&3_000_001).unwrap();
    assert!(
        unit.statuses
            .iter()
            .any(|entry| entry.effect == crate::game::status::STATUS_WET),
        "privileged status apply writes wet"
    );
    drop(unit);
    privileged_tick_bound(&world, pos, "status true wet u 10", true, Some(3_000_001));
    let unit = world.enemies.get(&3_000_001).unwrap();
    assert!(
        unit.statuses
            .iter()
            .all(|entry| entry.effect != crate::game::status::STATUS_WET),
        "privileged status clear removes wet"
    );
    drop(unit);
    privileged_tick_bound(
        &world,
        pos,
        "status false burning u 10",
        false,
        Some(3_000_001),
    );
    assert!(
        world.enemies.get(&3_000_001).unwrap().statuses.is_empty(),
        "unprivileged status is a no-op"
    );
    privileged_tick(&world, pos, "status false wet 0 10", true);
}

#[test]
fn spawnwave_setrule_explosion_setprop_mutate_world() {
    let program = compile("spawnwave 10 10 true").unwrap();
    assert!(matches!(program.instructions[0], Instr::SpawnWave(_, _, _)));
    let program = compile("setrule wave 5").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::SetRule(SetRuleSpec {
            rule: LogicRule::Wave,
            ..
        })
    ));
    let program = compile("explosion @crux 10 10 5 50 true true true true").unwrap();
    assert!(matches!(program.instructions[0], Instr::Explosion(_)));
    let program = compile("setprop @health u 10").unwrap();
    assert!(matches!(
        program.instructions[0],
        Instr::SetProp(SetPropSpec {
            key: SetPropKey::Access(LAccess::Health),
            ..
        })
    ));
    let (program, diagnostics) = compile_report("set x 1\nsetrule nope 1\nexplosion a");
    let program = program.unwrap();
    assert!(matches!(program.instructions[1], Instr::NoOp));
    assert!(matches!(program.instructions[2], Instr::NoOp));
    assert!(diagnostics.iter().any(|d| d.contains("setrule")));
    assert!(diagnostics.iter().any(|d| d.contains("explosion")));

    let mut world = logic_test_world("p111-world");
    if world.enemy_spawns.is_empty() {
        world.enemy_spawns.push((5, 5));
    }
    let pos = (10 << 16) | 10;
    world.tiles.insert(
        pos,
        crate::network::world::DynamicTile {
            position: pos,
            block: 442,
            team: 1,
            health: 500.0,
            ..Default::default()
        },
    );
    {
        let mut rules = world.wave_rules.write();
        rules.spawn_groups = vec![crate::network::units::MapSpawnGroup {
            unit_type: 0,
            begin: 0,
            end: u32::MAX,
            spacing: 1,
            max: 8,
            scaling: 0.0,
            shields: 0.0,
            shield_scaling: 0.0,
            unit_amount: 1,
            spawn: -1,
            effect: -1,
        }];
    }
    let world = std::sync::Arc::new(world);

    privileged_tick(&world, pos, "setrule wave 4", true);
    assert_eq!(
        world
            .game_state
            .wave
            .load(std::sync::atomic::Ordering::Relaxed),
        4
    );
    privileged_tick(&world, pos, "setrule unitCap 20", true);
    assert_eq!(world.wave_rules.read().unit_cap, 20);
    privileged_tick(&world, pos, "setrule waveSpacing 2", true);
    assert_eq!(world.wave_rules.read().wave_spacing, 120.0);
    privileged_tick(&world, pos, "setrule waves true", true);
    assert!(world.wave_rules.read().waves_enabled);
    privileged_tick(&world, pos, "setrule unitHealth 3 @sharded", true);
    assert_eq!(
        world.wave_rules.read().team_rule(1).unit_health_multiplier,
        3.0
    );
    privileged_tick(&world, pos, "setrule ban conveyor", true);
    assert!(world.wave_rules.read().block_banned(257));
    privileged_tick(&world, pos, "setrule wave 4", false);
    assert_eq!(
        world
            .game_state
            .wave
            .load(std::sync::atomic::Ordering::Relaxed),
        4,
        "unprivileged setrule does not mutate"
    );

    let before = world.enemies.len();
    privileged_tick(&world, pos, "spawnwave 10 10 false", true);
    assert!(
        world.enemies.len() > before,
        "non-natural spawnwave spawns groups at the tile"
    );
    assert_eq!(
        world
            .game_state
            .wave
            .load(std::sync::atomic::Ordering::Relaxed),
        4,
        "non-natural spawnwave does not increment wave"
    );
    privileged_tick(&world, pos, "spawnwave 10 10 true", true);
    assert_eq!(
        world
            .game_state
            .wave
            .load(std::sync::atomic::Ordering::Relaxed),
        5,
        "natural spawnwave increments like skipWave"
    );
    privileged_tick(&world, pos, "spawnwave 10 10 true", false);
    assert_eq!(
        world
            .game_state
            .wave
            .load(std::sync::atomic::Ordering::Relaxed),
        5,
        "unprivileged spawnwave is a no-op"
    );

    let unit_id = world
        .next_enemy_id
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    ubind_insert_unit(&world, unit_id, 1, 0, 0.0);
    privileged_tick_bound(
        &world,
        pos,
        "setprop @health u 10\nsetprop @flag u 42",
        true,
        Some(unit_id),
    );
    let unit = world.enemies.get(&unit_id).unwrap();
    assert_eq!(unit.health, 10.0);
    assert_eq!(unit.flag, 42.0);
    drop(unit);
    privileged_tick_bound(&world, pos, "setprop @health u 1", false, Some(unit_id));
    assert_eq!(
        world.enemies.get(&unit_id).unwrap().health,
        10.0,
        "unprivileged setprop is a no-op"
    );

    privileged_tick(
        &world,
        pos,
        "explosion @crux 10 10 5 50 true true true false",
        true,
    );
    let health_after = world.enemies.get(&unit_id).unwrap().health;
    assert!(
        health_after < 10.0,
        "explosion damages units of other teams"
    );
    privileged_tick(
        &world,
        pos,
        "explosion @crux 10 10 5 50 true true true false",
        false,
    );
    assert_eq!(
        world.enemies.get(&unit_id).unwrap().health,
        health_after,
        "unprivileged explosion is a no-op"
    );
}

#[test]
fn test_logic_sensor_flying() {
    use crate::network::world::EnemyUnit;
    let world = logic_test_world("maze");
    let grounded = EnemyUnit {
        id: 101,
        unit_type: 0,
        elevation: 0.0,
        x: 10.0,
        y: 10.0,
        team: 1,
        health: 100.0,
        ..Default::default()
    };
    let flying = EnemyUnit {
        id: 102,
        unit_type: 15,
        elevation: 1.0,
        x: 20.0,
        y: 20.0,
        team: 1,
        health: 100.0,
        ..Default::default()
    };
    world.enemies.insert(101, grounded);
    world.enemies.insert(102, flying);
    let grounded_flare = EnemyUnit {
        id: 103,
        unit_type: 15,
        elevation: 0.0,
        x: 30.0,
        y: 30.0,
        team: 1,
        health: 100.0,
        ..Default::default()
    };
    world.enemies.insert(103, grounded_flare);
    let view = WorldView {
        world: &world,
        processor_pos: (1 << 16) | 1,
        out: &crate::network::outbound::NOOP,
    };

    let sense_ground = view.unit_sensor(101, LAccess::Flying);
    let sense_fly = view.unit_sensor(102, LAccess::Flying);

    assert_eq!(sense_ground, SensorValue::Num(0.0));
    assert_eq!(sense_fly, SensorValue::Num(1.0));
    assert_eq!(
        view.unit_sensor(103, LAccess::Flying),
        SensorValue::Num(0.0),
        "grounded flying-type unit must read flying=0"
    );
}

fn spawn_logic_world(name: &str) -> std::sync::Arc<crate::network::world::DynamicWorld> {
    let world = std::sync::Arc::new(logic_test_world(name));
    world.wave_rules.write().disable_unit_cap = true;
    world
}

#[test]
fn logic_spawn_respects_explicit_team() {
    let world = spawn_logic_world("spawn-explicit-team");
    let pos = (1 << 16) | 1;
    privileged_tick(&world, pos, "spawn @dagger 80 80 0 @crux result true", true);
    assert!(
        world
            .enemies
            .iter()
            .any(|u| u.unit_type == 0 && u.team == 2),
        "spawn @crux must create team-2 unit, not sharded"
    );
    assert!(
        !world
            .enemies
            .iter()
            .any(|u| u.unit_type == 0 && u.team == 1),
        "must not default to team 1 when @crux specified"
    );
}

#[test]
fn logic_spawn_team_from_runtime_variable() {
    let world = spawn_logic_world("spawn-runtime-team");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set t 2\nspawn @dagger 80 80 0 t result true",
        true,
    );
    assert!(world.enemies.iter().any(|u| u.team == 2));
}

#[test]
fn logic_spawn_invalid_team_does_not_spawn() {
    let world = spawn_logic_world("spawn-invalid-team");
    let pos = (1 << 16) | 1;
    let before = world.enemies.len();
    privileged_tick(
        &world,
        pos,
        "set t -1\nspawn @dagger 80 80 0 t result true",
        true,
    );
    assert_eq!(world.enemies.len(), before);
}

#[test]
fn logic_spawn_respects_unit_creation_gate() {
    let mut world = logic_test_world("spawn-cap-gate");
    world.wave_rules.write().disable_unit_cap = false;
    world.wave_rules.write().unit_cap = 0;
    world.sharded_unit_cap = 0;
    let world = std::sync::Arc::new(world);
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "spawn @dagger 80 80 0 @sharded result true",
        true,
    );
    assert_eq!(world.enemies.iter().filter(|u| u.team == 1).count(), 0);
}

#[test]
fn logic_spawn_effect_true_applies_unmoving_and_invincible() {
    let world = spawn_logic_world("spawn-effect-true");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "spawn @dagger 80 80 0 @sharded result true",
        true,
    );
    let unit = world.enemies.iter().next().expect("spawned");
    assert!(
        unit.statuses.iter().any(|s| s.effect == 3),
        "effect=true must apply unmoving (id 3)"
    );
    assert!(
        unit.statuses.iter().any(|s| s.effect == 21),
        "effect=true must apply invincible (id 21)"
    );
}

#[test]
fn logic_spawn_effect_false_does_not_apply_spawn_statuses() {
    let world = spawn_logic_world("spawn-effect-false");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "spawn @dagger 80 80 0 @sharded result false",
        true,
    );
    let unit = world.enemies.iter().next().expect("spawned");
    assert!(!unit.statuses.iter().any(|s| s.effect == 3));
    assert!(!unit.statuses.iter().any(|s| s.effect == 21));
}

#[test]
fn logic_spawn_effect_runtime_variable_false() {
    let world = spawn_logic_world("spawn-effect-runtime-false");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set fx 0\nspawn @dagger 80 80 0 @crux result fx",
        true,
    );
    let unit = world.enemies.iter().next().expect("unit must spawn");
    assert_eq!(unit.team, 2);
    assert!(
        !unit
            .statuses
            .iter()
            .any(|s| s.effect == 3 || s.effect == 21),
        "runtime effect=0 must not apply spawn statuses (token 'fx' is not compile-time true)"
    );
}

#[test]
fn logic_spawn_effect_runtime_variable_true() {
    let world = spawn_logic_world("spawn-effect-runtime-true");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set fx 1\nspawn @dagger 80 80 0 @crux result fx",
        true,
    );
    let unit = world.enemies.iter().next().expect("unit must spawn");
    assert_eq!(unit.team, 2);
    assert!(unit.statuses.iter().any(|s| s.effect == 3));
    assert!(unit.statuses.iter().any(|s| s.effect == 21));
}

#[test]
fn logic_spawn_effect_runtime_variable_changes_between_executions() {
    let world = spawn_logic_world("spawn-effect-runtime-changes");
    let pos = (1 << 16) | 1;
    let program = compile("spawn @dagger 80 80 0 @sharded result fx\nstop").unwrap();
    let fx_idx = program.var_index("fx").expect("fx operand");

    let mut state = ExecutorState::new(program, vec![]);
    state.privileged = true;
    state.vars[fx_idx] = LVar::new_num("fx", 0.0);
    let connections = dashmap::DashMap::new();
    let view = WorldView {
        world: &world,
        processor_pos: pos,
        out: &connections,
    };
    state.run_tick(Some(&view), 16);
    let first = world.enemies.iter().next().expect("first spawn");
    assert!(
        !first
            .statuses
            .iter()
            .any(|s| s.effect == 3 || s.effect == 21),
        "first execution with fx=0 must skip spawn statuses"
    );
    let first_id = first.id;
    drop(first);
    world.enemies.remove(&first_id);

    state.counter = 0;
    state.yield_flag = false;
    state.vars[fx_idx] = LVar::new_num("fx", 1.0);
    state.run_tick(Some(&view), 16);
    let second = world.enemies.iter().next().expect("second spawn");
    assert!(
        second.statuses.iter().any(|s| s.effect == 3),
        "same compiled program must re-evaluate effect at runtime"
    );
    assert!(second.statuses.iter().any(|s| s.effect == 21));
}

#[test]
fn logic_spawn_result_references_spawned_unit() {
    let world = spawn_logic_world("spawn-result-ref");
    let pos = (1 << 16) | 1;
    let state = privileged_tick(
        &world,
        pos,
        "spawn @dagger 80 80 0 @sharded result true",
        true,
    );
    let spawned_id = world.enemies.iter().next().map(|u| u.id).expect("spawned");
    let result_idx = state.program.var_index("result").unwrap();
    match &state.vars[result_idx].objval {
        LObject::Unit(id) => assert_eq!(*id, spawned_id),
        other => panic!("result var expected Unit object, got {other:?}"),
    }
}

#[test]
fn logic_spawn_unprivileged_is_no_op() {
    let world = spawn_logic_world("spawn-unprivileged");
    let pos = (1 << 16) | 1;
    let before = world.enemies.len();
    privileged_tick(
        &world,
        pos,
        "spawn @dagger 10 10 0 @sharded result true",
        false,
    );
    assert_eq!(world.enemies.len(), before);
}

#[test]
fn logic_spawn_uses_tile_to_world_conversion() {
    let world = spawn_logic_world("spawn-unconv");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "spawn @dagger 10 10 0 @sharded result true",
        true,
    );
    let unit = world.enemies.iter().next().expect("spawned");
    // Official: World.unconv(10) + Mathf.range(0.01) → [80-0.01, 80+0.01).
    let x_band = (80.0 - SPAWN_POSITION_JITTER)..(80.0 + SPAWN_POSITION_JITTER);
    let y_band = (80.0 - SPAWN_POSITION_JITTER)..(80.0 + SPAWN_POSITION_JITTER);
    assert!(
        x_band.contains(&unit.x),
        "x must be tile*8 + Mathf.range(0.01), got {}",
        unit.x
    );
    assert!(
        y_band.contains(&unit.y),
        "y must be tile*8 + Mathf.range(0.01), got {}",
        unit.y
    );
}

#[test]
fn logic_spawn_position_has_official_jitter_bound() {
    // Arc: range(r) = random(-r, r) = -r + nextFloat()*2r, nextFloat ∈ [0,1)
    // → interval [-r, r).
    let band = -SPAWN_POSITION_JITTER..SPAWN_POSITION_JITTER;
    assert_eq!(
        mathf_range_from_unit(0.0, SPAWN_POSITION_JITTER),
        -SPAWN_POSITION_JITTER
    );
    for i in 0..1024 {
        let u = i as f32 / 1024.0;
        let j = mathf_range_from_unit(u, SPAWN_POSITION_JITTER);
        assert!(
            band.contains(&j),
            "unit={u} → jitter={j} outside [-0.01, 0.01)"
        );
    }
    for _ in 0..256 {
        let j = mathf_range(SPAWN_POSITION_JITTER);
        assert!(
            band.contains(&j),
            "runtime Mathf.range sample {j} outside [-0.01, 0.01)"
        );
    }
}

#[test]
fn logic_spawn_jitter_is_applied_to_x_and_y() {
    let (x, y) = spawn_world_position(10.0, 20.0, 0.005, -0.003);
    assert!(
        (x - 80.005).abs() < 1e-6,
        "x must receive its own jitter, got {x}"
    );
    assert!(
        (y - 159.997).abs() < 1e-6,
        "y must receive its own jitter, got {y}"
    );
}

#[test]
fn logic_spawn_still_uses_logic_tiles_times_8() {
    // Pure contract: tiles × 8 is the unconv base before jitter.
    let (x0, y0) = spawn_world_position(10.0, 15.0, 0.0, 0.0);
    assert_eq!(x0, 80.0);
    assert_eq!(y0, 120.0);

    let world = spawn_logic_world("spawn-tiles-times-8");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "spawn @dagger 10 15 0 @sharded result true",
        true,
    );
    let unit = world.enemies.iter().next().expect("spawned");
    let x_band = (80.0 - SPAWN_POSITION_JITTER)..(80.0 + SPAWN_POSITION_JITTER);
    let y_band = (120.0 - SPAWN_POSITION_JITTER)..(120.0 + SPAWN_POSITION_JITTER);
    assert!(
        x_band.contains(&unit.x),
        "expected ~logic_x*8, got {}",
        unit.x
    );
    assert!(
        y_band.contains(&unit.y),
        "expected ~logic_y*8, got {}",
        unit.y
    );
}

#[test]
fn logic_spawn_failure_preserves_result() {
    let mut world = logic_test_world("spawn-failure-result");
    world.wave_rules.write().disable_unit_cap = false;
    world.wave_rules.write().unit_cap = 0;
    world.sharded_unit_cap = 0;
    let world = std::sync::Arc::new(world);
    let pos = (1 << 16) | 1;
    let state = privileged_tick_bound(
        &world,
        pos,
        "set result 42\nspawn @dagger 10 10 0 @sharded result true",
        true,
        None,
    );
    assert_eq!(world.enemies.iter().filter(|u| u.team == 1).count(), 0);
    let result_idx = state.program.var_index("result").unwrap();
    assert!(
        !state.vars[result_idx].isobj,
        "failed spawn must not overwrite result with null"
    );
    assert_eq!(state.vars[result_idx].numval, 42.0);
}

#[test]
fn logic_spawn_wave_team_bypasses_cap_in_survival() {
    let mut world = logic_test_world("spawn-wave-cap");
    world.wave_rules.write().disable_unit_cap = false;
    world.wave_rules.write().unit_cap = 0;
    world.sharded_unit_cap = 0;
    world.wave_rules.write().wave_team = 2;
    let world = std::sync::Arc::new(world);
    let pos = (1 << 16) | 1;
    privileged_tick(&world, pos, "spawn @dagger 10 10 0 @crux result true", true);
    assert!(world.enemies.iter().any(|u| u.team == 2));
}

#[test]
fn logic_spawn_runtime_unit_type_variable() {
    let world = spawn_logic_world("spawn-runtime-type");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set t @dagger\nspawn t 10 10 0 @sharded result true",
        true,
    );
    assert!(world
        .enemies
        .iter()
        .any(|u| u.unit_type == 0 && u.team == 1));
}

#[test]
fn logic_spawn_direct_numeric_literal_is_invalid() {
    let world = spawn_logic_world("spawn-direct-numeric-type");
    let pos = (1 << 16) | 1;
    let state = privileged_tick(
        &world,
        pos,
        "set result 42\nspawn 0 10 10 0 @sharded result true",
        true,
    );
    assert!(world.enemies.is_empty(), "direct numeric ID must not spawn");
    let result = state.vars[state.program.var_index("result").unwrap()].clone();
    assert!(!result.isobj);
    assert_eq!(result.numval, 42.0);
}

#[test]
fn logic_spawn_at_numeric_literal_is_invalid() {
    let world = spawn_logic_world("spawn-at-numeric-type");
    let pos = (1 << 16) | 1;
    privileged_tick(&world, pos, "spawn @0 10 10 0 @sharded result true", true);
    assert!(world.enemies.is_empty(), "@numeric must not spawn");
}

#[test]
fn logic_spawn_bare_name_is_invalid() {
    let world = spawn_logic_world("spawn-bare-name-type");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "spawn dagger 10 10 0 @sharded result true",
        true,
    );
    assert!(world.enemies.is_empty(), "bare unit name must not spawn");
}

#[test]
fn logic_spawn_numeric_unit_type_is_invalid() {
    let world = spawn_logic_world("spawn-runtime-numeric-type");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set t 0\nspawn t 10 10 0 @sharded result true",
        true,
    );
    assert!(
        world.enemies.is_empty(),
        "numeric content ID must not spawn"
    );
}

#[test]
fn logic_spawn_string_unit_type_is_invalid() {
    let world = spawn_logic_world("spawn-runtime-string-type");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set t \"dagger\"\nspawn t 10 10 0 @sharded result true",
        true,
    );
    assert!(world.enemies.is_empty(), "string name must not spawn");
}

#[test]
fn logic_spawn_quoted_string_literal_is_invalid() {
    let world = spawn_logic_world("spawn-quoted-string-type");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "spawn \"dagger\" 10 10 0 @sharded result true",
        true,
    );
    assert!(
        world.enemies.is_empty(),
        "quoted unit names are String objects, not UnitType objects"
    );
}

#[test]
fn logic_spawn_null_unit_type_is_invalid() {
    let world = spawn_logic_world("spawn-runtime-null-type");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set t null\nspawn t 10 10 0 @sharded result true",
        true,
    );
    assert!(world.enemies.is_empty(), "null must not spawn");
}

#[test]
fn logic_spawn_non_unittype_object_is_invalid() {
    let world = spawn_logic_world("spawn-runtime-non-unittype");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set t @sharded\nspawn t 10 10 0 @sharded result true",
        true,
    );
    assert!(world.enemies.is_empty(), "Team object must not spawn");

    let world = spawn_logic_world("spawn-runtime-unit-object");
    privileged_tick(
        &world,
        pos,
        "spawn @dagger 10 10 0 @sharded t true\nspawn t 10 10 0 @sharded result true",
        true,
    );
    assert_eq!(
        world.enemies.len(),
        1,
        "Unit object must not be accepted as a UnitType"
    );
}

#[test]
fn can_create_unit_under_cap_true() {
    let mut world = logic_test_world("can-create-under-cap");
    world.sharded_unit_cap = 2;
    world.wave_rules.write().disable_unit_cap = false;
    assert!(crate::network::economy::can_create_unit(&world, 1, 0));
}

#[test]
fn can_create_unit_at_cap_false() {
    let mut world = logic_test_world("can-create-at-cap");
    world.sharded_unit_cap = 1;
    world.wave_rules.write().disable_unit_cap = false;
    world.enemies.insert(
        3_100_001,
        crate::network::world::EnemyUnit {
            id: 3_100_001,
            team: 1,
            unit_type: 0,
            ..Default::default()
        },
    );
    assert!(!crate::network::economy::can_create_unit(&world, 1, 0));
}

#[test]
fn can_create_banned_unit_false() {
    let mut world = logic_test_world("can-create-banned");
    world.sharded_unit_cap = 2;
    {
        let mut rules = world.wave_rules.write();
        rules.disable_unit_cap = false;
        rules.banned_units = vec![0];
    }
    assert!(!crate::network::economy::can_create_unit(&world, 1, 0));
}

#[test]
fn can_create_use_unit_cap_false_ignores_cap() {
    let mut world = logic_test_world("can-create-no-cap");
    world.sharded_unit_cap = 1;
    {
        let mut rules = world.wave_rules.write();
        rules.disable_unit_cap = false;
    }
    world.enemies.insert(
        3_100_002,
        crate::network::world::EnemyUnit {
            id: 3_100_002,
            team: 1,
            unit_type: 63,
            ..Default::default()
        },
    );
    assert!(
        crate::network::economy::can_create_unit(&world, 1, 63),
        "useUnitCap=false must bypass an at-cap count"
    );
    world.enemies.insert(
        3_100_003,
        crate::network::world::EnemyUnit {
            id: 3_100_003,
            team: 1,
            unit_type: 63,
            ..Default::default()
        },
    );
    assert!(
        crate::network::economy::can_create_unit(&world, 1, 63),
        "useUnitCap=false must bypass an over-cap count"
    );
}

#[test]
fn can_create_use_unit_cap_false_ignores_ban() {
    let mut world = logic_test_world("can-create-no-cap-banned");
    world.sharded_unit_cap = 0;
    {
        let mut rules = world.wave_rules.write();
        rules.disable_unit_cap = false;
        // Java's `!type.useUnitCap || (...)` short-circuits the ban check too.
        rules.banned_units = vec![63];
    }
    assert!(
        crate::network::economy::can_create_unit(&world, 1, 63),
        "useUnitCap=false must bypass a ban"
    );
}

#[test]
fn logic_spawn_rejects_internal_block_unit_type() {
    let world = spawn_logic_world("spawn-internal-block");
    let pos = (1 << 16) | 1;
    let before = world.enemies.len();
    privileged_tick(
        &world,
        pos,
        "spawn @block 10 10 0 @sharded result true",
        true,
    );
    assert_eq!(world.enemies.len(), before);
}

#[test]
fn logic_spawn_team_from_team_content_object() {
    let world = spawn_logic_world("spawn-team-object");
    let pos = (1 << 16) | 1;
    privileged_tick(&world, pos, "spawn @dagger 10 10 0 @crux result true", true);
    assert!(world.enemies.iter().any(|u| u.team == 2));
}

// ------------------------------------------------------------------
// LVar coercion (v159.7 LVar.bool / LVar.num / LVar.team)
// ------------------------------------------------------------------

#[test]
fn lvar_bool_numeric_zero_false() {
    assert!(!lvar_bool(&LVar::new_num("z", 0.0)));
    assert!(!lvar_bool(&LVar::new_num("z", -0.000001)));
    assert!(!lvar_bool(&LVar::new_num("z", 0.000009)));
}

#[test]
fn lvar_bool_numeric_nonzero_true() {
    assert!(lvar_bool(&LVar::new_num("z", 0.00001)));
    assert!(lvar_bool(&LVar::new_num("z", -0.00001)));
    assert!(lvar_bool(&LVar::new_num("z", -1.0)));
    assert!(lvar_bool(&LVar::new_num("z", 1.0)));
}

#[test]
fn lvar_bool_null_false() {
    assert!(!lvar_bool(&LVar::new_obj("n", LObject::Null)));
    let mut v = LVar::new_var("n");
    v.objval = LObject::Null;
    assert!(!lvar_bool(&v));
}

#[test]
fn lvar_bool_nonnull_string_true() {
    assert!(lvar_bool(&LVar::new_obj("s", LObject::Str(String::new()))));
    assert!(lvar_bool(&LVar::new_obj(
        "s",
        LObject::Str("hello".to_string())
    )));
}

#[test]
fn lvar_bool_nonnull_team_true() {
    assert!(lvar_bool(&LVar::new_obj("t", LObject::Team(2))));
}

#[test]
fn lvar_bool_nonnull_unit_true() {
    assert!(lvar_bool(&LVar::new_obj("u", LObject::Unit(42))));
}

#[test]
fn lvar_num_null_is_zero() {
    assert_eq!(LVar::new_obj("n", LObject::Null).num(), 0.0);
}

#[test]
fn lvar_num_nonnull_object_is_one() {
    assert_eq!(
        LVar::new_obj("s", LObject::Str("hello".to_string())).num(),
        1.0
    );
    assert_eq!(LVar::new_obj("t", LObject::Team(2)).num(), 1.0);
    assert_eq!(LVar::new_obj("u", LObject::Unit(1)).num(), 1.0);
}

#[test]
fn lvar_num_normal_numeric_preserved() {
    assert_eq!(LVar::new_num("x", 3.5).num(), 3.5);
    assert_eq!(LVar::new_num("x", -2.0).num(), -2.0);
    assert_eq!(LVar::new_num("x", 0.0).num(), 0.0);
}

#[test]
fn lvar_num_invalid_numeric_matches_java_1597() {
    // Official LVar.invalid: Double.isNaN || Double.isInfinite
    assert!(lvar_invalid(f64::NAN));
    assert!(lvar_invalid(f64::INFINITY));
    assert!(lvar_invalid(f64::NEG_INFINITY));
    assert!(!lvar_invalid(0.0));
    assert!(!lvar_invalid(1.0));
    assert_eq!(LVar::new_num("n", f64::NAN).num(), 0.0);
    assert_eq!(LVar::new_num("n", f64::INFINITY).num(), 0.0);
    assert_eq!(LVar::new_num("n", f64::NEG_INFINITY).num(), 0.0);
}

#[test]
fn lvar_team_team_object_valid() {
    assert_eq!(lvar_team(&LVar::new_obj("t", LObject::Team(2))), Some(2));
    assert_eq!(lvar_team(&LVar::new_obj("t", LObject::Team(0))), Some(0));
}

#[test]
fn lvar_team_numeric_valid() {
    assert_eq!(lvar_team(&LVar::new_num("t", 2.0)), Some(2));
    assert_eq!(lvar_team(&LVar::new_num("t", 0.0)), Some(0));
    assert_eq!(lvar_team(&LVar::new_num("t", 255.0)), Some(255));
}

#[test]
fn lvar_team_numeric_out_of_range_invalid() {
    assert_eq!(lvar_team(&LVar::new_num("t", -1.0)), None);
    assert_eq!(lvar_team(&LVar::new_num("t", 256.0)), None);
    assert_eq!(lvar_team(&LVar::new_num("t", 1000.0)), None);
}

#[test]
fn lvar_team_string_object_invalid() {
    assert_eq!(
        lvar_team(&LVar::new_obj("t", LObject::Str("crux".to_string()))),
        None,
        "String objects must not be reparsed into Teams at runtime"
    );
    assert_eq!(lvar_team(&LVar::new_obj("t", LObject::Null)), None);
    assert_eq!(lvar_team(&LVar::new_obj("t", LObject::Unit(1))), None);
}

#[test]
fn logic_spawn_effect_nonnull_object_is_true() {
    let world = spawn_logic_world("spawn-effect-obj-true");
    let pos = (1 << 16) | 1;
    // Empty string is non-null → bool() true → spawn statuses applied.
    privileged_tick(
        &world,
        pos,
        "set fx \"\"\nspawn @dagger 80 80 0 @sharded result fx",
        true,
    );
    let unit = world.enemies.iter().next().expect("unit must spawn");
    assert!(
        unit.statuses.iter().any(|s| s.effect == 3),
        "non-null object effect must apply unmoving"
    );
    assert!(
        unit.statuses.iter().any(|s| s.effect == 21),
        "non-null object effect must apply invincible"
    );
}

#[test]
fn logic_spawn_runtime_string_team_is_invalid() {
    let world = spawn_logic_world("spawn-string-team");
    let pos = (1 << 16) | 1;
    let before = world.enemies.len();
    privileged_tick(
        &world,
        pos,
        "set t \"crux\"\nspawn @dagger 80 80 0 t result 0",
        true,
    );
    assert_eq!(
        world.enemies.len(),
        before,
        "runtime String \"crux\" must not resolve as Team"
    );
}

#[test]
fn logic_spawn_runtime_team_object_is_valid() {
    let world = spawn_logic_world("spawn-runtime-team-obj");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set t @crux\nspawn @dagger 80 80 0 t result 0",
        true,
    );
    assert!(
        world.enemies.iter().any(|u| u.team == 2),
        "@crux Team object must remain valid at runtime"
    );
}

#[test]
fn logic_spawn_numeric_team_is_valid() {
    let world = spawn_logic_world("spawn-numeric-team");
    let pos = (1 << 16) | 1;
    privileged_tick(
        &world,
        pos,
        "set t 2\nspawn @dagger 80 80 0 t result 0",
        true,
    );
    assert!(world.enemies.iter().any(|u| u.team == 2));
}
