import java.io.InputStream;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.files.Fi;
import arc.math.geom.Vec2;
import arc.struct.StringMap;
import mindustry.Vars;
import mindustry.ai.UnitCommand;
import mindustry.ai.UnitStance;
import mindustry.ai.types.CommandAI;
import mindustry.ai.types.LogicAI;
import mindustry.content.Blocks;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.Logic;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.game.Waves;
import mindustry.gen.Building;
import mindustry.gen.Groups;
import mindustry.gen.Teamc;
import mindustry.gen.Unit;
import mindustry.io.SaveIO;
import mindustry.logic.GlobalVars;
import mindustry.logic.LUnitControl;
import mindustry.mod.Mods;
import mindustry.world.Tile;
import mindustry.world.Tiles;

/**
 * P1-C1 differential probe: what unit controller state survives a full
 * SaveIO.save / SaveIO.load round-trip on desktop.jar 158.1.
 *
 * Exercises {@code TypeIO.writeController}/{@code readController} together
 * with {@code UnitComp.afterRead} (controller reset unless keepState) and
 * {@code UnitComp.afterReadAll} → {@code CommandAI.afterRead}.
 *
 * Scenarios:
 *   command_roundtrip   — CommandAI with command, active pos + attack unit,
 *                         heterogeneous queue (building/unit/vec2) and two
 *                         stances before save.
 *   logic_roundtrip     — LogicAI with processor, non-default controlTimer,
 *                         move mode and coordinates before save.
 *   missing_attack_unit — attackTarget references a unit id never written.
 *   missing_queue_unit  — queued unit id absent from the save.
 *   building_attack_removed — active building attack target tile cleared
 *                         before save.
 *   building_queue_removed  — queued building tile cleared before save.
 *
 * Version gate: refuses to run unless the classpath version.properties
 * reports the official 158.1 build.
 */
public final class ParControllerSave158 {
    static final int WORLD = 16;
    static final float NON_DEFAULT_TIMER = 123.45f;
    static final int PHANTOM_UNIT_ID = 999_999;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParControllerSave158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

        boot();
        Fi saveFile = Core.files.local("parity-controller-save.msav");
        if (saveFile.exists()) saveFile.delete();

