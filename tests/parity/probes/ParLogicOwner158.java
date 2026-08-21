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
 * P0-C1 differential probe: LogicAI lease identity is the Building object,
 * not the tile position. desktop.jar 158.1 {@code LogicAI.updateMovement}
 * releases unless {@code controller != null && controller.isValid()}, and
 * {@code Building.isValid()} is {@code tile.build == this && !dead}.
 *
 * A processor destroyed and replaced on the SAME tile by another processor
 * (same block id) is a different Building, so the old lease must drop.
 * A later {@code ucontrol} from the new processor acquires a fresh lease.
 *
 * P0-C3 extends the same probe with the exact temporal boundary of the lease
 * and the {@code ucontrol unbind} contract, all measured in LEASE AGE — the
 * number of {@code updateMovement} passes since the acquiring instruction —
 * so the observation is independent of which tick the takeover happened on:
 *
 *   L lease    — no refresh: age 599 leaves {@code controlTimer} at 1,
 *                age 600 at 0 and STILL controlled (the guard checks
 *                {@code controlTimer > 0} BEFORE decrementing), age 601
 *                resets the controller.
 *   R refresh  — a valid {@code ucontrol} at age 599 writes the timer back
 *                to the full 600 (LExecutor.java:351), restarting the same
 *                599/600/601 boundary.
 *   G gate     — the identical instruction whose {@code checkLogicAI} fails
 *                (enemy team) never reaches that assignment, so the original
 *                boundary still expires on schedule and nothing is written.
 *   U unbind   — {@code case unbind -> unit.resetController()} drops the
 *                LogicAI but never touches {@code exec.unit}: @unit still
 *                holds the unit, the fresh controller reports no command and
 *                the next {@code ucontrol} legally re-acquires.
 *   T rts      — {@code unbind} on a unit with an ACTIVE CommandAI command
 *                fails {@code isLogicControllable()} and is a complete
 *                no-op: same controller object, command intact.
 *
 * Cadence per tick matches ParLease158: Time.delta = 1, unit updateMovement
 * (lease clock) then exec.runOnce (one instruction). Version-gated to
 * official 158.1.
 */
public final class ParLogicOwner158 {
    static final int WORLD = 16;
    static final String PROGRAM =
        "ubind @dagger\n"
        + "ucontrol flag 7\n"
        + "stop";

    /** Single-instruction programs driven on an already-bound @unit. */
    static final String CONTROL = "ucontrol flag 7\nstop";
    static final String MOVE = "ucontrol move 30 30\nstop";
    static final String UNBIND = "ucontrol unbind\nstop";
    /** Lease age (updateMovement passes since acquisition) probed exactly. */
    static final int AGE_BEFORE_EXPIRY = 599;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParLogicOwner158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

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

        Tile tile = Vars.world.tiles.get(8, 8);
        Unit unit = UnitTypes.dagger.create(Team.sharded);
        unit.set(80f, 80f);
        unit.add();
        Vars.state.teams.updateTeamStats();

        // --- A lives: first ucontrol installs LogicAI on processor A --------
        Building procA = placeProcessor(tile);
        LExecutor exec = executorFor(procA);
        Phase a = driveUntilAcquired(exec, unit, procA);

        // --- Destroy A, place processor B on the same tile, then the lease
        //     clock. A.isValid() is false because tile.build is now B. ------
        tile.setBlock(Blocks.air, Team.derelict, 0);
        Building procB = placeProcessor(tile);
        boolean stillLogicBeforeBTick = unit.controller() instanceof LogicAI;
        boolean aValidAfterBPlaced = procA.isValid();
        boolean bIsA = procB == procA;
        Time.delta = 1f;
        Time.time += Time.delta;
        if (unit.controller() instanceof LogicAI la) {
            la.updateMovement();
        }
        boolean stillLogicAfterBTick = unit.controller() instanceof LogicAI;

        // --- B's ucontrol acquires a fresh lease pointing at B --------------
        exec = executorFor(procB);
        Phase b = driveUntilAcquired(exec, unit, procB);
        boolean bControllerIsB = b.controller == procB;
        boolean bControllerIsA = b.controller == procA;

