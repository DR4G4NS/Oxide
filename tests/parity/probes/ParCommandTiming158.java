import java.io.InputStream;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.geom.Vec2;
import arc.util.Time;
import mindustry.Vars;
import mindustry.ai.types.CommandAI;
import mindustry.ai.types.LogicAI;
import mindustry.content.Blocks;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.Building;
import mindustry.gen.Groups;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.logic.LAssembler;
import mindustry.logic.LExecutor;
import mindustry.world.Tile;
import mindustry.world.Tiles;
import mindustry.world.blocks.logic.LogicBlock;

/**
 * P1-A1 differential probe: CommandAI transient {@code attackTarget != null &&
 * targetPos == null} and whether Logic can observe it within a game tick.
 *
 * Official 158.1 ordering inside {@code Logic.updateEntities}:
 * {@code Groups.unit.update()} (CommandAI.updateUnit) then
 * {@code Groups.build.update()} (LogicBlock.updateTile / LExecutor.runOnce).
 *
 * Each scenario traces T0–T4:
 *   T0 — before the command
 *   T1 — immediately after {@code commandTarget}/{@code commandPosition}
 *   T2 — after one {@code CommandAI.updateUnit()}
 *   T3 — after one {@code LExecutor.runOnce()} with {@code ucontrol move}
 *        (LogicBlock phase, unit update already ran this tick)
 *   T4 — after the next {@code CommandAI.updateUnit()} (following tick)
 *
 * Scenarios: building target, unit target, Vec2 position, invalid building
 * target (destroyed before the unit update), and a fresh baseline.
 *
 * Version-gated to official 158.1.
 */
public final class ParCommandTiming158 {
    static final int WORLD = 16;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParCommandTiming158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

        Vars.headless = true;
        Vars.platform = new mindustry.core.Platform(){};
        Vars.net = new mindustry.net.Net(Vars.platform.getNet());
        Vars.indexer = new mindustry.ai.BlockIndexer();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Groups.init();
        Core.files = new SdlFiles();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();
        Vars.state = new GameState();
        Vars.state.rules.disableUnitCap = true;
        Vars.state.rules.logicUnitControl = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();

        Tile procTile = Vars.world.tiles.get(7, 7);
        Tile wallTile = Vars.world.tiles.get(8, 8);
        Building proc = placeProcessor(procTile);

        Unit foe = UnitTypes.dagger.create(Team.crux);
        foe.set(120f, 120f);
        foe.add();

        Unit unit = UnitTypes.flare.create(Team.sharded);
        unit.set(80f, 80f);
        unit.add();
        Vars.state.teams.updateTeamStats();

        if (!(unit.controller() instanceof CommandAI)) {
            System.err.println("ParCommandTiming158: flare did not receive CommandAI");
            System.exit(3);
        }
        CommandAI ai = (CommandAI) unit.controller();

        // --- building target (commandTarget) --------------------------------
        wallTile.setBlock(Blocks.copperWall, Team.crux, 0);
        Building wall = wallTile.build;
        Trace building = runScenario(unit, proc, t -> t.commandTarget(wall));

        // --- unit target (commandTarget) ------------------------------------
        Trace unitTarget = runScenario(unit, proc, t -> t.commandTarget(foe));

        // --- Vec2 position (commandPosition — no transient) ----------------
        Trace vec2 = runScenario(unit, proc, t -> t.commandPosition(new Vec2(100f, 100f)));

