import java.io.InputStream;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.Mathf;
import arc.util.Time;
import mindustry.Vars;
import mindustry.ai.types.LogicAI;
import mindustry.content.Blocks;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.entities.EntityCollisions;
import mindustry.game.Team;
import mindustry.gen.Building;
import mindustry.gen.Groups;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.logic.LAssembler;
import mindustry.logic.LExecutor;
import mindustry.logic.LUnitControl;
import mindustry.world.Tile;
import mindustry.world.Tiles;
import mindustry.world.blocks.logic.LogicBlock;

/**
 * P1-B1 differential probe: tick ordering for LogicAI {@code ucontrol move}.
 *
 * Official 158.1 ordering inside {@code Logic.updateEntities}:
 * {@code Groups.unit.update()} (VelComp then LogicAI.updateMovement) then
 * {@code Groups.build.update()} (LogicBlock / LExecutor.runOnce).
 *
 * Each scenario traces five phases around tick N when the processor issues
 * {@code ucontrol move} (or {@code ucontrol stop}):
 *   n_minus_1 — end of tick N-1 (after unit update)
 *   n_after_ucontrol — immediately after the processor instruction on tick N
 *   end_n — same as n_after_ucontrol (end of tick N)
 *   end_n_plus_1 — after unit update on tick N+1
 *   end_n_plus_2 — after unit update on tick N+2
 *
 * Observable fields: position, velocity, LogicAI control mode, move target.
 *
 * Scenarios: flying (flare), grounded (dagger), stop→move, move→stop,
 * processor destroyed after issuing move.
 *
 * Version-gated to official 158.1.
 */
public final class ParLogicMoveTiming158 {
    static final int WORLD = 16;
    static final float START_X = 80f;
    static final float START_Y = 80f;
    static final float TARGET_X = 200f;
    static final float TARGET_Y = 80f;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParLogicMoveTiming158: refusing to run: classpath version.properties reports '"
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
        Vars.collisions = new EntityCollisions();

