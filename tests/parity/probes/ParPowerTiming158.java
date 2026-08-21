import java.io.InputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.geom.Point2;
import arc.util.Time;
import mindustry.Vars;
import mindustry.content.Blocks;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.Building;
import mindustry.gen.Groups;
import mindustry.input.InputHandler;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.world.Tile;
import mindustry.world.Tiles;
import mindustry.world.blocks.power.BeamNode;
import mindustry.world.blocks.power.PowerGraph;

/**
 * P1-B3 differential probe: same-tick ordering for power distribution,
 * PowerDiode transfer, PowerNode unlink/split, BeamNode rescan and payload
 * pickup/drop topology on desktop.jar 158.1.
 *
 * Official {@code Logic.updateEntities} order each tick:
 *   {@code Groups.powerGraph.update()} then {@code Groups.build.update()}
 * (PowerDiode.updateTile, BeamNode.updateDirections when tileChanges).
 *
 * Each scenario traces n_minus_1 / end_n / end_n_plus_1 / end_n_plus_2.
 */
public final class ParPowerTiming158 {
    static final int WORLD = 16;
    static final float DELTA = 1f / 60f;
    static final int WARMUP = 3;
    static final int DIODE_WARMUP = 0;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParPowerTiming158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

        bootMinimal();
        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": \"158.1\",\n");
        json.append("  \"probe_name\": \"power-timing\",\n");
        json.append("  \"tick\": 0,\n");
        bootPayload();
        appendTrace(json, "graph_then_diode", traceGraphThenDiode(), false);
        appendTrace(json, "diode_then_distribution", traceDiodeThenDistribution(), false);
        appendTrace(json, "unlink_split", traceUnlinkSplit(), false);
        appendTrace(json, "beam_insulated_wall", traceBeamInsulatedWall(), false);
        appendTrace(json, "payload_topology", tracePayloadTopology(), true);
        json.append("\n}\n");
        System.out.print(json);
    }

    /** A: graph update runs before diode transfer on the same tick. */
    static Trace traceGraphThenDiode() {
        resetWorld();
        Building back = place(8, 10, Blocks.battery, Team.sharded);
        Building diode = place(9, 10, Blocks.diode, Team.sharded);
        Building front = place(10, 10, Blocks.battery, Team.sharded);
        // 2x2 consumer east of the front battery; stays off the diode front tile.
        Building consumer = place(11, 10, Blocks.waterExtractor, Team.sharded);
        normalizePower(back, diode, front, consumer);
        back.power.status = 0.9f;
        front.power.status = 0f;
        refreshGraphs(back, front, consumer);
        for (int i = 0; i < DIODE_WARMUP; i++) gameTick();

        Trace t = new Trace();
        t.nMinus1 = snapSplit(back, front, consumer);

        gameTick();
        t.endN = snapSplit(back, front, consumer);

        gameTick();
        t.endNPlus1 = snapSplit(back, front, consumer);

        gameTick();
        t.endNPlus2 = snapSplit(back, front, consumer);
        return t;
    }

    /** B: diode transfer at N is visible to distribution on N+1. */
    static Trace traceDiodeThenDistribution() {
        resetWorld();
        Building back = place(8, 10, Blocks.battery, Team.sharded);
        Building diode = place(9, 10, Blocks.diode, Team.sharded);
        Building front = place(10, 10, Blocks.battery, Team.sharded);
        Building consumer = place(11, 10, Blocks.waterExtractor, Team.sharded);
        normalizePower(back, diode, front, consumer);
        back.power.status = 0.9f;
        front.power.status = 0f;
        refreshGraphs(back, front, consumer);
        for (int i = 0; i < DIODE_WARMUP; i++) gameTick();

        Trace t = new Trace();
        t.nMinus1 = snapSplit(back, front, consumer);
        gameTick();
        t.endN = snapSplit(back, front, consumer);
        gameTick();
        t.endNPlus1 = snapSplit(back, front, consumer);
        gameTick();
        t.endNPlus2 = snapSplit(back, front, consumer);
        return t;
    }

    /** C: PowerNode unlink splits graphs immediately, before the next tick. */
    static Trace traceUnlinkSplit() {
        resetWorld();
        Building source = place(2, 2, Blocks.powerSource, Team.sharded);
        Building node = place(4, 2, Blocks.powerNode, Team.sharded);
        node.block.configurations.get(Point2[].class).get(node, new Point2[]{ new Point2(-2, 0) });
        for (int i = 0; i < WARMUP; i++) gameTick();

        Trace t = new Trace();
        t.nMinus1 = snapLink(source, node);

        node.block.configurations.get(Point2[].class).get(node, new Point2[]{});
        gameTick();
        t.endN = snapLink(source, node);

        gameTick();
        t.endNPlus1 = snapLink(source, node);

        gameTick();
        t.endNPlus2 = snapLink(source, node);
        return t;
    }

    /** D: insulated wall insert/remove vs BeamNode rescan (build pass). */
    static Trace traceBeamInsulatedWall() {
        resetWorld();
        Building solar = place(5, 10, Blocks.powerSource, Team.sharded);
        Building beam = place(6, 10, Blocks.beamNode, Team.sharded);
        Building laser = place(12, 10, Blocks.laserDrill, Team.sharded);
        normalizePower(solar, beam, laser);
        for (int i = 0; i < WARMUP; i++) gameTick();

        Trace t = new Trace();
        t.nMinus1 = snapBeam(solar, beam, laser, null);

        Tile wallTile = tile(9, 10);
        wallTile.setBlock(Blocks.plastaniumWall, Team.sharded, 0);
        gameTick();
        t.endN = snapBeam(solar, beam, laser, wallTile.build);

        wallTile.setBlock(Blocks.air);
        gameTick();
        t.endNPlus1 = snapBeam(solar, beam, laser, null);

        gameTick();
        t.endNPlus2 = snapBeam(solar, beam, laser, null);
        return t;
    }

    /** E: payload pickup clears links; drop rebuilds them next tick. */
    static Trace tracePayloadTopology() {
        resetWorld();
        Building node = place(5, 5, Blocks.powerNode, Team.sharded);
        Building battery = place(8, 5, Blocks.battery, Team.sharded);
        normalizePower(node, battery);
        node.block.configurations.get(Point2[].class).get(node, new Point2[]{ new Point2(3, 0) });
        for (int i = 0; i < WARMUP; i++) gameTick();

        mindustry.gen.Player player = mindustry.gen.Player.create();
        player.team(Team.sharded);
        mindustry.gen.Unit mega = mindustry.content.UnitTypes.mega.create(Team.sharded);
        mega.set(40f, 40f);
        mega.add();
        player.unit(mega);
        Vars.state.teams.updateTeamStats();

        Trace t = new Trace();
        t.nMinus1 = snapLink(node, battery);

        InputHandler.requestBuildPayload(player, battery);
        gameTick();
        t.endN = snapLink(node, findBuilding(8, 5));

        mega.set(60f, 44f);
        InputHandler.requestDropPayload(player, 60f, 44f);
        gameTick();
        Building dropped = findBuilding(7, 5);
        t.endNPlus1 = snapLink(node, dropped);

        gameTick();
        t.endNPlus2 = snapLink(node, dropped);
        return t;
    }

    // --- Tick helpers ----------------------------------------------------------

    static void gameTick() {
        Time.delta = DELTA;
        Time.time += DELTA;
        Groups.powerGraph.update();
        Groups.build.update();
    }

    static void normalizePower(Building... builds) {
        for (Building build : builds) {
            if (build != null) {
                build.updatePowerGraph();
                build.placed();
            }
        }
    }

    static void refreshGraphs(Building... builds) {
        for (Building build : builds) {
            if (build != null && build.power != null) {
                build.power.graph.update();
            }
        }
    }

    static void normalizeGraphs(Building... builds) {
        for (Building build : builds) {
            if (build != null) {
                build.updatePowerGraph();
            }
        }
    }

    // --- Snapshots -------------------------------------------------------------

    static Snapshot snapSplit(Building back, Building front, Building consumer) {
        Snapshot s = new Snapshot();
        PowerGraph bg = back.power.graph;
        PowerGraph fg = front.power.graph;
        s.back_stored = bg.getBatteryStored();
        s.front_stored = fg.getBatteryStored();
        s.back_capacity = bg.getTotalBatteryCapacity();
        s.front_capacity = fg.getTotalBatteryCapacity();
        s.total_stored = s.back_stored + s.front_stored;
        s.total_capacity = s.back_capacity + s.front_capacity;
        s.back_members = members(bg);
        s.front_members = members(fg);
        s.same_graph = bg == fg;
        s.consumer_eff = consumer.power.status;
        return s;
    }

    static Snapshot snapLink(Building a, Building b) {
        Snapshot s = new Snapshot();
        if (a != null && a.power != null) {
            PowerGraph g = a.power.graph;
            s.total_stored = g.getBatteryStored();
            s.total_capacity = g.getTotalBatteryCapacity();
            s.back_members = members(g);
            s.node_links = linksOf(a);
        }
        if (b != null && b.power != null) {
            s.battery_links = linksOf(b);
        }
        s.same_graph = a != null && b != null && a.power != null && b.power != null
            && a.power.graph == b.power.graph;
        s.beam_dest = -1;
        return s;
    }

    static Snapshot snapBeam(Building solar, Building beam, Building laser, Building wall) {
        Snapshot s = new Snapshot();
        s.beam_dest = beamDestEast(beam);
        s.same_graph = solar.power.graph == laser.power.graph;
        s.consumer_eff = laser.power.status;
        s.total_stored = laser.power.graph.getBatteryStored();
        s.total_capacity = laser.power.graph.getTotalBatteryCapacity();
        s.back_members = members(laser.power.graph);
        s.node_links = linksOf(beam);
        s.wall_present = wall != null && !wall.dead;
        return s;
    }

    static int beamDestEast(Building beam) {
        if (beam == null || !(beam instanceof BeamNode.BeamNodeBuild bb)) {
            return -1;
        }
        Building link = bb.links[0];
        return link != null ? link.pos() : -1;
    }

    static List<Integer> members(PowerGraph graph) {
        List<Integer> out = new ArrayList<>();
        for (Building b : graph.all) {
            out.add(b.pos());
        }
        Collections.sort(out);
        return out;
    }

    static List<Integer> linksOf(Building building) {
        List<Integer> out = new ArrayList<>();
        if (building != null && building.power != null) {
            for (int i = 0; i < building.power.links.size; i++) {
                out.add(building.power.links.get(i));
            }
        }
        Collections.sort(out);
        return out;
    }

    // --- World boot / placement ------------------------------------------------

    static void bootMinimal() {
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Groups.init();
        stubNet();
        Vars.state = new GameState();
        Vars.state.rules.disableUnitCap = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
    }

    static void bootPayload() {
        bootMinimal();
        Vars.platform = new mindustry.core.Platform(){};
        Core.files = new SdlFiles();
        Core.audio = new arc.audio.Audio(true);
        Core.app = new arc.Application() {
            public arc.struct.Seq<arc.ApplicationListener> getListeners() {
                return new arc.struct.Seq<>();
            }
            public arc.Application.ApplicationType getType() {
                return arc.Application.ApplicationType.headless;
            }
            public boolean isHeadless() {
                return true;
            }
            public String getClipboardText() {
                return "";
            }
            public void setClipboardText(String text) {}
            public void post(Runnable r) {
                r.run();
            }
            public void exit() {}
        };
    }

    static void stubNet() {
        Vars.net = new Net(new Net.NetProvider() {
            public void connectClient(String ip, int port, Runnable success) {}
            public void sendClient(Object object, boolean reliable) {}
            public void disconnectClient() {}
            public void discoverServers(arc.func.Cons<Host> found, Runnable done) { done.run(); }
            public void pingHost(String address, int port, arc.func.Cons<Host> valid, arc.func.Cons<Exception> failed) {}
            public void hostServer(int port) {}
            public Iterable<? extends NetConnection> getConnections() { return java.util.List.of(); }
            public void closeServer() {}
        });
    }

    static void resetWorld() {
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();
        Groups.init();
    }

    static Building place(int x, int y, mindustry.world.Block block, Team team) {
        Tile tile = tile(x, y);
        tile.setBlock(block, team, 0);
        return tile.build;
    }

    static Building findBuilding(int x, int y) {
        Tile tile = tile(x, y);
        return tile != null ? tile.build : null;
    }

    static Tile tile(int x, int y) {
        return Vars.world.tiles.get(x, y);
    }

    // --- JSON ------------------------------------------------------------------

    static void appendTrace(StringBuilder json, String name, Trace trace, boolean last) {
        json.append("  \"").append(name).append("\": {\n");
        appendPhase(json, "n_minus_1", trace.nMinus1);
        appendPhase(json, "end_n", trace.endN);
        appendPhase(json, "end_n_plus_1", trace.endNPlus1);
        appendPhase(json, "end_n_plus_2", trace.endNPlus2, true);
        json.append("  }").append(last ? "" : ",").append("\n");
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s) {
        appendPhase(json, phase, s, false);
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s, boolean lastPhase) {
        json.append("    \"").append(phase).append("\": {");
        json.append("\"back_stored\": ").append(num(s.back_stored)).append(", ");
        json.append("\"front_stored\": ").append(num(s.front_stored)).append(", ");
        json.append("\"total_stored\": ").append(num(s.total_stored)).append(", ");
        json.append("\"back_capacity\": ").append(num(s.back_capacity)).append(", ");
        json.append("\"front_capacity\": ").append(num(s.front_capacity)).append(", ");
        json.append("\"total_capacity\": ").append(num(s.total_capacity)).append(", ");
        json.append("\"consumer_eff\": ").append(num(s.consumer_eff)).append(", ");
        json.append("\"same_graph\": ").append(s.same_graph).append(", ");
        json.append("\"beam_dest\": ").append(s.beam_dest).append(", ");
        json.append("\"wall_present\": ").append(s.wall_present).append(", ");
        json.append("\"back_members\": ").append(ints(s.back_members)).append(", ");
        json.append("\"front_members\": ").append(ints(s.front_members)).append(", ");
        json.append("\"node_links\": ").append(ints(s.node_links)).append(", ");
        json.append("\"battery_links\": ").append(ints(s.battery_links));
        json.append("}").append(lastPhase ? "" : ",").append("\n");
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParPowerTiming158.class.getResourceAsStream("/version.properties")) {
            if (in == null) return "missing";
            Properties p = new Properties();
            p.load(in);
            return p.getProperty("type", "missing") + " " + p.getProperty("build", "missing");
        }
    }

    static String ints(List<Integer> values) {
        StringBuilder out = new StringBuilder("[");
        for (int i = 0; i < values.size(); i++) {
            if (i > 0) out.append(", ");
            out.append(values.get(i));
        }
        return out.append("]").toString();
    }

    static String num(float value) {
        if (Float.isInfinite(value)) return value > 0 ? "1e38" : "-1e38";
        if (Float.isNaN(value)) return "0.000000";
        return String.format(Locale.US, "%.6f", value);
    }

    static final class Trace {
        Snapshot nMinus1, endN, endNPlus1, endNPlus2;
    }

    static final class Snapshot {
        float back_stored;
        float front_stored;
        float total_stored;
        float back_capacity;
        float front_capacity;
        float total_capacity;
        float consumer_eff;
        boolean same_graph;
        int beam_dest = -1;
        boolean wall_present;
        List<Integer> back_members = List.of();
        List<Integer> front_members = List.of();
        List<Integer> node_links = List.of();
        List<Integer> battery_links = List.of();
    }
}