        // --- B replaced by C (same block id, same tile) → old lease drops ---
        tile.setBlock(Blocks.air, Team.derelict, 0);
        Building procC = placeProcessor(tile);
        Time.delta = 1f;
        Time.time += Time.delta;
        if (unit.controller() instanceof LogicAI la) {
            la.updateMovement();
        }
        boolean stillLogicAfterCTick = unit.controller() instanceof LogicAI;
        boolean cIsB = procC == procB;

        // --- Wall replacement of a fresh A' also drops the lease ------------
        unit.resetController();
        Building procWallOwner = placeProcessor(tile);
        exec = executorFor(procWallOwner);
        Phase wallAcquire = driveUntilAcquired(exec, unit, procWallOwner);
        tile.setBlock(Blocks.copperWall, Team.sharded, 0);
        Time.delta = 1f;
        Time.time += Time.delta;
        if (unit.controller() instanceof LogicAI la) {
            la.updateMovement();
        }
        boolean stillLogicAfterWall = unit.controller() instanceof LogicAI;

        // === P0-C3: exact lease boundary, refresh points and unbind ========
        Building proc = placeProcessor(tile);

        // --- L: no refresh at all ------------------------------------------
        acquireLease(proc, unit);
        float leaseTimerAtAcquire = timerOf(unit);
        for (int i = 0; i < AGE_BEFORE_EXPIRY; i++) leaseTick(unit);
        float leaseTimer599 = timerOf(unit);
        boolean leaseControlled599 = isLogic(unit);
        leaseTick(unit);
        float leaseTimer600 = timerOf(unit);
        boolean leaseControlled600 = isLogic(unit);
        leaseTick(unit);
        boolean leaseControlled601 = isLogic(unit);

        // --- R: a valid ucontrol at age 599 restarts the boundary ----------
        acquireLease(proc, unit);
        for (int i = 0; i < AGE_BEFORE_EXPIRY; i++) leaseTick(unit);
        float refreshTimerBefore = timerOf(unit);
        controlOnce(proc, unit, CONTROL);
        float refreshTimerAfter = timerOf(unit);
        for (int i = 0; i < 600; i++) leaseTick(unit);
        boolean refreshControlled600 = isLogic(unit);
        leaseTick(unit);
        boolean refreshControlled601 = isLogic(unit);

        // --- G: the same instruction with a failing checkLogicAI -----------
        acquireLease(proc, unit);
        for (int i = 0; i < AGE_BEFORE_EXPIRY; i++) leaseTick(unit);
        unit.flag = 0;
        unit.team(Team.crux);
        controlOnce(proc, unit, CONTROL);
        float gateTimerAfter = timerOf(unit);
        boolean gateFlagWritten = unit.flag != 0;
        leaseTick(unit);
        boolean gateControlled600 = isLogic(unit);
        leaseTick(unit);
        boolean gateControlled601 = isLogic(unit);
        unit.team(Team.sharded);

        // --- U: unbind resets the controller and preserves @unit -----------
        unit.resetController();
        controlOnce(proc, unit, MOVE);
        boolean unbindLogicBefore = isLogic(unit);
        LExecutor unbindExec = executorFor(proc, UNBIND);
        unbindExec.unit.setconst(unit);
        unbindExec.runOnce();
        boolean unbindLogicAfter = isLogic(unit);
        boolean unbindUnitVarKept = unbindExec.unit.obj() == unit;
        boolean unbindHasCommand = unit.controller() instanceof CommandAI cai && cai.hasCommand();
        boolean unbindLogicControllable = unit.controller().isLogicControllable();
        controlOnce(proc, unit, CONTROL);
        boolean unbindReacquired = isLogic(unit);
        float unbindReacquireTimer = timerOf(unit);

        // --- T: unbind of an actively RTS-commanded unit is a no-op --------
        CommandAI rts = new CommandAI();
        unit.controller(rts);
        rts.commandPosition(new Vec2(200f, 200f));
        boolean rtsHasCommandBefore = rts.hasCommand();
        LExecutor rtsExec = executorFor(proc, UNBIND);
        rtsExec.unit.setconst(unit);
        rtsExec.runOnce();
        boolean rtsSameController = unit.controller() == rts;
        boolean rtsBecameLogic = isLogic(unit);
        boolean rtsHasCommandAfter = rts.hasCommand();
        boolean rtsUnitVarKept = rtsExec.unit.obj() == unit;