        Tile procTile = Vars.world.tiles.get(7, 7);
        Building proc = placeProcessor(procTile);

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("logic-move-timing")).append(",\n");
        json.append("  \"start_x\": ").append(START_X).append(",\n");
        json.append("  \"start_y\": ").append(START_Y).append(",\n");
        json.append("  \"target_x\": ").append(TARGET_X).append(",\n");
        json.append("  \"target_y\": ").append(TARGET_Y).append(",\n");

        appendTrace(json, "flying", runFlying(proc));
        appendTrace(json, "grounded", runGrounded(proc));
        appendTrace(json, "stop_to_move", runStopToMove(proc));
        appendTrace(json, "move_to_stop", runMoveToStop(proc));
        appendTrace(json, "proc_destroyed", runProcDestroyed(procTile, proc), true);
        json.append("}\n");
        System.out.print(json);
    }

    /** Flare (flying): idle → ucontrol move on tick N. */
    static Trace runFlying(Building proc) {
        Unit unit = createUnit(UnitTypes.flare, START_X, START_Y);
        acquireLogic(proc, unit);
        setIdle(unit);
        return traceMoveOnTickN(proc, unit, "ucontrol move " + TARGET_X + " " + TARGET_Y);
    }

    /** Dagger (grounded): idle → ucontrol move on tick N. */
    static Trace runGrounded(Building proc) {
        Unit unit = createUnit(UnitTypes.dagger, START_X, START_Y);
        acquireLogic(proc, unit);
        setIdle(unit);
        return traceMoveOnTickN(proc, unit, "ucontrol move " + TARGET_X + " " + TARGET_Y);
    }

    /** stop → move: ucontrol stop preloaded, then ucontrol move on tick N. */
    static Trace runStopToMove(Building proc) {
        Unit unit = createUnit(UnitTypes.dagger, START_X, START_Y);
        acquireLogic(proc, unit);
        controlOnce(proc, unit, "ucontrol stop\nstop");
        unitTick(unit);
        return traceMoveOnTickN(proc, unit, "ucontrol move " + TARGET_X + " " + TARGET_Y);
    }

    /** move → stop: already moving, ucontrol stop on tick N. */
    static Trace runMoveToStop(Building proc) {
        Unit unit = createUnit(UnitTypes.dagger, START_X, START_Y);
        acquireLogic(proc, unit);
        controlOnce(proc, unit, "ucontrol move " + TARGET_X + " " + TARGET_Y + "\nstop");
        unitTick(unit);
        unitTick(unit);
        return traceMoveOnTickN(proc, unit, "ucontrol stop\nstop");
    }

    /** Processor destroyed immediately after ucontrol move on tick N. */
    static Trace runProcDestroyed(Tile procTile, Building proc) {
        Unit unit = createUnit(UnitTypes.dagger, START_X, START_Y);
        acquireLogic(proc, unit);
        setIdle(unit);

        Trace t = new Trace();
        unitTick(unit);
        t.nMinus1 = snapshot(unit);

        unitTick(unit);
        controlOnce(proc, unit, "ucontrol move " + TARGET_X + " " + TARGET_Y + "\nstop");
        procTile.setBlock(Blocks.air, Team.derelict, 0);
        t.nAfterUcontrol = snapshot(unit);
        t.endN = t.nAfterUcontrol;

        unitTick(unit);
        t.endNPlus1 = snapshot(unit);

        unitTick(unit);
        t.endNPlus2 = snapshot(unit);
        return t;
    }

    /** Standard trace: tick N-1 idle, tick N issues {@code command}. */
    static Trace traceMoveOnTickN(Building proc, Unit unit, String command) {
        Trace t = new Trace();
        unitTick(unit);
        t.nMinus1 = snapshot(unit);

        unitTick(unit);
        controlOnce(proc, unit, command + "\nstop");
        t.nAfterUcontrol = snapshot(unit);
        t.endN = t.nAfterUcontrol;

        unitTick(unit);
        t.endNPlus1 = snapshot(unit);

        unitTick(unit);
        t.endNPlus2 = snapshot(unit);
        return t;
    }

    static Unit createUnit(mindustry.type.UnitType type, float x, float y) {
        Unit unit = type.create(Team.sharded);
        unit.set(x, y);
        unit.add();
        Vars.state.teams.updateTeamStats();
        return unit;
    }

    static void acquireLogic(Building proc, Unit unit) {
        unit.resetController();
        controlOnce(proc, unit, "ucontrol flag 0\nstop");
        if (!(unit.controller() instanceof LogicAI)) {
            System.err.println("ParLogicMoveTiming158: failed to acquire LogicAI");
            System.exit(3);
        }
    }

    static void setIdle(Unit unit) {
        if (unit.controller() instanceof LogicAI la) {
            la.control = LUnitControl.stop;
        }
    }

    static void controlOnce(Building proc, Unit unit, String program) {
        LExecutor exec = executorFor(proc, program);
        exec.unit.setconst(unit);
        exec.runOnce();
    }

    /**
     * One unit-update pass in official order: VelComp integration (priority -1)
     * then controller.updateUnit() → LogicAI.updateMovement().
     */
    static void unitTick(Unit unit) {
        Time.delta = 1f;
        Time.time += Time.delta;
        integrateVelocity(unit);
        if (unit.controller() instanceof LogicAI la) {
            la.updateMovement();
        }
    }

    /** Mirrors VelComp.update() (VelComp.java:21-33). */
    static void integrateVelocity(Unit unit) {
        float px = unit.x(), py = unit.y();
        unit.move(unit.vel.x * Time.delta, unit.vel.y * Time.delta);
        if (Mathf.equal(px, unit.x())) unit.vel.x = 0f;
        if (Mathf.equal(py, unit.y())) unit.vel.y = 0f;
        unit.vel.scl(Math.max(1f - unit.drag() * Time.delta, 0f));
    }

    static Snapshot snapshot(Unit unit) {
        Snapshot s = new Snapshot();
        s.x = unit.x();
        s.y = unit.y();
        s.velX = unit.vel.x;
        s.velY = unit.vel.y;
        s.isLogic = unit.controller() instanceof LogicAI;
        if (unit.controller() instanceof LogicAI la) {
            s.control = la.control.name();
            s.moveX = la.moveX;
            s.moveY = la.moveY;
            s.processorValid = la.controller != null && la.controller.isValid();
        } else {
            s.control = "none";
            s.moveX = Float.NaN;
            s.moveY = Float.NaN;
            s.processorValid = false;
        }
        return s;
    }

    static Building placeProcessor(Tile tile) {
        tile.setBlock(Blocks.microProcessor, Team.sharded, 0);
        Building build = tile.build;
        if (!(build instanceof LogicBlock.LogicBuild)) {
            System.err.println("ParLogicMoveTiming158: micro processor did not build a LogicBuild");
            System.exit(4);
        }
        return build;
    }

    static LExecutor executorFor(Building proc, String program) {
        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(program, false));
        if (!exec.initialized()) {
            System.err.println("ParLogicMoveTiming158: executor did not initialize");
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
        appendPhase(json, "n_minus_1", trace.nMinus1);
        appendPhase(json, "n_after_ucontrol", trace.nAfterUcontrol);
        appendPhase(json, "end_n", trace.endN);
        appendPhase(json, "end_n_plus_1", trace.endNPlus1);
        appendPhase(json, "end_n_plus_2", trace.endNPlus2, true);
        json.append("  }").append(last ? "\n" : ",\n");
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s) {
        appendPhase(json, phase, s, false);
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s, boolean lastPhase) {
        json.append("    ").append(jsonString(phase)).append(": {");
        json.append("\"x\": ").append(fmt(s.x)).append(", ");
        json.append("\"y\": ").append(fmt(s.y)).append(", ");
        json.append("\"vel_x\": ").append(fmt(s.velX)).append(", ");
        json.append("\"vel_y\": ").append(fmt(s.velY)).append(", ");
        json.append("\"control\": ").append(jsonString(s.control)).append(", ");
        json.append("\"move_x\": ").append(fmt(s.moveX)).append(", ");
        json.append("\"move_y\": ").append(fmt(s.moveY)).append(", ");
        json.append("\"is_logic\": ").append(s.isLogic).append(", ");
        json.append("\"processor_valid\": ").append(s.processorValid);
        json.append("}").append(lastPhase ? "\n" : ",\n");
    }

    static String fmt(float v) {
        if (Float.isNaN(v)) return "null";
        return String.format(java.util.Locale.ROOT, "%.4f", v);
    }

    static final class Trace {
        Snapshot nMinus1, nAfterUcontrol, endN, endNPlus1, endNPlus2;
    }

    static final class Snapshot {
        float x, y, velX, velY, moveX, moveY;
        String control;
        boolean isLogic, processorValid;
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParLogicMoveTiming158.class.getResourceAsStream("/version.properties")) {
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
