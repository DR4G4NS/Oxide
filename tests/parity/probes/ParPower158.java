import java.io.InputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Properties;

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
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.world.Tile;
import mindustry.world.Tiles;
import mindustry.world.blocks.power.PowerGraph;

/**
 * P0-00 differential probe: power link/unlink lifecycle on desktop.jar 158.1.
 *
 * Builds a minimal headless world (no game loop, no client): a 16x16 tile
 * grid, a PowerSource at (2,2) and a PowerNode at (4,2), both team sharded.
 * The node is linked to the source through the official PowerNode
 * {@code Point2[]} config consumer (the same path a server uses for
 * {@code configLoaded}), the merged power graph is updated for 600 game ticks
 * (Time.delta = 1/60 each), then the node is unlinked with an empty
 * {@code Point2[]} config and the graph update is re-run.
 *
 * The probe emits the normalized topology and power state before and after
 * the unlink. The Rust side replays the same scenario with its power domain
 * (DynamicWorld + apply_configuration) and compares the link topology and
 * graph split field by field; the Java-only power/status numbers are
 * informational (the Rust server does not simulate PowerGraph status).
 *
 * Version gate: refuses to run unless the classpath version.properties
 * reports the official 158.1 build.
 */
public final class ParPower158 {
    /** Game ticks the linked graph is updated before the unlink. */
    static final int TICKS_LINKED = 600;
    static final int WORLD = 16;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParPower158: refusing to run: classpath version.properties reports '" + version
                + "', expected official 158.1");
            System.exit(2);
        }

        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        // PowerGraph.checkAdd registers the graph with the powerGraph entity
        // group (Groups.init creates the generated EntityGroups; it is pure
        // data and needs no Arc app).
        Groups.init();
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
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();

        Tile srcTile = Vars.world.tiles.get(2, 2);
        Tile nodeTile = Vars.world.tiles.get(4, 2);
        srcTile.setBlock(Blocks.powerSource, Team.sharded, 0);
        nodeTile.setBlock(Blocks.powerNode, Team.sharded, 0);
        Building src = srcTile.build;
        Building node = nodeTile.build;
        if (src == null || node == null || src.power == null || node.power == null) {
            System.err.println("ParPower158: buildings or power modules missing");
            System.exit(3);
        }

        // Link: dispatch the official PowerNode Point2[] config (relative
        // offsets) directly, exactly like configLoaded on a server.
        node.block.configurations.get(Point2[].class).get(node, new Point2[]{ new Point2(-2, 0) });
        if (node.power.links.size != 1 || src.power.links.size != 1) {
            System.err.println("ParPower158: link failed: node links=" + node.power.links
                + " source links=" + src.power.links);
            System.exit(4);
        }

        for (int tick = 1; tick <= TICKS_LINKED; tick++) {
            Time.delta = 1f / 60f;
            Time.time += Time.delta;
            src.power.graph.update();
        }

        StringBuilder linked = snapshot(src, node, true);

        // Unlink: official empty Point2[] config.
        node.block.configurations.get(Point2[].class).get(node, new Point2[]{});
        Time.delta = 1f / 60f;
        Time.time += Time.delta;
        src.power.graph.update();
        node.power.graph.update();

        if (node.power.links.size != 0 || src.power.links.size != 0) {
            System.err.println("ParPower158: unlink failed: node links=" + node.power.links
                + " source links=" + src.power.links);
            System.exit(5);
        }
        StringBuilder unlinked = snapshot(src, node, false);

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("power-unlink")).append(",\n");
        json.append("  \"tick\": ").append(TICKS_LINKED + 1).append(",\n");
        json.append("  \"source_pos\": ").append(srcTile.pos()).append(",\n");
        json.append("  \"node_pos\": ").append(nodeTile.pos()).append(",\n");
        json.append("  \"linked\": ").append(linked).append(",\n");
        json.append("  \"unlinked\": ").append(unlinked).append("\n");
        json.append("}\n");
        System.out.print(json);
    }

    /** Normalized snapshot of both buildings and their graphs. */
    static StringBuilder snapshot(Building src, Building node, boolean sameGraph) {
        PowerGraph sg = src.power.graph;
        PowerGraph ng = node.power.graph;
        StringBuilder out = new StringBuilder();
        out.append("{\n");
        out.append("    \"source_links\": ").append(linksJson(src)).append(",\n");
        out.append("    \"node_links\": ").append(linksJson(node)).append(",\n");
        out.append("    \"same_graph\": ").append(sg == ng).append(",\n");
        out.append("    \"source_graph_buildings\": ").append(sg.all.size).append(",\n");
        out.append("    \"node_graph_buildings\": ").append(ng.all.size).append(",\n");
        out.append("    \"graph_produced\": ").append(sg.getPowerProduced()).append(",\n");
        out.append("    \"graph_needed\": ").append(sg.getPowerNeeded()).append(",\n");
        out.append("    \"source_power_status\": ").append(src.power.status).append(",\n");
        out.append("    \"node_power_status\": ").append(node.power.status).append("\n");
        out.append("  }");
        return out;
    }

    /** Sorted packed position list of a building's power links. */
    static StringBuilder linksJson(Building building) {
        List<Integer> links = new ArrayList<>();
        for (int i = 0; i < building.power.links.size; i++) {
            links.add(building.power.links.get(i));
        }
        Collections.sort(links);
        StringBuilder out = new StringBuilder("[");
        for (int i = 0; i < links.size(); i++) {
            if (i > 0) out.append(", ");
            out.append(links.get(i));
        }
        return out.append("]");
    }

    /** Reads version.properties from the classpath (the desktop.jar root). */
    static String classpathVersion() throws Exception {
        try (InputStream in = ParPower158.class.getResourceAsStream("/version.properties")) {
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