        if (Float.isNaN(leaseTimerAtAcquire) || Float.isNaN(leaseTimer599)
            || Float.isNaN(leaseTimer600) || Float.isNaN(refreshTimerBefore)
            || Float.isNaN(refreshTimerAfter) || Float.isNaN(gateTimerAfter)
            || Float.isNaN(unbindReacquireTimer) || !unbindLogicBefore || !rtsHasCommandBefore) {
            System.err.println("ParLogicOwner158: boundary scenario incomplete:"
                + " acquire=" + leaseTimerAtAcquire + " t599=" + leaseTimer599
                + " t600=" + leaseTimer600 + " refresh=" + refreshTimerAfter
                + " gate=" + gateTimerAfter + " unbindPre=" + unbindLogicBefore
                + " rtsPre=" + rtsHasCommandBefore);
            System.exit(6);
        }

        if (!a.acquired || !b.acquired || !wallAcquire.acquired
            || a.controller == null || b.controller == null) {
            System.err.println("ParLogicOwner158: scenario incomplete: acquireA=" + a.acquired
                + " acquireB=" + b.acquired + " acquireWall=" + wallAcquire.acquired);
            System.exit(5);
        }

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("logic-owner")).append(",\n");
        json.append("  \"tick\": ").append(a.acquireTick).append(",\n");
        json.append("  \"program\": ").append(jsonString(PROGRAM)).append(",\n");
        json.append("  \"a_acquired\": ").append(a.acquired).append(",\n");
        json.append("  \"a_controller_valid\": ").append(a.controllerValid).append(",\n");
        json.append("  \"a_controller_is_build\": ").append(a.controller == procA).append(",\n");
        json.append("  \"a_valid_after_b_placed\": ").append(aValidAfterBPlaced).append(",\n");
        json.append("  \"b_is_a\": ").append(bIsA).append(",\n");
        json.append("  \"still_logic_before_b_tick\": ").append(stillLogicBeforeBTick).append(",\n");
        json.append("  \"still_logic_after_b_tick\": ").append(stillLogicAfterBTick).append(",\n");
        json.append("  \"b_acquired\": ").append(b.acquired).append(",\n");
        json.append("  \"b_controller_is_b\": ").append(bControllerIsB).append(",\n");
        json.append("  \"b_controller_is_a\": ").append(bControllerIsA).append(",\n");
        json.append("  \"c_is_b\": ").append(cIsB).append(",\n");
        json.append("  \"still_logic_after_c_tick\": ").append(stillLogicAfterCTick).append(",\n");
        json.append("  \"wall_acquired\": ").append(wallAcquire.acquired).append(",\n");
        json.append("  \"still_logic_after_wall\": ").append(stillLogicAfterWall).append(",\n");
        json.append("  \"control_program\": ").append(jsonString(CONTROL)).append(",\n");
        json.append("  \"move_program\": ").append(jsonString(MOVE)).append(",\n");
        json.append("  \"unbind_program\": ").append(jsonString(UNBIND)).append(",\n");
        json.append("  \"lease_timer_at_acquire\": ").append(leaseTimerAtAcquire).append(",\n");
        json.append("  \"lease_timer_at_599\": ").append(leaseTimer599).append(",\n");
        json.append("  \"lease_controlled_at_599\": ").append(leaseControlled599).append(",\n");
        json.append("  \"lease_timer_at_600\": ").append(leaseTimer600).append(",\n");
        json.append("  \"lease_controlled_at_600\": ").append(leaseControlled600).append(",\n");
        json.append("  \"lease_controlled_at_601\": ").append(leaseControlled601).append(",\n");
        json.append("  \"refresh_timer_before\": ").append(refreshTimerBefore).append(",\n");
        json.append("  \"refresh_timer_after\": ").append(refreshTimerAfter).append(",\n");
        json.append("  \"refresh_controlled_at_600\": ").append(refreshControlled600).append(",\n");
        json.append("  \"refresh_controlled_at_601\": ").append(refreshControlled601).append(",\n");
        json.append("  \"gate_timer_after\": ").append(gateTimerAfter).append(",\n");
        json.append("  \"gate_flag_written\": ").append(gateFlagWritten).append(",\n");
        json.append("  \"gate_controlled_at_600\": ").append(gateControlled600).append(",\n");
        json.append("  \"gate_controlled_at_601\": ").append(gateControlled601).append(",\n");
        json.append("  \"unbind_logic_after\": ").append(unbindLogicAfter).append(",\n");
        json.append("  \"unbind_unit_var_kept\": ").append(unbindUnitVarKept).append(",\n");
        json.append("  \"unbind_has_command\": ").append(unbindHasCommand).append(",\n");
        json.append("  \"unbind_logic_controllable\": ").append(unbindLogicControllable).append(",\n");
        json.append("  \"unbind_reacquired\": ").append(unbindReacquired).append(",\n");
        json.append("  \"unbind_reacquire_timer\": ").append(unbindReacquireTimer).append(",\n");
        json.append("  \"rts_same_controller\": ").append(rtsSameController).append(",\n");
        json.append("  \"rts_became_logic\": ").append(rtsBecameLogic).append(",\n");
        json.append("  \"rts_has_command_after\": ").append(rtsHasCommandAfter).append(",\n");
        json.append("  \"rts_unit_var_kept\": ").append(rtsUnitVarKept).append("\n");
        json.append("}\n");
        System.out.print(json);
    }

    static boolean isLogic(Unit unit) {
        return unit.controller() instanceof LogicAI;
    }

    /** {@code LogicAI.controlTimer}, or NaN when the unit is not controlled. */
    static float timerOf(Unit unit) {
        return unit.controller() instanceof LogicAI la ? la.controlTimer : Float.NaN;
    }

    /** One lease clock pass: Time.delta = 1, then LogicAI.updateMovement. */
    static void leaseTick(Unit unit) {
        Time.delta = 1f;
        Time.time += Time.delta;
        if (unit.controller() instanceof LogicAI la) {
            la.updateMovement();
        }
    }

    /**
     * Runs exactly one instruction of {@code program} from {@code proc} with
     * @unit already pointing at {@code unit} — the state a preceding
     * {@code ubind} leaves behind (UnitBindI does {@code exec.unit.setconst}).
     */
    static void controlOnce(Building proc, Unit unit, String program) {
        LExecutor exec = executorFor(proc, program);
        exec.unit.setconst(unit);
        exec.runOnce();
    }

    /** Fresh controller, then one ucontrol: a full 600-tick lease on proc. */
    static void acquireLease(Building proc, Unit unit) {
        unit.resetController();
        controlOnce(proc, unit, CONTROL);
        if (!isLogic(unit)) {
            System.err.println("ParLogicOwner158: lease acquisition failed");
            System.exit(7);
        }
    }

    static Building placeProcessor(Tile tile) {
        tile.setBlock(Blocks.microProcessor, Team.sharded, 0);
        Building build = tile.build;
        if (!(build instanceof LogicBlock.LogicBuild)) {
            System.err.println("ParLogicOwner158: micro processor did not build a LogicBuild");
            System.exit(3);
        }
        return build;
    }

    static LExecutor executorFor(Building proc) {
        return executorFor(proc, PROGRAM);
    }

    static LExecutor executorFor(Building proc, String program) {
        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(program, false));
        if (!exec.initialized()) {
            System.err.println("ParLogicOwner158: executor did not initialize");
            System.exit(4);
        }
        exec.team = Team.sharded;
        exec.thisv.setconst(proc);
        return exec;
    }

    /** ubind then ucontrol on ticks 1-2; stop parks. Returns acquire observation. */
    static Phase driveUntilAcquired(LExecutor exec, Unit unit, Building proc) {
        Phase phase = new Phase();
        for (int tick = 1; tick <= 4; tick++) {
            Time.delta = 1f;
            Time.time += Time.delta;
            if (unit.controller() instanceof LogicAI la) {
                la.updateMovement();
            }
            exec.runOnce();
            if (!phase.acquired && unit.controller() instanceof LogicAI la) {
                phase.acquired = true;
                phase.acquireTick = tick;
                phase.controller = la.controller;
                phase.controllerValid = la.controller != null && la.controller.isValid();
                phase.controllerIsProc = la.controller == proc;
            }
        }
        return phase;
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParLogicOwner158.class.getResourceAsStream("/version.properties")) {
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

    static final class Phase {
        boolean acquired;
        int acquireTick = -1;
        Building controller;
        boolean controllerValid;
        boolean controllerIsProc;
    }
}