        Map<String, Object> scenarios = new LinkedHashMap<>();
        scenarios.put("command_roundtrip", runCommandRoundtrip(saveFile));
        resetWorld(saveFile);
        scenarios.put("logic_roundtrip", runLogicRoundtrip(saveFile));
        resetWorld(saveFile);
        scenarios.put("missing_attack_unit", runMissingAttackUnit(saveFile));
        resetWorld(saveFile);
        scenarios.put("missing_queue_unit", runMissingQueueUnit(saveFile));
        resetWorld(saveFile);
        scenarios.put("building_attack_removed", runBuildingAttackRemoved(saveFile));
        resetWorld(saveFile);
        scenarios.put("building_queue_removed", runBuildingQueueRemoved(saveFile));

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("controller-save")).append(",\n");
        json.append("  \"tick\": 0,\n");
        appendScenarioMap(json, "scenarios", scenarios, false);
        appendOutcomes(json, scenarios);
        json.append("\n}\n");
        System.out.print(json);
    }

    // --- Scenarios -----------------------------------------------------------

    static Map<String, Object> runCommandRoundtrip(Fi saveFile) throws Exception {
        Unit foe = UnitTypes.dagger.create(Team.crux);
        foe.set(120f, 120f);
        foe.add();
        Unit unit = UnitTypes.mega.create(Team.sharded);
        unit.set(80f, 80f);
        unit.add();
        Vars.state.teams.updateTeamStats();
        CommandAI ai = commandAi(unit);
        Tile wallTile = Vars.world.tiles.get(8, 8);
        wallTile.setBlock(Blocks.copperWall, Team.crux, 0);
        Building wall = wallTile.build;

        ai.commandTarget(foe, true);
        materializeAttack(ai, foe);
        ai.commandQueue(wall);
        ai.commandQueue(foe);
        ai.commandQueue(new Vec2(400f, 400f));
        ai.setStance(UnitStance.pursueTarget);
        ai.setStance(UnitStance.holdFire);
        ai.command(UnitCommand.repairCommand);

        Map<String, Object> before = snapshotCommand(unit.id);
        int unitId = unit.id;
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Map<String, Object> after = snapshotCommand(unitId);
        return scenario(before, after);
    }

    static Map<String, Object> runLogicRoundtrip(Fi saveFile) throws Exception {
        Tile procTile = Vars.world.tiles.get(8, 8);
        procTile.setBlock(Blocks.microProcessor, Team.sharded, 0);
        Building proc = procTile.build;

        Unit unit = UnitTypes.dagger.create(Team.sharded);
        unit.set(80f, 80f);
        unit.add();
        LogicAI ai = new LogicAI();
        ai.controller = proc;
        ai.controlTimer = NON_DEFAULT_TIMER;
        ai.control = LUnitControl.move;
        ai.moveX = 200f;
        ai.moveY = 300f;
        ai.moveRad = 40f;
        ai.boost = true;
        ai.shoot = true;
        ai.aimControl = LUnitControl.target;
        unit.controller(ai);

        Map<String, Object> before = snapshotLogic(unit.id);
        int unitId = unit.id;
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Map<String, Object> after = snapshotLogic(unitId);
        return scenario(before, after);
    }

    static Map<String, Object> runMissingAttackUnit(Fi saveFile) throws Exception {
        Unit unit = spawnFlare(80f, 80f);
        CommandAI ai = commandAi(unit);
        Unit phantom = UnitTypes.dagger.create(Team.crux);
        phantom.set(120f, 120f);
        phantom.add();
        int phantomId = phantom.id;
        ai.command(UnitCommand.moveCommand);
        ai.commandTarget(phantom, true);
        materializeAttack(ai, phantom);
        phantom.remove();

        Map<String, Object> before = snapshotCommand(unit.id);
        before.put("phantom_id", phantomId);
        int unitId = unit.id;
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Map<String, Object> after = snapshotCommand(unitId);
        return scenario(before, after);
    }

    static Map<String, Object> runMissingQueueUnit(Fi saveFile) throws Exception {
        Unit unit = spawnFlare(80f, 80f);
        CommandAI ai = commandAi(unit);
        Unit foe = UnitTypes.dagger.create(Team.crux);
        foe.set(120f, 120f);
        foe.add();
        ai.commandPosition(new Vec2(100f, 100f));
        ai.commandQueue(foe);
        ai.commandQueue(new Vec2(200f, 200f));
        int foeId = foe.id;
        foe.remove();

        Map<String, Object> before = snapshotCommand(unit.id);
        before.put("removed_queue_unit_id", foeId);
        int unitId = unit.id;
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Map<String, Object> after = snapshotCommand(unitId);
        return scenario(before, after);
    }

    static Map<String, Object> runBuildingAttackRemoved(Fi saveFile) throws Exception {
        Unit unit = spawnFlare(80f, 80f);
        CommandAI ai = commandAi(unit);
        Tile wallTile = Vars.world.tiles.get(8, 8);
        wallTile.setBlock(Blocks.copperWall, Team.crux, 0);
        Building wall = wallTile.build;
        int wallPos = wall.pos();
        ai.command(UnitCommand.rebuildCommand);
        ai.commandTarget(wall, true);
        materializeAttack(ai, wall);
        wallTile.setBlock(Blocks.air, Team.derelict, 0);

        Map<String, Object> before = snapshotCommand(unit.id);
        before.put("wall_pos", wallPos);
        int unitId = unit.id;
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Map<String, Object> after = snapshotCommand(unitId);
        return scenario(before, after);
    }

    static Map<String, Object> runBuildingQueueRemoved(Fi saveFile) throws Exception {
        Unit unit = spawnFlare(80f, 80f);
        CommandAI ai = commandAi(unit);
        Tile wallTile = Vars.world.tiles.get(8, 8);
        wallTile.setBlock(Blocks.copperWall, Team.crux, 0);
        Building wall = wallTile.build;
        int wallPos = wall.pos();
        ai.commandPosition(new Vec2(100f, 100f));
        ai.commandQueue(wall);
        ai.commandQueue(new Vec2(300f, 300f));
        wallTile.setBlock(Blocks.air, Team.derelict, 0);

        Map<String, Object> before = snapshotCommand(unit.id);
        before.put("wall_pos", wallPos);
        int unitId = unit.id;
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Map<String, Object> after = snapshotCommand(unitId);
        return scenario(before, after);
    }

    // --- Snapshots -----------------------------------------------------------

    static Map<String, Object> scenario(Map<String, Object> before, Map<String, Object> after) {
        Map<String, Object> out = new LinkedHashMap<>();
        out.put("before", before);
        out.put("after", after);
        return out;
    }

    static Map<String, Object> snapshotCommand(int unitId) {
        Unit unit = Groups.unit.getByID(unitId);
        Map<String, Object> out = new LinkedHashMap<>();
        if (unit == null) {
            out.put("unit_found", false);
            return out;
        }
        out.put("unit_found", true);
        out.put("controller", controllerName(unit));
        if (!(unit.controller() instanceof CommandAI ai)) {
            out.put("is_command_ai", false);
            return out;
        }
        out.put("is_command_ai", true);
        out.put("command_id", ai.command == null ? -1 : (int) ai.command.id);
        out.put("has_target_pos", ai.targetPos != null);
        out.put("target_x", ai.targetPos == null ? 0f : ai.targetPos.x);
        out.put("target_y", ai.targetPos == null ? 0f : ai.targetPos.y);
        out.put("has_attack_target", ai.attackTarget != null);
        out.put("attack_kind", attackKind(ai.attackTarget));
        out.put("attack_building_pos", attackBuildingPos(ai.attackTarget));
        out.put("attack_unit_id", attackUnitId(ai.attackTarget));
        out.put("read_attack_target", ai.readAttackTarget);
        out.put("queue_size", ai.commandQueue.size);
        out.put("queue", queueEntries(ai));
        out.put("stances", enabledStances(ai));
        out.put("has_command", ai.hasCommand());
        out.put("logic_controllable", ai.isLogicControllable());
        return out;
    }

    static Map<String, Object> snapshotLogic(int unitId) {
        Unit unit = Groups.unit.getByID(unitId);
        Map<String, Object> out = new LinkedHashMap<>();
        if (unit == null) {
            out.put("unit_found", false);
            return out;
        }
        out.put("unit_found", true);
        out.put("controller", controllerName(unit));
        if (!(unit.controller() instanceof LogicAI ai)) {
            out.put("is_logic_ai", false);
            return out;
        }
        out.put("is_logic_ai", true);
        out.put("controller_pos", ai.controller == null ? -1 : ai.controller.pos());
        out.put("controller_valid", ai.controller != null && ai.controller.isValid());
        out.put("control_timer", ai.controlTimer);
        out.put("control_mode", ai.control.name());
        out.put("move_x", ai.moveX);
        out.put("move_y", ai.moveY);
        out.put("move_rad", ai.moveRad);
        out.put("boost", ai.boost);
        out.put("shoot", ai.shoot);
        out.put("aim_control", ai.aimControl.name());
        return out;
    }

    static List<Map<String, Object>> queueEntries(CommandAI ai) {
        List<Map<String, Object>> out = new ArrayList<>();
        for (int i = 0; i < ai.commandQueue.size; i++) {
            Object entry = ai.commandQueue.get(i);
            Map<String, Object> item = new LinkedHashMap<>();
            if (entry instanceof Building b) {
                item.put("kind", "building");
                item.put("pos", b.pos());
            } else if (entry instanceof Unit u) {
                item.put("kind", "unit");
                item.put("id", u.id);
            } else if (entry instanceof Vec2 v) {
                item.put("kind", "vec2");
                item.put("x", v.x);
                item.put("y", v.y);
            } else {
                item.put("kind", "other");
            }
            out.add(item);
        }
        return out;
    }

    static List<Integer> enabledStances(CommandAI ai) {
        List<Integer> out = new ArrayList<>();
        for (var stance : Vars.content.unitStances()) {
            if (ai.hasStance(stance)) out.add((int) stance.id);
        }
        return out;
    }

    static String attackKind(Teamc target) {
        if (target == null) return "none";
        if (target instanceof Building) return "building";
        if (target instanceof Unit) return "unit";
        return "other";
    }

    static int attackBuildingPos(Teamc target) {
        return target instanceof Building b ? b.pos() : -1;
    }

    static int attackUnitId(Teamc target) {
        return target instanceof Unit u ? u.id : -1;
    }

    static String controllerName(Unit unit) {
        return unit.controller().getClass().getSimpleName();
    }

    // --- World helpers -------------------------------------------------------

    static Unit spawnFlare(float x, float y) {
        Unit unit = UnitTypes.flare.create(Team.sharded);
        unit.set(x, y);
        unit.add();
        Vars.state.teams.updateTeamStats();
        return unit;
    }

    static CommandAI commandAi(Unit unit) {
        if (!(unit.controller() instanceof CommandAI ai)) {
            System.err.println("ParControllerSave158: expected CommandAI controller");
            System.exit(3);
        }
        return (CommandAI) unit.controller();
    }

    static void materializeAttack(CommandAI ai, Teamc target) {
        ai.attackTarget = target;
        if (ai.targetPos == null) ai.targetPos = new Vec2();
        ai.targetPos.set(target.getX(), target.getY());
        ai.setupLastPos();
    }

    static void boot() {
        Version.build = 158;
        Vars.headless = true;
        Vars.platform = new mindustry.core.Platform(){};
        Vars.net = new mindustry.net.Net(Vars.platform.getNet());
        Core.files = new SdlFiles();
        Core.settings = new arc.Settings();
        Vars.dataDirectory = Core.files.local("mindustry-parity-data/");
        Vars.customMapDirectory = Vars.dataDirectory.child("maps/");
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.logic = new Logic();
        Vars.waves = new Waves();
        Vars.mods = new Mods();
        Vars.maps = new mindustry.maps.Maps();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();
        Vars.state = new GameState();
        Vars.state.map = new mindustry.maps.Map(StringMap.of(
            "name", "parity-controller",
            "width", String.valueOf(WORLD),
            "height", String.valueOf(WORLD)
        ));
        Vars.state.rules.disableUnitCap = true;
        Groups.init();
        Vars.world = new World();
        resetTiles();
    }

    static void resetWorld(Fi saveFile) throws Exception {
        if (saveFile.exists()) saveFile.delete();
        Vars.logic.reset();
        Groups.init();
        Vars.state = new GameState();
        Vars.state.map = new mindustry.maps.Map(StringMap.of(
            "name", "parity-controller",
            "width", String.valueOf(WORLD),
            "height", String.valueOf(WORLD)
        ));
        Vars.state.rules.disableUnitCap = true;
        resetTiles();
    }

    static void resetTiles() {
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();
    }

    // --- JSON ----------------------------------------------------------------

    @SuppressWarnings("unchecked")
    static void appendOutcomes(StringBuilder json, Map<String, Object> scenarios) {
        Map<String, Object> cmd = (Map<String, Object>) scenarios.get("command_roundtrip");
        Map<String, Object> cmdAfter = (Map<String, Object>) ((Map<String, Object>) cmd.get("after"));
        Map<String, Object> logic = (Map<String, Object>) scenarios.get("logic_roundtrip");
        Map<String, Object> logicAfter = (Map<String, Object>) ((Map<String, Object>) logic.get("after"));

        Map<String, Object> outcomes = new LinkedHashMap<>();
        outcomes.put("command_ai_survives", cmdAfter.get("is_command_ai"));
        outcomes.put("command_persisted_id", cmdAfter.get("command_id"));
        outcomes.put("command_target_pos_persisted", cmdAfter.get("has_target_pos"));
        outcomes.put("command_attack_unit_persisted", cmdAfter.get("has_attack_target"));
        outcomes.put("command_queue_building_persisted",
            ((List<?>) cmdAfter.get("queue")).stream().anyMatch(e -> "building".equals(((Map<?, ?>) e).get("kind"))));
        outcomes.put("command_queue_vec2_persisted",
            ((List<?>) cmdAfter.get("queue")).stream().anyMatch(e -> "vec2".equals(((Map<?, ?>) e).get("kind"))));
        outcomes.put("command_queue_unit_persisted",
            ((List<?>) cmdAfter.get("queue")).stream().anyMatch(e -> "unit".equals(((Map<?, ?>) e).get("kind"))));
        outcomes.put("command_stances_persisted", ((List<?>) cmdAfter.get("stances")).size());
        outcomes.put("logic_ai_survives", logicAfter.get("is_logic_ai"));
        outcomes.put("logic_controller_pos_persisted", logicAfter.get("controller_pos"));
        outcomes.put("logic_control_timer_persisted_non_default",
            Math.abs(((Number) logicAfter.get("control_timer")).floatValue() - NON_DEFAULT_TIMER) < 0.01f);
        outcomes.put("logic_move_mode_persisted", !"idle".equals(logicAfter.get("control_mode")));
        outcomes.put("logic_move_coords_persisted",
            ((Number) logicAfter.get("move_x")).floatValue() != 0f
                || ((Number) logicAfter.get("move_y")).floatValue() != 0f);

        appendScenarioMap(json, "outcomes", outcomes, true);
    }

    static void appendScenarioMap(StringBuilder json, String key, Map<String, Object> map, boolean last) {
        json.append("  \"").append(key).append("\": {\n");
        int i = 0;
        for (var entry : map.entrySet()) {
            json.append("    \"").append(entry.getKey()).append("\": ");
            appendValue(json, entry.getValue(), "    ");
            json.append(i++ == map.size() - 1 ? "\n" : ",\n");
        }
        json.append("  }").append(last ? "" : ",").append("\n");
    }

    @SuppressWarnings("unchecked")
    static void appendValue(StringBuilder json, Object value, String indent) {
        if (value == null) {
            json.append("null");
        } else if (value instanceof Boolean b) {
            json.append(b);
        } else if (value instanceof Integer i) {
            json.append(i);
        } else if (value instanceof Long l) {
            json.append(l);
        } else if (value instanceof Float f) {
            json.append(num(f));
        } else if (value instanceof Double d) {
            json.append(num(d.floatValue()));
        } else if (value instanceof Number n) {
            json.append(n.intValue());
        } else if (value instanceof String s) {
            json.append(jsonString(s));
        } else if (value instanceof List<?> list) {
            json.append("[");
            for (int i = 0; i < list.size(); i++) {
                if (i > 0) json.append(", ");
                appendValue(json, list.get(i), indent);
            }
            json.append("]");
        } else if (value instanceof Map<?, ?> map) {
            json.append("{\n");
            int i = 0;
            for (var entry : ((Map<String, Object>) map).entrySet()) {
                json.append(indent).append("  \"").append(entry.getKey()).append("\": ");
                appendValue(json, entry.getValue(), indent + "  ");
                json.append(i++ == map.size() - 1 ? "\n" : ",\n");
            }
            json.append(indent).append("}");
        } else {
            json.append(jsonString(String.valueOf(value)));
        }
    }

    static String num(float value) {
        if (Float.isInfinite(value)) return value > 0 ? "1e38" : "-1e38";
        if (Float.isNaN(value)) return "0.000000";
        return String.format(Locale.US, "%.6f", value);
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParControllerSave158.class.getResourceAsStream("/version.properties")) {
            if (in == null) return "missing";
            Properties p = new Properties();
            p.load(in);
            return p.getProperty("type", "missing") + " " + p.getProperty("build", "missing");
        }
    }

    static String jsonString(String value) {
        StringBuilder out = new StringBuilder("\"");
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            switch (c) {
                case '"' -> out.append("\\\"");
                case '\\' -> out.append("\\\\");
                case '\n' -> out.append("\\n");
                case '\r' -> out.append("\\r");
                case '\t' -> out.append("\\t");
                default -> {
                    if (c < 0x20) {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
                }
            }
        }
        return out.append("\"").toString();
    }
}
