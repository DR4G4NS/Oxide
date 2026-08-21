import java.io.InputStream;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.util.Time;
import mindustry.Vars;
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
 * P0-03 differential probe: LogicAI acquisition, 600-tick lease and
 * resetController lifecycle on desktop.jar 158.1.
 *
 * Minimal headless world (no game loop, no client) following ParUbind158:
 * one REAL micro-processor building placed at (8,8) team sharded — the
 * LogicAI controller must reference a valid Building, because
 * `LogicAI.updateMovement` releases the unit unless
 * `controller != null && controller.isValid()` — plus one sharded dagger in
 * the team unit cache. An LExecutor with `exec.team = Team.sharded` and
 * `exec.thisv = the processor build` (exactly what LogicBlock's code loader
 * does, LogicBlock.java:423) runs one instruction per tick:
 *
 *   ubind @dagger          idx 0   bind the dagger
 *   ucontrol flag 7        idx 1   first takeover: installs LogicAI and
 *                                  sets controlTimer = 60f*10f
 *   stop                   idx 2   park; no further refresh is issued
 *
 * `ucontrol flag` is used because it changes no movement state: the timer
 * block at the top of `LogicAI.updateMovement` (LogicAI.java:59-64) is the
 * only thing left running. Per game tick the probe drives, in Java's entity
 * order (units update before buildings — Groups unit precedes build):
 *
 *   1. Time.delta = 1f  (the official server's delta is
 *      getDeltaTime()*60f == 1.0 per tick at 60 TPS, ServerControl.java:197);
 *   2. if the unit is logic-controlled: ai.updateMovement() — the official
 *      decrement / reset / processor-validity code;
 *   3. exec.runOnce() — the processor's one instruction for this tick.
 *
 * Scenario A observes the lease boundary: which tick still holds LogicAI,
 * the exact controlTimer values around it and the tick the controller resets
 * (pre-decrement `controlTimer > 0` means a 600.0 timer reaches exactly 0.0
 * after 600 decrements and resets on the next update). Scenario B re-runs
 * the same program on the SAME still-valid processor (the unit reverted to
 * its default CommandAI after A's expiry, so the takeover is legal again),
 * destroys the processor (tile -> air) at tick 50 and records the reset
 * tick. The unit's lifecycle call is the official `unit.add()`: GroupTrait
 * registration sets the `added` flag that `unit.isValid()`
 * (`!dead && isAdded()`, HealthComp) — and thus checkLogicAI — requires.
 *
 * The Rust side replays the identical cadence with its authoritative lease
 * pass (`simulate_logic_control_leases`, unit-authority driven).
 *
 * Version gate: refuses to run unless the classpath version.properties
 * reports the official 158.1 build.
 */
public final class ParLease158 {
    /** Total driven ticks per scenario (well past the 600-tick lease). */
    static final int TICKS = 700;
    /** Tick at which scenario B destroys the processor tile. */
    static final int DESTROY_AT = 50;
    static final int WORLD = 16;

    /** The exact logic source replayed by the Rust side. */
    static final String PROGRAM =
        "ubind @dagger\n"
        + "ucontrol flag 7\n"
        + "stop";

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParLease158: refusing to run: classpath version.properties reports '" + version
                + "', expected official 158.1");
            System.exit(2);
        }

        // Shared headless setup: one world, one processor, one dagger.
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Groups.init();
        Core.files = new SdlFiles();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();
        Vars.state = new GameState();
        Vars.state.rules.disableUnitCap = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();
        Tile procTile = Vars.world.tiles.get(8, 8);
        procTile.setBlock(Blocks.microProcessor, Team.sharded, 0);
        Building proc = procTile.build;
        if (!(proc instanceof LogicBlock.LogicBuild)) {
            System.err.println("ParLease158: micro processor did not build a LogicBuild");
            System.exit(3);
        }
        Unit unit = UnitTypes.dagger.create(Team.sharded);
        unit.set(80f, 80f);
        unit.add();
        Vars.state.teams.updateTeamStats();

        // --- Scenario A: no refresh after the single ucontrol -------------
        Object[] a = runScenario(procTile, proc, unit, false);

        // --- Scenario B: same processor re-acquires, destroyed at tick 50 --
        Object[] b = runScenario(procTile, proc, unit, true);

        int acquireTickA = (Integer) a[0];
        float timerAtAcquire = (Float) a[1];
        float timer599 = (Float) a[2];
        float timer600 = (Float) a[3];
        float timer601 = (Float) a[4];
        float timer602 = (Float) a[5];
        boolean buildAt599 = (Boolean) a[6];
        int releaseTickA = (Integer) a[7];
        int stillLogicAtDestroy = (Integer) b[8];
        int acquireTickB = (Integer) b[0];
        int releaseTickB = (Integer) b[7];

        // Any NaN/-1 would poison the JSON fixture: both acquisitions and
        // both releases must have been observed.
        if (acquireTickA < 0 || releaseTickA < 0 || acquireTickB < 0 || releaseTickB < 0
            || stillLogicAtDestroy < 0 || Float.isNaN(timerAtAcquire) || Float.isNaN(timer599)
            || Float.isNaN(timer600) || Float.isNaN(timer601) || Float.isNaN(timer602)) {
            System.err.println("ParLease158: scenario incomplete: acquireA=" + acquireTickA
                + " releaseA=" + releaseTickA + " acquireB=" + acquireTickB
                + " releaseB=" + releaseTickB + " destroySeen=" + stillLogicAtDestroy);
            System.exit(5);
        }

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("lease-600")).append(",\n");
        json.append("  \"tick\": ").append(TICKS).append(",\n");
        json.append("  \"program\": ").append(jsonString(PROGRAM)).append(",\n");
        json.append("  \"processor_team\": ").append(Team.sharded.id).append(",\n");
        json.append("  \"unit_team\": ").append(Team.sharded.id).append(",\n");
        json.append("  \"acquire_tick\": ").append(acquireTickA).append(",\n");
        json.append("  \"timer_at_acquire\": ").append(timerAtAcquire).append(",\n");
        json.append("  \"timer_at_599\": ").append(timer599).append(",\n");
        json.append("  \"timer_at_600\": ").append(timer600).append(",\n");
        json.append("  \"timer_at_601\": ").append(timer601).append(",\n");
        json.append("  \"timer_at_602\": ").append(timer602).append(",\n");
        json.append("  \"controller_is_build_at_599\": ").append(buildAt599).append(",\n");
        json.append("  \"release_tick\": ").append(releaseTickA).append(",\n");
        json.append("  \"destroy_at\": ").append(DESTROY_AT).append(",\n");
        json.append("  \"reacquire_tick\": ").append(acquireTickB).append(",\n");
        json.append("  \"still_logic_at_destroy\": ").append(stillLogicAtDestroy >= 0).append(",\n");
        json.append("  \"destroy_release_tick\": ").append(releaseTickB).append("\n");
        json.append("}\n");
        System.out.print(json);
    }

    /**
     * Runs the three-instruction program for TICKS ticks against the given
     * processor/unit. Returns {acquireTick, timerAtAcquire, timer599,
     * timer600, timer601, timer602, controllerIsBuildAt599, releaseTick,
     * stillLogicAtDestroyTick}.
     */
    static Object[] runScenario(Tile procTile, Building proc, Unit unit, boolean destroyProcessor) {
        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(PROGRAM, false));
        if (!exec.initialized()) {
            System.err.println("ParLease158: executor did not initialize");
            System.exit(4);
        }
        // LogicBuild.updateTile assigns executor.team every tick; the code
        // loader binds @this to the building (LogicBlock.java:423).
        exec.team = Team.sharded;
        exec.thisv.setconst(proc);

        int acquireTick = -1;
        float timerAtAcquire = Float.NaN;
        float timer599 = Float.NaN, timer600 = Float.NaN, timer601 = Float.NaN, timer602 = Float.NaN;
        boolean buildAt599 = false;
        int releaseTick = -1;
        int stillLogicAtDestroy = -1;

        for (int tick = 1; tick <= TICKS; tick++) {
            Time.delta = 1f;
            Time.time += Time.delta;

            // Units update before buildings: the LogicAI lease clock first.
            if (unit.controller() instanceof LogicAI la) {
                if (tick == 599) buildAt599 = la.controller == proc;
                la.updateMovement();
            }

            // Then the processor's single instruction.
            exec.runOnce();

            if (acquireTick < 0 && unit.controller() instanceof LogicAI la) {
                acquireTick = tick;
                timerAtAcquire = la.controlTimer;
            }
            if (tick == 599 && unit.controller() instanceof LogicAI la) timer599 = la.controlTimer;
            if (tick == 600 && unit.controller() instanceof LogicAI la) timer600 = la.controlTimer;
            if (tick == 601 && unit.controller() instanceof LogicAI la) timer601 = la.controlTimer;
            if (tick == 602 && unit.controller() instanceof LogicAI la) timer602 = la.controlTimer;
            if (releaseTick < 0 && acquireTick >= 0 && !(unit.controller() instanceof LogicAI)) {
                releaseTick = tick;
            }

            if (destroyProcessor && tick == DESTROY_AT) {
                stillLogicAtDestroy = unit.controller() instanceof LogicAI ? tick : -1;
                // Official destruction state: the tile no longer holds this
                // building, so controller.isValid() is false.
                procTile.setBlock(Blocks.air, Team.derelict, 0);
            }
        }

        return new Object[]{
            acquireTick, timerAtAcquire, timer599, timer600, timer601, timer602,
            buildAt599, releaseTick, stillLogicAtDestroy
        };
    }

    /** Reads version.properties from the classpath (the desktop.jar root). */
    static String classpathVersion() throws Exception {
        try (InputStream in = ParLease158.class.getResourceAsStream("/version.properties")) {
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
