import java.io.InputStream;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.geom.Rect;
import mindustry.Vars;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.Groups;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.logic.LAssembler;
import mindustry.logic.LExecutor;
import mindustry.world.Tiles;

/**
 * P0-02 differential probe: `ubind` round-robin on desktop.jar 158.1.
 *
 * Minimal headless world (no game loop, no client): five sharded daggers
 * created in order with logic flags 11..15, registered in the sharded
 * TeamData unit cache through the official path
 * (`Teams.updateTeamStats`, which rebuilds `TeamData.unitsByType` by
 * iterating `Groups.unit` — insertion order = creation order), then an
 * LExecutor with `exec.team = Team.sharded` runs the program below once
 * per tick (runOnce, 1 instruction/tick, exactly like ParLogic158):
 *
 *   ubind @dagger          idx 0   bind the next sharded dagger
 *   sensor n @unit @flag   idx 1   the bound unit's logic flag
 *   print n                idx 2
 *   print " "              idx 3
 *   op add i i 1           idx 4
 *   jump 0 lessThan i 20   idx 5   20 ubind executions
 *   stop                   idx 6   park forever
 *
 * After 200 runOnce calls the textBuffer holds the 20 bound flags —
 * "11 12 13 14 15 11 12 13 14 15 " for a 1..5 round-robin — which is the
 * full observable ubind sequence. The Rust side creates the same five
 * units (ids in creation order, same flags) and replays the identical
 * program through ExecutorState + WorldView.
 *
 * The `@flag` property is used instead of `@id` because unit entity ids
 * are JVM-global counters: the flag is assigned per creation index, so
 * the sequence is comparable across languages (and UnitComp.sense handles
 * `flag` while `id` is not sensed by units at all in 158.1).
 *
 * Version gate: refuses to run unless the classpath version.properties
 * reports the official 158.1 build.
 */
public final class ParUbind158 {
    /** runOnce calls; one instruction per call (121 needed to park on stop). */
    static final int TICKS = 200;
    static final int UNITS = 5;
    static final int WORLD = 16;

    /** The exact logic source replayed by the Rust side. */
    static final String PROGRAM =
        "ubind @dagger\n"
        + "sensor n @unit @flag\n"
        + "print n\n"
        + "print \" \"\n"
        + "op add i i 1\n"
        + "jump 0 lessThan i 20\n"
        + "stop";

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParUbind158: refusing to run: classpath version.properties reports '" + version
                + "', expected official 158.1");
            System.exit(2);
        }

        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Groups.init();
        Vars.state = new GameState();
        Vars.state.rules.disableUnitCap = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();

        // @dagger (UnitType) and @flag (LAccess) resolve through
        // Vars.logicVars; init() reads Core.files only for logicids.dat,
        // which this probe's programs never look up.
        Core.files = new SdlFiles();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();

        // Five sharded daggers in creation order, flags 11..15.
        for (int i = 1; i <= UNITS; i++) {
            Unit unit = UnitTypes.dagger.create(Team.sharded);
            unit.set(80f + i * 8f, 80f);
            unit.flag(10 + i);
            Groups.unit.add(unit);
        }
        // Official cache rebuild (Teams.updateTeamStats populates
        // TeamData.unitsByType from Groups.unit in insertion order).
        Vars.state.teams.updateTeamStats();
        var cache = Team.sharded.data().unitCache(UnitTypes.dagger);
        if (cache == null || cache.size != UNITS) {
            System.err.println("ParUbind158: unit cache wrong: " + (cache == null ? "null" : cache.size));
            System.exit(3);
        }
        StringBuilder cacheFlags = new StringBuilder("[");
        for (int i = 0; i < cache.size; i++) {
            if (i > 0) cacheFlags.append(", ");
            cacheFlags.append((long) cache.get(i).flag());
        }
        cacheFlags.append("]");

        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(PROGRAM, false));
        if (!exec.initialized()) {
            System.err.println("ParUbind158: executor did not initialize");
            System.exit(4);
        }
        // LogicBuild.updateTile assigns executor.team = team every tick.
        exec.team = Team.sharded;

        for (int tick = 0; tick < TICKS; tick++) {
            exec.runOnce();
        }

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("ubind-20")).append(",\n");
        json.append("  \"tick\": ").append(TICKS).append(",\n");
        json.append("  \"executions\": ").append(TICKS).append(",\n");
        json.append("  \"program\": ").append(jsonString(PROGRAM)).append(",\n");
        json.append("  \"processor_team\": ").append(Team.sharded.id).append(",\n");
        json.append("  \"unit_count\": ").append(UNITS).append(",\n");
        json.append("  \"cache_flags\": ").append(cacheFlags).append(",\n");
        json.append("  \"counter\": ").append((long) exec.counter.numval).append(",\n");
        json.append("  \"text\": ").append(jsonString(exec.textBuffer.toString())).append(",\n");
        json.append("  \"vars\": {");
        boolean first = true;
        if (exec.vars != null) {
            java.util.TreeMap<String, mindustry.logic.LVar> userVars = new java.util.TreeMap<>();
            for (mindustry.logic.LVar v : exec.vars) {
                if (v != null && !v.name.startsWith("@")) {
                    userVars.put(v.name, v);
                }
            }
            for (var entry : userVars.entrySet()) {
                if (!first) json.append(",");
                first = false;
                mindustry.logic.LVar v = entry.getValue();
                json.append("\n    ").append(jsonString(entry.getKey())).append(": {\"isobj\": ")
                    .append(v.isobj).append(", \"num\": ").append(v.numval).append("}");
            }
        }
        json.append("\n  }\n}\n");
        System.out.print(json);
    }

    /** Reads version.properties from the classpath (the desktop.jar root). */
    static String classpathVersion() throws Exception {
        try (InputStream in = ParUbind158.class.getResourceAsStream("/version.properties")) {
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