        // --- invalid building (destroyed before unit update) ----------------
        wallTile.setBlock(Blocks.copperWall, Team.crux, 0);
        Building wall2 = wallTile.build;
        Trace invalid = runInvalidScenario(unit, proc, wall2, wallTile);

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("command-timing")).append(",\n");
        json.append("  \"tick\": 4,\n");
        json.append("  \"unit_team\": ").append(Team.sharded.id).append(",\n");
        appendTrace(json, "building", building);
        appendTrace(json, "unit", unitTarget);
        appendTrace(json, "vec2", vec2);
        appendTrace(json, "invalid", invalid, true);
        json.append("}\n");
        System.out.print(json);
    }

    /** Runs command → unit update → ucontrol → next unit update. */
    static Trace runScenario(Unit unit, Building proc, CommandIssued issue) {
        unit.resetController();
        CommandAI ai = (CommandAI) unit.controller();
        LExecutor exec = executorFor(proc, "ucontrol move 50 50\nstop");
        exec.unit.setconst(unit);

        Trace t = new Trace();
        t.t0 = snapshot(unit);

        issue.apply(ai);
        t.t1 = snapshot(unit);

        tickUnit(unit);
        t.t2 = snapshot(unit);

        exec.runOnce();
        t.t3 = snapshot(unit);
        t.t3.ucontrolBecameLogic = unit.controller() instanceof LogicAI;

        tickUnit(unit);
        t.t4 = snapshot(unit);
        return t;
    }

    /** commandTarget(building) then destroy the tile before the unit update. */
    static Trace runInvalidScenario(Unit unit, Building proc, Building wall, Tile wallTile) {
        unit.resetController();
        CommandAI ai = (CommandAI) unit.controller();
        LExecutor exec = executorFor(proc, "ucontrol move 50 50\nstop");
        exec.unit.setconst(unit);

        Trace t = new Trace();
        t.t0 = snapshot(unit);

        ai.commandTarget(wall);
        wallTile.setBlock(Blocks.air, Team.derelict, 0);
        t.t1 = snapshot(unit);

        tickUnit(unit);
        t.t2 = snapshot(unit);

        exec.runOnce();
        t.t3 = snapshot(unit);
        t.t3.ucontrolBecameLogic = unit.controller() instanceof LogicAI;

        tickUnit(unit);
        t.t4 = snapshot(unit);
        return t;
    }

    static Snapshot snapshot(Unit unit) {
        if (unit.controller() instanceof CommandAI ai) {
            Snapshot s = new Snapshot();
            s.hasCommand = ai.hasCommand();
            s.attackTarget = ai.attackTarget != null;
            s.targetPos = ai.targetPos != null;
            s.logicControllable = ai.isLogicControllable();
            return s;
        }
        Snapshot s = new Snapshot();
        s.hasCommand = false;
        s.attackTarget = false;
        s.targetPos = false;
        s.logicControllable = unit.controller().isLogicControllable();
        return s;
    }

    static void tickUnit(Unit unit) {
        Time.delta = 1f;
        Time.time += Time.delta;
        if (unit.controller() instanceof LogicAI la) {
            la.updateMovement();
        } else if (unit.controller() instanceof CommandAI ai) {
            ai.updateUnit();
        }
    }

    static Building placeProcessor(Tile tile) {
        tile.setBlock(Blocks.microProcessor, Team.sharded, 0);
        Building build = tile.build;
        if (!(build instanceof LogicBlock.LogicBuild)) {
            System.err.println("ParCommandTiming158: micro processor did not build a LogicBuild");
            System.exit(4);
        }
        return build;
    }

    static LExecutor executorFor(Building proc, String program) {
        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(program, false));
        if (!exec.initialized()) {
            System.err.println("ParCommandTiming158: executor did not initialize");
            System.exit(5);
        }
        exec.team = Team.sharded;
        exec.thisv.setconst(proc);
        return exec;
    }

    static void appendTrace(StringBuilder json, String name, Trace trace) {
        appendTrace(json, name, trace, false);
    }

    static void appendTrace(StringBuilder json, String name, Trace trace, boolean last) {
        json.append("  ").append(jsonString(name)).append(": {\n");
        appendPhase(json, "t0", trace.t0);
        appendPhase(json, "t1", trace.t1);
        appendPhase(json, "t2", trace.t2);
        appendPhase(json, "t3", trace.t3, true);
        appendPhase(json, "t4", trace.t4, false, true);
        json.append("  }").append(last ? "\n" : ",\n");
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s) {
        appendPhase(json, phase, s, false);
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s, boolean includeUcontrol) {
        appendPhase(json, phase, s, includeUcontrol, false);
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s, boolean includeUcontrol, boolean lastPhase) {
        json.append("    ").append(jsonString(phase)).append(": {");
        json.append("\"has_command\": ").append(s.hasCommand).append(", ");
        json.append("\"attack_target\": ").append(s.attackTarget).append(", ");
        json.append("\"target_pos\": ").append(s.targetPos).append(", ");
        json.append("\"logic_controllable\": ").append(s.logicControllable);
        if (includeUcontrol) {
            json.append(", \"ucontrol_became_logic\": ").append(s.ucontrolBecameLogic);
        }
        json.append("}").append(lastPhase ? "\n" : ",\n");
    }

    @FunctionalInterface
    interface CommandIssued {
        void apply(CommandAI ai);
    }

    static final class Trace {
        Snapshot t0, t1, t2, t3, t4;
    }

    static final class Snapshot {
        boolean hasCommand;
        boolean attackTarget;
        boolean targetPos;
        boolean logicControllable;
        boolean ucontrolBecameLogic;
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParCommandTiming158.class.getResourceAsStream("/version.properties")) {
            if (in == null) return "missing";
            Properties p = new Properties();
            p.load(in);
            String build = p.getProperty("build", "missing");
            String type = p.getProperty("type", "missing");
            return type + " " + build;
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
