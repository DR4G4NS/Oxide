import java.io.InputStream;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.geom.Vec2;
import arc.util.Time;
import mindustry.Vars;
import mindustry.ai.types.CommandAI;
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
import mindustry.world.Tile;
import mindustry.world.Tiles;

/**
 * P0-04 differential probe: RTS CommandAI order semantics on desktop.jar
 * 158.1 — first-queued-target promotion, queue dedup, the 50-entry cap,
 * per-update invalidation of queued/active building and unit targets, and
 * hasCommand()/isLogicControllable() across arrival-driven finishPath
 * progression.
 *
 * Minimal headless world (no game loop, no client) following ParLease158:
 * one sharded FLARE (flying, so defaultBehavior never consults the
 * ControlPathfinder — grounded units would need pathfinder state) whose
 * factory controller is already a CommandAI (UnitComp.set -> type.
 * createController for a player-commandable team), plus one crux wall
 * building and one crux dagger as Healthc targets.
 *
 * The probe never relies on the physics loop: the unit is TELEPORTED onto
 * each active target before the update that should observe the arrival, so
 * finishPath progression is deterministic. Per game tick:
 *
 *   Time.delta = 1f; Time.time += 1f; ai.updateUnit();
 *
 * Scenarios (all state read directly off the CommandAI):
 *   A promotion  — commandQueue(vec) on a fresh AI becomes the ACTIVE
 *                  command (targetPos set), queue stays empty
 *                  (CommandAI.java:494-499).
 *   B dedup      — two more DISTINCT Vec2 instances with identical
 *                  coordinates: one appended, the second skipped
 *                  (contains + Vec2.equals by exact bits, CommandAI.java:500).
 *   C cap        — direct active target + 60 unique queued positions: the
 *                  queue stops at maxCommandQueueSize = 50
 *                  (CommandAI.java:19, 500).
 *   D arrival    — four teleport+update steps pop four queue entries in
 *                  FIFO order (finishPath, CommandAI.java:412-486).
 *   E building   — a queued wall is pruned the update after its tile is
 *                  destroyed (CommandAI.java:136-139); an ACTIVE attack
 *                  building target clears targetPos on invalidation
 *                  (CommandAI.java:244-247) and finishes the path.
 *   F unit       — a queued enemy unit is pruned the update after
 *                  Groups removal (isValid false).
 *   G exhaust    — after the last queued target is consumed, hasCommand()
 *                  is false and isLogicControllable() true again.
 *
 * Version gate: refuses to run unless the classpath version.properties
 * reports the official 158.1 build.
 */
