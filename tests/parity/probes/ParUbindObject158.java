import java.io.InputStream;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.geom.Vec2;
import mindustry.Vars;
import mindustry.ai.types.CommandAI;
import mindustry.ai.types.GroundAI;
import mindustry.ai.types.LogicAI;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.Groups;
import mindustry.gen.Player;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.logic.LAssembler;
import mindustry.logic.LExecutor;
import mindustry.logic.LVar;
import mindustry.world.Tiles;

/**
 * P0-B1 differential probe: `ubind <Unit object>` on desktop.jar 158.1.
 *
 * Official UnitBindI.run Unit-object branch (LExecutor.java:200-202) binds
 * the given unit when
 *
 *   (u.team == exec.team || exec.privileged) && u.type.logicControllable
 *
 * and otherwise sets @unit to null. Controller-level eligibility
 * (`unit.controller().isLogicControllable()`) is NOT consulted — that gate
 * belongs to ucontrol's checkLogicAI. ubind itself never installs a LogicAI.
 *
 * Six cases, one runOnce of `ubind u` each, against a headless 16x16 world
 * (no game loop, no client):
 *
 *   1 same_team_default_ai          sharded dagger + GroundAI          → bind
 *   2 same_team_command_ai_active   sharded dagger + CommandAI target  → bind
 *   3 same_team_player              sharded dagger + Player            → bind
 *   4 enemy_nonprivileged           crux dagger, exec.privileged=false → null
 *   5 enemy_privileged              crux dagger, exec.privileged=true  → bind
 *   6 not_logic_controllable        sharded assembly-drone             → null
 *
 * Version gate: refuses to run unless the classpath version.properties
 * reports the official 158.1 build.
 */
public final class ParUbindObject158 {
    static final int WORLD = 16;
    static final String PROGRAM = "ubind u";

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParUbindObject158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

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
        Vars.state.rules.waves = true; // only sharded is player-commandable
        Vars.state.rules.disableUnitCap = true;
        Vars.state.rules.logicUnitControl = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();
        Vars.state.teams.updateTeamStats();

        if (UnitTypes.assemblyDrone.logicControllable) {
            System.err.println("ParUbindObject158: assembly-drone unexpectedly logicControllable");
            System.exit(3);
        }

        // --- 1: same team + DefaultAI (GroundAI, isLogicControllable true) --
        Unit d1 = spawn(Team.sharded, 80f, 11);
        d1.controller(new GroundAI());
        Case c1 = runCase(d1, false);
        if (!c1.bound) {
            System.err.println("ParUbindObject158: case 1 (DefaultAI) did not bind");
            System.exit(5);
        }

        // --- 2: same team + CommandAI with an active target -----------------
        Unit d2 = spawn(Team.sharded, 96f, 12);
        if (!(d2.controller() instanceof CommandAI)) {
            System.err.println("ParUbindObject158: sharded dagger did not receive a CommandAI");
            System.exit(3);
        }
        CommandAI ai = (CommandAI) d2.controller();
        ai.commandQueue(new Vec2(100f, 100f));
        if (!ai.hasCommand() || ai.isLogicControllable()) {
            System.err.println("ParUbindObject158: CommandAI did not take an active target");
            System.exit(3);
        }
        Case c2 = runCase(d2, false);
        if (!c2.bound) {
            System.err.println("ParUbindObject158: case 2 (CommandAI active) did not bind");
            System.exit(5);
        }

        // --- 3: same team + Player authority --------------------------------
        Unit d3 = spawn(Team.sharded, 112f, 13);
        Player p = Player.create();
        p.team(Team.sharded);
        p.unit(d3);
        if (!d3.isPlayer() || d3.controller().isLogicControllable()) {
            System.err.println("ParUbindObject158: player possession did not stick");
            System.exit(3);
        }
        Case c3 = runCase(d3, false);
        if (!c3.bound) {
            System.err.println("ParUbindObject158: case 3 (Player) did not bind");
            System.exit(5);
        }

        // --- 4: enemy + nonprivileged → null --------------------------------
        Unit d4 = spawn(Team.crux, 128f, 14);
        Case c4 = runCase(d4, false);
        if (c4.bound) {
            System.err.println("ParUbindObject158: case 4 (enemy nonprivileged) bound");
            System.exit(5);
        }

