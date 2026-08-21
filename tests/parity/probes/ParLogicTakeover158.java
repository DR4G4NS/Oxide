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
import mindustry.entities.units.BuildPlan;
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
 * P0-C2 differential probe: first LogicAI acquisition on a Builder unit
 * (poly) clears {@code unit.mineTile} and {@code unit.clearBuilding()}
 * (desktop 158.1 {@code UnitControlI.checkLogicAI}). A later {@code ucontrol
 * move} on the same LogicAI does not repeat that wipe — only {@code ucontrol
 * stop} does. A failed {@code checkLogicAI} gate (CommandAI.hasCommand) is a
 * complete no-op on mining and plans.
 *
 * Cadence matches ParLease158: Time.delta = 1, then exec.runOnce (one
 * instruction). Version-gated to official 158.1.
 */
public final class ParLogicTakeover158 {
    static final int WORLD = 16;
    static final String PROGRAM =
        "ubind @poly\n"
        + "ucontrol move 10 10\n"
        + "ucontrol move 20 20\n"
        + "stop";

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParLogicTakeover158: refusing to run: classpath version.properties reports '"
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

        Tile procTile = Vars.world.tiles.get(8, 8);
        Tile ore = Vars.world.tiles.get(4, 4);
        Building proc = placeProcessor(procTile);

        Unit unit = UnitTypes.poly.create(Team.sharded);
        unit.set(80f, 80f);
        unit.add();
        Vars.state.teams.updateTeamStats();

        seedDirty(unit, ore);

        LExecutor exec = executorFor(proc, PROGRAM);
        tick(exec, unit); // ubind
        tick(exec, unit); // first ucontrol move: takeover + wipe
        boolean firstLogic = unit.controller() instanceof LogicAI;
        boolean firstMineNull = unit.mineTile == null;
        boolean firstPlansEmpty = unit.plans.size == 0;
        int firstPlans = unit.plans.size;

        seedDirty(unit, ore);
        tick(exec, unit); // second ucontrol move: refresh only
        boolean secondLogic = unit.controller() instanceof LogicAI;
        boolean secondMineKept = unit.mineTile == ore;
        boolean secondPlansKept = unit.plans.size > 0;
        int secondPlans = unit.plans.size;

        unit.resetController();
        if (!(unit.controller() instanceof CommandAI)) {
            System.err.println("ParLogicTakeover158: resetController did not install CommandAI");
            System.exit(5);
        }
        CommandAI ai = (CommandAI) unit.controller();
        ai.commandPosition(new Vec2(400f, 400f));
        seedDirty(unit, ore);
        boolean failWasCommand = unit.controller() instanceof CommandAI;
        boolean failHasCommand = ai.hasCommand();
        LExecutor failExec = executorFor(proc, "ucontrol move 30 30\nstop");
        failExec.unit.setconst(unit);
        tick(failExec, unit);
        boolean failStillCommand = unit.controller() instanceof CommandAI;
        boolean failMineKept = unit.mineTile == ore;
        boolean failPlansKept = unit.plans.size > 0;
        boolean failBecameLogic = unit.controller() instanceof LogicAI;

        if (!firstLogic || !secondLogic) {
            System.err.println("ParLogicTakeover158: scenario incomplete: firstLogic=" + firstLogic
                + " secondLogic=" + secondLogic);
            System.exit(5);
        }

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("logic-takeover")).append(",\n");
        json.append("  \"tick\": 2,\n");
        json.append("  \"program\": ").append(jsonString(PROGRAM)).append(",\n");
        json.append("  \"first_logic\": ").append(firstLogic).append(",\n");
        json.append("  \"first_mine_null\": ").append(firstMineNull).append(",\n");
        json.append("  \"first_plans_empty\": ").append(firstPlansEmpty).append(",\n");
        json.append("  \"first_plans\": ").append(firstPlans).append(",\n");
        json.append("  \"second_logic\": ").append(secondLogic).append(",\n");
        json.append("  \"second_mine_kept\": ").append(secondMineKept).append(",\n");
        json.append("  \"second_plans_kept\": ").append(secondPlansKept).append(",\n");
        json.append("  \"second_plans\": ").append(secondPlans).append(",\n");
        json.append("  \"fail_was_command\": ").append(failWasCommand).append(",\n");
        json.append("  \"fail_has_command\": ").append(failHasCommand).append(",\n");
        json.append("  \"fail_still_command\": ").append(failStillCommand).append(",\n");
        json.append("  \"fail_mine_kept\": ").append(failMineKept).append(",\n");
        json.append("  \"fail_plans_kept\": ").append(failPlansKept).append(",\n");
        json.append("  \"fail_became_logic\": ").append(failBecameLogic).append("\n");
        json.append("}\n");
        System.out.print(json);
    }

    static void seedDirty(Unit unit, Tile ore) {
        unit.mineTile = ore;
        unit.clearBuilding();
        unit.addBuild(new BuildPlan(2, 2, 0, Blocks.copperWall));
    }

    static void tick(LExecutor exec, Unit unit) {
        Time.delta = 1f;
        Time.time += Time.delta;
        if (unit.controller() instanceof LogicAI la) {
            la.updateMovement();
        }
        exec.runOnce();
    }

    static Building placeProcessor(Tile tile) {
        tile.setBlock(Blocks.microProcessor, Team.sharded, 0);
        Building build = tile.build;
        if (!(build instanceof LogicBlock.LogicBuild)) {
            System.err.println("ParLogicTakeover158: micro processor did not build a LogicBuild");
            System.exit(3);
        }
        return build;
    }

    static LExecutor executorFor(Building proc, String program) {
        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(program, false));
        if (!exec.initialized()) {
            System.err.println("ParLogicTakeover158: executor did not initialize");
            System.exit(4);
        }
        exec.team = Team.sharded;
        exec.thisv.setconst(proc);
        return exec;
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParLogicTakeover158.class.getResourceAsStream("/version.properties")) {
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