public final class ParCommand158 {
    static final int WORLD = 16;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParCommand158: refusing to run: classpath version.properties reports '" + version
                + "', expected official 158.1");
            System.exit(2);
        }

        // Shared headless setup: one world, one wall, one foe, one flare.
        Vars.headless = true;
        Vars.platform = new mindustry.core.Platform(){};
        Vars.net = new mindustry.net.Net(Vars.platform.getNet());
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
        Tile wallTile = Vars.world.tiles.get(8, 8);

        Unit foe = UnitTypes.dagger.create(Team.crux);
        foe.set(120f, 120f);
        foe.add();

        // Flying + player-commandable team => factory CommandAI controller.
        Unit unit = UnitTypes.flare.create(Team.sharded);
        unit.set(80f, 80f);
        unit.add();
        Vars.state.teams.updateTeamStats();

        if (!(unit.controller() instanceof CommandAI)) {
            System.err.println("ParCommand158: flare did not receive a CommandAI controller");
            System.exit(3);
        }
        CommandAI ai = (CommandAI) unit.controller();

        long ticks = 0;

        // --- A: first queued command is promoted to active ----------------
        ai.commandQueue(new Vec2(100f, 100f));
        boolean aHasCommand = ai.hasCommand();
        int aQueue = ai.commandQueue.size;
        float aX = targetX(ai), aY = targetY(ai);
        boolean aLogic = ai.isLogicControllable();

        // --- B: dedup of distinct-but-equal Vec2 instances -----------------
        ai.commandQueue(new Vec2(100f, 100f));
        int bAfterSecond = ai.commandQueue.size;
        ai.commandQueue(new Vec2(100f, 100f));
        int bAfterThird = ai.commandQueue.size;

        // --- C: queue cap --------------------------------------------------
        ai.clearCommands();
        ai.commandPosition(new Vec2(200f, 200f));
        for (int i = 0; i < 60; i++) {
            ai.commandQueue(new Vec2(300f + i * 7f, 300f));
        }
        int cQueue = ai.commandQueue.size;
        boolean cHasCommand = ai.hasCommand();

        // --- D: arrival pops the queue in FIFO order ------------------------
        // Queue holds P_i = (300 + i*7, 300), active is (200, 200).
        boolean[] dHas = new boolean[4];
        int[] dQueue = new int[4];
        float[] dX = new float[4], dY = new float[4];
        for (int step = 0; step < 4; step++) {
            // Teleport onto the CURRENT active target, then update: the
            // arrival branch fires finishPath, promoting queue head.
            unit.set(targetX(ai), targetY(ai));
            tick(ai);
            ticks++;
            dHas[step] = ai.hasCommand();
            dQueue[step] = ai.commandQueue.size;
            dX[step] = targetX(ai);
            dY[step] = targetY(ai);
        }

        // --- E: building targets --------------------------------------------
        ai.clearCommands();
        wallTile.setBlock(Blocks.copperWall, Team.crux, 0);
        Building wall = wallTile.build;
        ai.commandPosition(new Vec2(200f, 200f));
        ai.commandQueue(wall);
        ai.commandQueue(new Vec2(400f, 400f));
        int eQueueBefore = ai.commandQueue.size;
        wallTile.setBlock(Blocks.air, Team.derelict, 0); // destroyed
        tick(ai);
        ticks++;
        int eQueueAfter = ai.commandQueue.size;

        // Active attack building target: transient before the first update
        // (commandTarget sets attackTarget only, targetPos stays null), then
        // materialized by defaultBehavior, then cleared when invalidated.
        wallTile.setBlock(Blocks.copperWall, Team.crux, 0);
        Building wall2 = wallTile.build;
        ai.clearCommands();
        ai.commandTarget(wall2);
        boolean eActiveHasCommandBefore = ai.hasCommand();
        tick(ai);
        ticks++;
        boolean eActiveHasCommandAfter = ai.hasCommand();
        wallTile.setBlock(Blocks.air, Team.derelict, 0); // destroyed
        tick(ai);
        ticks++;
        boolean eActiveHasCommandDestroyed = ai.hasCommand();
        boolean eActiveLogicAfterDestroy = ai.isLogicControllable();

        // --- F: queued enemy unit --------------------------------------------
        ai.clearCommands();
        ai.commandPosition(new Vec2(200f, 200f));
        ai.commandQueue(foe);
        int fQueueBefore = ai.commandQueue.size;
        foe.remove(); // Groups deregistration -> isValid false
        tick(ai);
        ticks++;
        int fQueueAfter = ai.commandQueue.size;

        // --- G: exhausting the queue restores logic control -------------------
        ai.clearCommands();
        ai.commandQueue(new Vec2(80f, 80f));  // promoted: active
        ai.commandQueue(new Vec2(90f, 90f));  // queued
        boolean gBlocked = ai.isLogicControllable();
        unit.set(80f, 80f);
        tick(ai);
        ticks++;
        boolean gAfterFirst = ai.hasCommand(); // popped (90,90): still active
        unit.set(90f, 90f);
        tick(ai);
        ticks++;
        boolean gRestored = ai.isLogicControllable();
        boolean gFinalHasCommand = ai.hasCommand();

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("command-queue")).append(",\n");
        json.append("  \"tick\": ").append(ticks).append(",\n");
        json.append("  \"unit_team\": ").append(Team.sharded.id).append(",\n");
        json.append("  \"promote_has_command\": ").append(aHasCommand).append(",\n");
        json.append("  \"promote_queue_size\": ").append(aQueue).append(",\n");
        json.append("  \"promote_target_x\": ").append(aX).append(",\n");
        json.append("  \"promote_target_y\": ").append(aY).append(",\n");
        json.append("  \"promote_logic_controllable\": ").append(aLogic).append(",\n");
        json.append("  \"dedup_after_second\": ").append(bAfterSecond).append(",\n");
        json.append("  \"dedup_after_third\": ").append(bAfterThird).append(",\n");
        json.append("  \"cap_queue_size\": ").append(cQueue).append(",\n");
        json.append("  \"cap_has_command\": ").append(cHasCommand).append(",\n");
        for (int step = 0; step < 4; step++) {
            json.append("  \"arrival_").append(step).append("_has_command\": ").append(dHas[step]).append(",\n");
            json.append("  \"arrival_").append(step).append("_queue_size\": ").append(dQueue[step]).append(",\n");
            json.append("  \"arrival_").append(step).append("_target_x\": ").append(dX[step]).append(",\n");
            json.append("  \"arrival_").append(step).append("_target_y\": ").append(dY[step]).append(",\n");
        }
        json.append("  \"building_queue_before\": ").append(eQueueBefore).append(",\n");
        json.append("  \"building_queue_after_destroy\": ").append(eQueueAfter).append(",\n");
        // Informative only (the Rust order materializes targetPos eagerly;
        // Java keeps a one-update transient with attackTarget-only state).
        json.append("  \"attack_active_before_update_has_command\": ").append(eActiveHasCommandBefore).append(",\n");
        json.append("  \"attack_active_after_update_has_command\": ").append(eActiveHasCommandAfter).append(",\n");
        json.append("  \"attack_active_after_destroy_has_command\": ").append(eActiveHasCommandDestroyed).append(",\n");
        json.append("  \"attack_active_logic_after_destroy\": ").append(eActiveLogicAfterDestroy).append(",\n");
        json.append("  \"unit_queue_before\": ").append(fQueueBefore).append(",\n");
        json.append("  \"unit_queue_after_remove\": ").append(fQueueAfter).append(",\n");
        json.append("  \"exhaust_blocked_logic\": ").append(gBlocked).append(",\n");
        json.append("  \"exhaust_after_first_has_command\": ").append(gAfterFirst).append(",\n");
        json.append("  \"exhaust_restored_logic\": ").append(gRestored).append(",\n");
        json.append("  \"exhaust_final_has_command\": ").append(gFinalHasCommand).append("\n");
        json.append("}\n");
        System.out.print(json);
    }

    /** One game tick for the CommandAI only (no physics loop, no client). */
    static void tick(CommandAI ai) {
        Time.delta = 1f;
        Time.time += Time.delta;
        ai.updateUnit();
    }

    static float targetX(CommandAI ai) {
        return ai.targetPos == null ? Float.NaN : ai.targetPos.x;
    }

    static float targetY(CommandAI ai) {
        return ai.targetPos == null ? Float.NaN : ai.targetPos.y;
    }

    /** Reads version.properties from the classpath (the desktop.jar root). */
    static String classpathVersion() throws Exception {
        try (InputStream in = ParCommand158.class.getResourceAsStream("/version.properties")) {
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