        // --- 5: enemy + privileged → bind -----------------------------------
        Unit d5 = spawn(Team.crux, 144f, 15);
        Case c5 = runCase(d5, true);
        if (!c5.bound) {
            System.err.println("ParUbindObject158: case 5 (enemy privileged) did not bind");
            System.exit(5);
        }

        // --- 6: type.logicControllable = false → null -----------------------
        Unit drone = UnitTypes.assemblyDrone.create(Team.sharded);
        drone.set(160f, 80f);
        drone.flag(16);
        drone.add();
        if (drone.type.logicControllable) {
            System.err.println("ParUbindObject158: assembly-drone type.logicControllable is true");
            System.exit(3);
        }
        Case c6 = runCase(drone, false);
        if (c6.bound) {
            System.err.println("ParUbindObject158: case 6 (not logicControllable) bound");
            System.exit(5);
        }

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("ubind-object")).append(",\n");
        json.append("  \"tick\": 6,\n");
        json.append("  \"executions\": 6,\n");
        json.append("  \"program\": ").append(jsonString(PROGRAM)).append(",\n");
        json.append("  \"processor_team\": ").append(Team.sharded.id).append(",\n");
        json.append("  \"cases\": {\n");
        json.append(caseJson("same_team_default_ai", c1)).append(",\n");
        json.append(caseJson("same_team_command_ai_active", c2)).append(",\n");
        json.append(caseJson("same_team_player", c3)).append(",\n");
        json.append(caseJson("enemy_nonprivileged", c4)).append(",\n");
        json.append(caseJson("enemy_privileged", c5)).append(",\n");
        json.append(caseJson("not_logic_controllable", c6)).append("\n");
        json.append("  }\n}\n");
        System.out.print(json);
    }

    static Unit spawn(Team team, float x, double flag) {
        Unit unit = UnitTypes.dagger.create(team);
        unit.set(x, 80f);
        unit.flag(flag);
        unit.add();
        return unit;
    }

    /**
     * Fresh LExecutor, `ubind u` once with {@code u} holding {@code unit}.
     * {@code exec.team} is always sharded; privilege is the only executor
     * flag that varies across cases.
     */
    static Case runCase(Unit unit, boolean privileged) {
        String before = unit.controller().getClass().getName();
        boolean typeOk = unit.type.logicControllable;
        boolean ctrlOk = unit.controller().isLogicControllable();
        double flag = unit.flag();

        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(PROGRAM, false));
        if (!exec.initialized()) {
            System.err.println("ParUbindObject158: executor did not initialize");
            System.exit(4);
        }
        exec.team = Team.sharded;
        exec.privileged = privileged;
        LVar u = exec.optionalVar("u");
        if (u == null) {
            System.err.println("ParUbindObject158: assembler did not produce variable u");
            System.exit(4);
        }
        u.setobj(unit);
        exec.runOnce();

        Case out = new Case();
        out.bound = exec.unit.obj() == unit;
        out.typeLogic = typeOk;
        out.controllerLogic = ctrlOk;
        out.controllerUnchanged = before.equals(unit.controller().getClass().getName());
        out.acquiredLogic = unit.controller() instanceof LogicAI;
        out.flag = flag;
        out.controller = unit.controller().getClass().getSimpleName();
        return out;
    }

    static String caseJson(String name, Case c) {
        StringBuilder sb = new StringBuilder("    ");
        sb.append(jsonString(name)).append(": {");
        sb.append("\"bound\": ").append(c.bound);
        sb.append(", \"type_logic_controllable\": ").append(c.typeLogic);
        sb.append(", \"controller_logic_controllable\": ").append(c.controllerLogic);
        sb.append(", \"controller_unchanged\": ").append(c.controllerUnchanged);
        sb.append(", \"acquired_logic\": ").append(c.acquiredLogic);
        sb.append(", \"controller\": ").append(jsonString(c.controller));
        if (c.bound) {
            sb.append(", \"flag\": ").append((long) c.flag);
        }
        sb.append("}");
        return sb.toString();
    }

    static final class Case {
        boolean bound;
        boolean typeLogic;
        boolean controllerLogic;
        boolean controllerUnchanged;
        boolean acquiredLogic;
        double flag;
        String controller;
    }

    /** Reads version.properties from the classpath (the desktop.jar root). */
    static String classpathVersion() throws Exception {
        try (InputStream in = ParUbindObject158.class.getResourceAsStream("/version.properties")) {
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
