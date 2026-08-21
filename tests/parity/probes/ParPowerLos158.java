import java.io.InputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Properties;

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

/**
 * P1-12 differential probe: PowerNode autolink line-of-sight on desktop.jar 158.1.
 *
 * Headless 16x16 world, no game loop. Four geometries:
 * <ol>
 *   <li>clear LOS — PowerNode (2,2) then battery (6,2): {@code placed()} autolinks</li>
 *   <li>blocked — node (2,8), plastanium wall (4,8), battery (6,8): no laser</li>
 *   <li>restored — remove the wall, re-place the battery so {@code placed()} runs again</li>
 *   <li>team mismatch — sharded node (2,12) and crux battery (6,12): no laser</li>
 * </ol>
 *
 * Version gate: refuses to run unless classpath version.properties is official 158.1.
 */
public final class ParPowerLos158 {
    static final int WORLD = 16;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParPowerLos158: refusing to run: classpath version.properties reports '" + version
                + "', expected official 158.1");
            System.exit(2);
        }

        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
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

        Tile clearNode = Vars.world.tiles.get(2, 2);
        Tile clearBat = Vars.world.tiles.get(6, 2);
        clearNode.setBlock(Blocks.powerNode, Team.sharded, 0);
        clearBat.setBlock(Blocks.battery, Team.sharded, 0);

        Tile blockedNode = Vars.world.tiles.get(2, 8);
        Tile wall = Vars.world.tiles.get(4, 8);
        Tile blockedBat = Vars.world.tiles.get(6, 8);
        blockedNode.setBlock(Blocks.powerNode, Team.sharded, 0);
        wall.setBlock(Blocks.plastaniumWall, Team.sharded, 0);
        blockedBat.setBlock(Blocks.battery, Team.sharded, 0);

        StringBuilder blocked = pair(blockedNode.build, blockedBat.build);

        wall.setBlock(Blocks.air);
        blockedBat.setBlock(Blocks.air);
        blockedBat.setBlock(Blocks.battery, Team.sharded, 0);

        Tile teamNode = Vars.world.tiles.get(2, 12);
        Tile teamBat = Vars.world.tiles.get(6, 12);
        teamNode.setBlock(Blocks.powerNode, Team.sharded, 0);
        teamBat.setBlock(Blocks.battery, Team.crux, 0);

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("power-los")).append(",\n");
        json.append("  \"tick\": 0,\n");
        json.append("  \"clear\": ").append(pair(clearNode.build, clearBat.build)).append(",\n");
        json.append("  \"blocked\": ").append(blocked).append(",\n");
        json.append("  \"restored\": ").append(pair(blockedNode.build, blockedBat.build)).append(",\n");
        json.append("  \"team_mismatch\": ").append(pair(teamNode.build, teamBat.build)).append("\n");
        json.append("}\n");
        System.out.print(json);
    }

    static StringBuilder pair(Building node, Building battery) {
        StringBuilder out = new StringBuilder();
        out.append("{\n");
        out.append("    \"node_links\": ").append(linksJson(node)).append(",\n");
        out.append("    \"battery_links\": ").append(linksJson(battery)).append("\n");
        out.append("  }");
        return out;
    }

    static StringBuilder linksJson(Building building) {
        List<Integer> links = new ArrayList<>();
        if (building != null && building.power != null) {
            for (int i = 0; i < building.power.links.size; i++) {
                links.add(building.power.links.get(i));
            }
        }
        Collections.sort(links);
        StringBuilder out = new StringBuilder("[");
        for (int i = 0; i < links.size(); i++) {
            if (i > 0) out.append(", ");
            out.append(links.get(i));
        }
        return out.append("]");
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParPowerLos158.class.getResourceAsStream("/version.properties")) {
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
