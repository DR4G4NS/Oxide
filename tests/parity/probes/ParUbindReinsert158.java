import java.io.InputStream;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.Mathf;
import mindustry.Vars;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.entities.EntityGroup;
import mindustry.game.Team;
import mindustry.gen.Groups;
import mindustry.gen.Payloadc;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.logic.LAssembler;
import mindustry.logic.LExecutor;
import mindustry.world.Tiles;
import mindustry.world.blocks.payloads.UnitPayload;

/**
 * P0-B2 differential probe: `ubind @UnitType` order after Groups.unit
 * remove/re-add, on desktop.jar 158.1.
 *
 * Official path (Teams.updateTeamStats → TeamData.unitCache): the cache is
 * rebuilt by iterating Groups.unit. EntityGroup's backing Seq is constructed
 * unordered (ordered=false), so remove is swap-remove and add appends.
 * This probe records the observable flag sequence — never JVM entity ids.
 *
 * Scenarios (flags 11=A, 12=B, 13=C, 14=D):
 *   baseline      spawn A,B,C → 12× ubind
 *   readd         spawn A,B,C, B.remove(), B.add() → 12× ubind
 *   payload       spawn A,B,C, mega.pickup(B), drop B back → 12× ubind
 *   death_spawn   spawn A,B,C, B dies, spawn D → 12× ubind
 *   cursor_len    2× ubind, remove A (seq.len changes), 4× more ubind
 *   four_remove_b spawn A,B,C,D, remove B (no re-add) → cache + 12× ubind
 *
 * Version gate: refuses to run unless classpath version.properties is
 * official 158.1.
 */
public final class ParUbindReinsert158 {
    static final int WORLD = 32;
    static final int BIND_COUNT = 12;
    static final int TICKS = 100;

    static final String PROGRAM =
        "ubind @dagger\n"
        + "sensor n @unit @flag\n"
        + "print n\n"
        + "print \" \"\n"
        + "op add i i 1\n"
        + "jump 0 lessThan i 12\n"
        + "stop";

    static final String CURSOR_PROGRAM =
        "ubind @dagger\n"
        + "sensor n @unit @flag\n"
        + "print n\n"
        + "print \" \"\n"
        + "op add i i 1\n"
        + "jump 0 lessThan i 100\n"
        + "stop";

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParUbindReinsert158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

        boot();

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("ubind-reinsert")).append(",\n");
        json.append("  \"tick\": ").append(BIND_COUNT).append(",\n");
        json.append("  \"executions\": ").append(BIND_COUNT).append(",\n");
        json.append("  \"program\": ").append(jsonString(PROGRAM)).append(",\n");
        json.append("  \"processor_team\": ").append(Team.sharded.id).append(",\n");
        json.append("  \"scenarios\": {\n");

        json.append(scenarioJson("baseline", runBaseline())).append(",\n");
        json.append(scenarioJson("readd", runReadd())).append(",\n");
        json.append(scenarioJson("payload", runPayload())).append(",\n");
        json.append(scenarioJson("death_spawn", runDeathSpawn())).append(",\n");
        json.append(cursorJson(runCursorLen())).append(",\n");
        json.append(scenarioJson("four_remove_b", runFourRemoveB())).append("\n");

        json.append("  }\n}\n");
        System.out.print(json);
    }

    static Scenario runBaseline() {
        clearUnits();
        Unit a = spawnDagger(11, 80f);
        Unit b = spawnDagger(12, 96f);
        Unit c = spawnDagger(13, 112f);
        Vars.state.teams.updateTeamStats();
        return capture("baseline", new Unit[]{a, b, c});
    }

    static Scenario runReadd() {
        clearUnits();
        Unit a = spawnDagger(11, 80f);
        Unit b = spawnDagger(12, 96f);
        Unit c = spawnDagger(13, 112f);
        Vars.state.teams.updateTeamStats();
        if (!b.isAdded()) {
            System.err.println("ParUbindReinsert158: B was not added before remove");
            System.exit(5);
        }
        b.remove();
        b.add();
        Vars.state.teams.updateTeamStats();
        return capture("readd", new Unit[]{a, b, c});
    }

    static Scenario runPayload() {
        clearUnits();
        Unit a = spawnDagger(11, 80f);
        Unit b = spawnDagger(12, 96f);
        Unit c = spawnDagger(13, 112f);
        Unit megaUnit = UnitTypes.mega.create(Team.sharded);
        megaUnit.set(96f, 120f);
        megaUnit.add();
        Payloadc mega = (Payloadc) megaUnit;
        Vars.state.teams.updateTeamStats();

        String payloadPath = "pickup+dropUnit";
        try {
            if (!mega.canPickup(b)) {
                payloadPath = "groups-equivalent (canPickup false)";
                payloadRejoin(b);
            } else {
                mega.pickup(b);
                if (b.isAdded()) {
                    payloadPath = "groups-equivalent (still added after pickup)";
                    payloadRejoin(b);
                } else {
                    Mathf.rand.setSeed(1L);
                    boolean dropped = mega.dropUnit(new UnitPayload(b));
                    if (!dropped) {
                        payloadPath = "groups-equivalent (dropUnit false)";
                        payloadRejoin(b);
                    }
                }
            }
        } catch (Throwable t) {
            payloadPath = "groups-equivalent (" + t.getClass().getSimpleName() + ")";
            payloadRejoin(b);
        }
        Vars.state.teams.updateTeamStats();
        Scenario s = capture("payload", new Unit[]{a, b, c});
        s.payloadPath = payloadPath;
        return s;
    }

    /** Official dropUnit Groups effect: new id, then add() (append). */
    static void payloadRejoin(Unit b) {
        if (b.isAdded()) b.remove();
        b.id = EntityGroup.nextId();
        if (!b.isAdded()) b.team.data().updateCount(b.type, -1);
        b.add();
    }

    static Scenario runDeathSpawn() {
        clearUnits();
        Unit a = spawnDagger(11, 80f);
        Unit b = spawnDagger(12, 96f);
        Unit c = spawnDagger(13, 112f);
        Vars.state.teams.updateTeamStats();
        b.health = 0f;
        b.remove();
        Unit d = spawnDagger(14, 128f);
        Vars.state.teams.updateTeamStats();
        return capture("death_spawn", new Unit[]{a, c, d});
    }

    static CursorScenario runCursorLen() {
        clearUnits();
        Unit a = spawnDagger(11, 80f);
        spawnDagger(12, 96f);
        spawnDagger(13, 112f);
        Vars.state.teams.updateTeamStats();

        LExecutor exec = newExecutor(CURSOR_PROGRAM);
        // 2 binds: 2 loops × 6 instructions.
        for (int i = 0; i < 12; i++) exec.runOnce();
        String before = exec.textBuffer.toString();

        a.remove();
        Vars.state.teams.updateTeamStats();
        var cache = Team.sharded.data().unitCache(UnitTypes.dagger);

        for (int i = 0; i < 24; i++) exec.runOnce();
        String after = exec.textBuffer.toString().substring(before.length());

        CursorScenario s = new CursorScenario();
        s.textBefore = before;
        s.textAfter = after;
        s.text = exec.textBuffer.toString();
        s.cacheFlags = cacheFlags(cache);
        s.groupsFlags = groupsDaggerFlags();
        s.idSortedFlags = idSortedFlags();
        s.differsFromIdSort = !s.cacheFlags.equals(s.idSortedFlags);
        s.seqLen = cache == null ? 0 : cache.size;
        return s;
    }

    static Scenario runFourRemoveB() {
        clearUnits();
        Unit a = spawnDagger(11, 80f);
        Unit b = spawnDagger(12, 96f);
        Unit c = spawnDagger(13, 112f);
        Unit d = spawnDagger(14, 128f);
        Vars.state.teams.updateTeamStats();
        b.remove();
        Vars.state.teams.updateTeamStats();
        return capture("four_remove_b", new Unit[]{a, c, d});
    }

    static Scenario capture(String name, Unit[] live) {
        var cache = Team.sharded.data().unitCache(UnitTypes.dagger);
        if (cache == null || cache.size != live.length) {
            System.err.println("ParUbindReinsert158: " + name + " cache size "
                + (cache == null ? "null" : cache.size) + " expected " + live.length);
            System.exit(3);
        }
        Scenario s = new Scenario();
        s.cacheFlags = cacheFlags(cache);
        s.groupsFlags = groupsDaggerFlags();
        s.idSortedFlags = idSortedFlags();
        s.differsFromIdSort = !s.cacheFlags.equals(s.idSortedFlags);
        s.text = runBinds();
        s.unitCount = cache.size;
        return s;
    }

    static String runBinds() {
        LExecutor exec = newExecutor(PROGRAM);
        for (int tick = 0; tick < TICKS; tick++) {
            exec.runOnce();
        }
        return exec.textBuffer.toString();
    }

    static LExecutor newExecutor(String program) {
        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(program, false));
        if (!exec.initialized()) {
            System.err.println("ParUbindReinsert158: executor did not initialize");
            System.exit(4);
        }
        exec.team = Team.sharded;
        return exec;
    }

    static Unit spawnDagger(int flag, float x) {
        Unit unit = UnitTypes.dagger.create(Team.sharded);
        unit.set(x, 80f);
        unit.flag(flag);
        unit.add();
        return unit;
    }

    static void clearUnits() {
        List<Unit> copy = new ArrayList<>();
        Groups.unit.each(copy::add);
        for (Unit u : copy) {
            if (u.isAdded()) u.remove();
        }
        Groups.unit.clear();
        Vars.state.teams.updateTeamStats();
    }

    static List<Long> cacheFlags(arc.struct.Seq<Unit> cache) {
        List<Long> flags = new ArrayList<>();
        if (cache == null) return flags;
        for (int i = 0; i < cache.size; i++) {
            flags.add((long) cache.get(i).flag());
        }
        return flags;
    }

    static List<Long> groupsDaggerFlags() {
        List<Long> flags = new ArrayList<>();
        Groups.unit.each(u -> {
            if (u.type == UnitTypes.dagger) flags.add((long) u.flag());
        });
        return flags;
    }

    static List<Long> idSortedFlags() {
        List<Unit> units = new ArrayList<>();
        Groups.unit.each(u -> {
            if (u.type == UnitTypes.dagger) units.add(u);
        });
        units.sort(Comparator.comparingInt(u -> u.id));
        List<Long> flags = new ArrayList<>();
        for (Unit u : units) flags.add((long) u.flag());
        return flags;
    }

    static void boot() {
        Vars.headless = true;
        Vars.platform = new mindustry.core.Platform(){};
        Vars.net = new mindustry.net.Net(Vars.platform.getNet());
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Groups.init();
        Vars.state = new GameState();
        Vars.state.rules.disableUnitCap = true;
        Vars.state.rules.logicUnitControl = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();
        Core.files = new SdlFiles();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();
        Vars.state.teams.updateTeamStats();
    }

    static String scenarioJson(String name, Scenario s) {
        StringBuilder sb = new StringBuilder("    ");
        sb.append(jsonString(name)).append(": {");
        sb.append("\"cache_flags\": ").append(s.cacheFlags);
        sb.append(", \"groups_dagger_flags\": ").append(s.groupsFlags);
        sb.append(", \"id_sorted_flags\": ").append(s.idSortedFlags);
        sb.append(", \"differs_from_id_sort\": ").append(s.differsFromIdSort);
        sb.append(", \"unit_count\": ").append(s.unitCount);
        sb.append(", \"text\": ").append(jsonString(s.text));
        if (s.payloadPath != null) {
            sb.append(", \"payload_path\": ").append(jsonString(s.payloadPath));
        }
        sb.append("}");
        return sb.toString();
    }

    static String cursorJson(CursorScenario s) {
        StringBuilder sb = new StringBuilder("    ");
        sb.append(jsonString("cursor_len")).append(": {");
        sb.append("\"cache_flags\": ").append(s.cacheFlags);
        sb.append(", \"groups_dagger_flags\": ").append(s.groupsFlags);
        sb.append(", \"id_sorted_flags\": ").append(s.idSortedFlags);
        sb.append(", \"differs_from_id_sort\": ").append(s.differsFromIdSort);
        sb.append(", \"seq_len\": ").append(s.seqLen);
        sb.append(", \"text_before\": ").append(jsonString(s.textBefore));
        sb.append(", \"text_after\": ").append(jsonString(s.textAfter));
        sb.append(", \"text\": ").append(jsonString(s.text));
        sb.append("}");
        return sb.toString();
    }

    static final class Scenario {
        List<Long> cacheFlags;
        List<Long> groupsFlags;
        List<Long> idSortedFlags;
        boolean differsFromIdSort;
        int unitCount;
        String text;
        String payloadPath;
    }

    static final class CursorScenario {
        List<Long> cacheFlags;
        List<Long> groupsFlags;
        List<Long> idSortedFlags;
        boolean differsFromIdSort;
        int seqLen;
        String textBefore;
        String textAfter;
        String text;
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParUbindReinsert158.class.getResourceAsStream("/version.properties")) {
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
