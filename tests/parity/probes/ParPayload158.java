import java.io.InputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Locale;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.Mathf;
import arc.math.geom.Point2;
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
import mindustry.gen.Payloadc;
import mindustry.gen.Player;
import mindustry.gen.Unit;
import mindustry.input.InputHandler;
import mindustry.logic.GlobalVars;
import mindustry.world.Tile;
import mindustry.world.Tiles;
import mindustry.world.blocks.payloads.BuildPayload;
import mindustry.world.blocks.payloads.Payload;
import mindustry.world.blocks.payloads.PayloadBlock;
import mindustry.world.blocks.payloads.UnitPayload;

/**
 * P0-F1 differential probe: RequestBuildPayload / RequestUnitPayload /
 * RequestDropPayload on desktop.jar 158.1.
 *
 * Drives the official InputHandler RPC gates and the Call picked/payloadDropped
 * lifecycle (including dropLastPayload temporary carrier teleport). Each scenario
 * resets a minimal 80x80 headless world, executes one RPC sequence, and dumps
 * observable postconditions: carrier payloads, world membership, drop placement,
 * multiblock footprint, and neighbor power links.
 *
 * Version gate: refuses to run unless classpath version.properties is official 158.1.
 */
public final class ParPayload158 {
    static final int WORLD = 80;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParPayload158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

        boot();

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("payload-rpc")).append(",\n");
        json.append("  \"tick\": 0,\n");
        json.append("  \"world_size\": ").append(WORLD).append(",\n");
        json.append("  \"scenarios\": {\n");

        List<String> parts = new ArrayList<>();
        parts.add(scenario("build_in_range", buildInRange()));
        parts.add(scenario("build_out_of_range", buildOutOfRange()));
        parts.add(scenario("build_hidden", buildHidden()));
        parts.add(scenario("build_can_pickup_false", buildCanPickupFalse()));
        parts.add(scenario("build_enemy_team", buildEnemyTeam()));
        parts.add(scenario("build_internal_payload", buildInternalPayload()));
        parts.add(scenario("build_exact_capacity", buildExactCapacity()));
        parts.add(scenario("build_over_capacity", buildOverCapacity()));
        parts.add(scenario("unit_ai_grounded", unitAiGrounded()));
        parts.add(scenario("unit_player_controller", unitPlayerController()));
        parts.add(scenario("unit_flying", unitFlying()));
        parts.add(scenario("unit_out_of_range", unitOutOfRange()));
        parts.add(scenario("drop_within_four", dropWithinFour()));
        parts.add(scenario("drop_clamp_far", dropClampFar()));
        parts.add(scenario("drop_blocked", dropBlocked()));
        parts.add(scenario("drop_unit_payload", dropUnitPayload()));
        parts.add(scenario("drop_build_payload", dropBuildPayload()));
        parts.add(scenario("power_pickup_drop", powerPickupDrop()));
        parts.add(scenario("race_two_build", raceTwoBuild()));
        parts.add(scenario("drop_dead_player", dropDeadPlayer()));

        json.append(String.join(",\n", parts));
        json.append("\n  }\n}\n");
        System.out.print(json);
    }

    // --- Build pickup scenarios ------------------------------------------------

    static Case buildInRange() {
        resetWorld();
        int origin = tilePos(5, 5);
        Building wall = place(5, 5, Blocks.titaniumWall, Team.sharded);
        Carrier c = carrierAt(40f, 40f, 10);
        boolean ok = requestBuild(c.player, wall);
        return dump(c, origin, -1, ok, -1f, -1f, -1, -1);
    }

    static Case buildOutOfRange() {
        resetWorld();
        int origin = tilePos(60, 60);
        Building wall = place(60, 60, Blocks.titaniumWall, Team.sharded);
        Carrier c = carrierAt(40f, 40f, 10);
        boolean ok = requestBuild(c.player, wall);
        return dump(c, origin, -1, ok, -1f, -1f, -1, -1);
    }

    static Case buildHidden() {
        resetWorld();
        int origin = tilePos(5, 5);
        tile(5, 5).setBlock(Blocks.spawn, Team.sharded, 0);
        Building hidden = tileAtPos(origin).build;
        Carrier c = carrierAt(40f, 40f, 10);
        boolean ok = hidden != null && requestBuild(c.player, hidden);
        return dump(c, origin, -1, ok, -1f, -1f, -1, -1);
    }

    static Case buildCanPickupFalse() {
        resetWorld();
        int origin = tilePos(5, 5);
        Building core = place(5, 5, Blocks.coreShard, Team.sharded);
        Carrier c = carrierAt(40f, 40f, 10);
        boolean ok = requestBuild(c.player, core);
        return dump(c, origin, -1, ok, -1f, -1f, -1, -1);
    }

    static Case buildEnemyTeam() {
        resetWorld();
        int origin = tilePos(5, 5);
        Building wall = place(5, 5, Blocks.titaniumWall, Team.crux);
        Carrier c = carrierAt(40f, 40f, 10);
        boolean ok = requestBuild(c.player, wall);
        return dump(c, origin, -1, ok, -1f, -1f, -1, -1);
    }

    static Case buildInternalPayload() {
        resetWorld();
        int origin = tilePos(5, 5);
        Tile loaderTile = tile(5, 5);
        loaderTile.setBlock(Blocks.payloadLoader, Team.sharded, 0);
        PayloadBlock.PayloadBlockBuild loader =
            (PayloadBlock.PayloadBlockBuild) loaderTile.build;
        loader.payload = new BuildPayload(Blocks.titaniumWall, Team.sharded);
        Carrier c = carrierAt(40f, 40f, 10);
        boolean ok = requestBuild(c.player, loaderTile.build);
        Case out = dump(c, origin, -1, ok, -1f, -1f, -1, -1);
        out.loaderStillExists = loaderTile.build != null && !loaderTile.build.dead;
        return out;
    }

    static Case buildExactCapacity() {
        resetWorld();
        int origin = tilePos(5, 5);
        Building large = place(5, 5, Blocks.titaniumWallLarge, Team.sharded);
        Carrier c = carrierAt(40f, 40f, 10);
        boolean ok = requestBuild(c.player, large);
        return dump(c, origin, -1, ok, -1f, -1f, -1, -1);
    }

    static Case buildOverCapacity() {
        resetWorld();
        int largeOrigin = tilePos(5, 5);
        Building large = place(5, 5, Blocks.titaniumWallLarge, Team.sharded);
        Carrier c = carrierAt(40f, 40f, 10);
        requestBuild(c.player, large);
        int extraOrigin = tilePos(8, 5);
        Building extra = place(8, 5, Blocks.titaniumWall, Team.sharded);
        boolean ok = requestBuild(c.player, extra);
        Case out = dump(c, extraOrigin, -1, ok, -1f, -1f, -1, -1);
        out.extraOriginExists = tileAtPos(extraOrigin).build != null;
        return out;
    }

    // --- Unit pickup scenarios -------------------------------------------------

    static Case unitAiGrounded() {
        resetWorld();
        Carrier c = carrierAt(40f, 40f, 10);
        Unit dagger = spawnDagger(20, 40f, 40f, 0f, true);
        int targetId = dagger.id;
        boolean ok = requestUnit(c.player, dagger);
        return dump(c, -1, targetId, ok, -1f, -1f, -1, -1);
    }

    static Case unitPlayerController() {
        resetWorld();
        Carrier c = carrierAt(40f, 40f, 10);
        Unit dagger = spawnDagger(20, 40f, 40f, 0f, true);
        Player other = Player.create();
        other.team(Team.sharded);
        other.unit(dagger);
        boolean ok = requestUnit(c.player, dagger);
        return dump(c, -1, dagger.id, ok, -1f, -1f, -1, -1);
    }

    static Case unitFlying() {
        resetWorld();
        Carrier c = carrierAt(40f, 40f, 10);
        Unit flare = UnitTypes.flare.create(Team.sharded);
        flare.set(40f, 40f);
        flare.add();
        boolean ok = requestUnit(c.player, flare);
        return dump(c, -1, flare.id, ok, -1f, -1f, -1, -1);
    }

    static Case unitOutOfRange() {
        resetWorld();
        Carrier c = carrierAt(40f, 40f, 10);
        Unit dagger = spawnDagger(21, 120f, 40f, 0f, true);
        boolean ok = requestUnit(c.player, dagger);
        return dump(c, -1, dagger.id, ok, -1f, -1f, -1, -1);
    }

    // --- Drop scenarios ----------------------------------------------------------

    static Case dropWithinFour() {
        resetWorld();
        Carrier c = carrierAt(80f, 80f, 10);
        giveBuild(c.unit, Blocks.titaniumWall, Team.sharded);
        boolean ok = requestDrop(c.player, 88f, 80f);
        Case out = dump(c, -1, -1, ok, 88f, 80f, 11, 10);
        out.footprint = footprintOf(Blocks.titaniumWall, 11, 10);
        return out;
    }

    static Case dropClampFar() {
        resetWorld();
        Carrier c = carrierAt(80f, 80f, 10);
        giveBuild(c.unit, Blocks.titaniumWall, Team.sharded);
        boolean ok = requestDrop(c.player, 240f, 80f);
        Case out = dump(c, -1, -1, ok, 112f, 80f, 14, 10);
        out.footprint = footprintOf(Blocks.titaniumWall, 14, 10);
        return out;
    }

    static Case dropBlocked() {
        resetWorld();
        place(10, 10, Blocks.titaniumWall, Team.sharded);
        Carrier c = carrierAt(80f, 80f, 12);
        giveBuild(c.unit, Blocks.titaniumWall, Team.sharded);
        boolean ok = requestDrop(c.player, 80f, 80f);
        return dump(c, -1, -1, ok, -1f, -1f, -1, -1);
    }

    static Case dropUnitPayload() {
        resetWorld();
        Carrier c = carrierAt(80f, 80f, 10);
        Unit dagger = spawnDagger(30, 0f, 0f, 0f, true);
        ((Payloadc) c.unit).pickup(dagger);
        Mathf.rand.setSeed(1L);
        boolean ok = requestDrop(c.player, 88f, 80f);
        Case out = dump(c, -1, -1, ok, -1f, -1f, -1, -1);
        Unit dropped = findDroppedDagger();
        if (dropped != null) {
            out.dropX = dropped.x();
            out.dropY = dropped.y();
            out.droppedUnitExists = true;
        }
        return out;
    }

    static Unit findDroppedDagger() {
        for (Unit unit : Groups.unit) {
            if (unit.type() == UnitTypes.dagger && unit.id != 30) {
                return unit;
            }
        }
        return null;
    }

    static Case dropBuildPayload() {
        resetWorld();
        Carrier c = carrierAt(80f, 80f, 10);
        giveBuild(c.unit, Blocks.titaniumWall, Team.sharded);
        boolean ok = requestDrop(c.player, 80f, 80f);
        Case out = dump(c, -1, -1, ok, 80f, 80f, 10, 10);
        out.footprint = footprintOf(Blocks.titaniumWall, 10, 10);
        return out;
    }

    static Case powerPickupDrop() {
        resetWorld();
        Building node = place(5, 5, Blocks.powerNode, Team.sharded);
        node.placed();
        Building battery = place(8, 5, Blocks.battery, Team.sharded);
        battery.placed();
        normalizePower(node, battery);
        Carrier c = carrierAt(40f, 40f, 10);
        int batteryOrigin = tilePos(8, 5);
        boolean picked = requestBuild(c.player, battery);
        List<int[]> linksAfterPickup = linkTiles(node);
        c.unit.set(80f, 80f);
        boolean dropped = requestDrop(c.player, 80f, 80f);
        Case out = dump(c, -1, -1, picked && dropped, 80f, 80f, 10, 10);
        out.nodeLinksAfterPickup = linksAfterPickup;
        out.nodeLinksAfterDrop = linkTiles(node);
        out.emitNodeLinks = true;
        out.carrierPayloadCount = payloadCount(c.unit);
        return out;
    }

    static Case raceTwoBuild() {
        resetWorld();
        place(5, 5, Blocks.titaniumWall, Team.sharded);
        int origin = tilePos(5, 5);
        Carrier a = carrierAt(40f, 40f, 10);
        Carrier b = carrierAt(40f, 40f, 11);
        Building firstRef = tileAtPos(origin).build;
        boolean first = firstRef != null && requestBuild(a.player, firstRef);
        Building secondRef = tileAtPos(origin).build;
        boolean second = secondRef != null && requestBuild(b.player, secondRef);
        Case out = dump(a, origin, -1, first, -1f, -1f, -1, -1);
        out.secondSuccess = second;
        out.secondPayloadCount = payloadCount(b.unit);
        out.emitRace = true;
        return out;
    }

    static Case dropDeadPlayer() {
        resetWorld();
        Carrier c = carrierAt(80f, 80f, 10);
        giveBuild(c.unit, Blocks.titaniumWall, Team.sharded);
        c.unit.kill();
        boolean ok = requestDrop(c.player, 80f, 80f);
        return dump(c, -1, -1, ok, -1f, -1f, -1, -1);
    }

    // --- RPC helpers (InputHandler entry points) ---------------------------------

    static boolean requestBuild(Player player, Building build) {
        int before = payloadCount(player.unit());
        InputHandler.requestBuildPayload(player, build);
        return payloadCount(player.unit()) > before;
    }

    static boolean requestUnit(Player player, Unit target) {
        int before = payloadCount(player.unit());
        InputHandler.requestUnitPayload(player, target);
        return payloadCount(player.unit()) > before;
    }

    static boolean requestDrop(Player player, float x, float y) {
        int before = payloadCount(player.unit());
        InputHandler.requestDropPayload(player, x, y);
        return payloadCount(player.unit()) < before;
    }

    // --- World / entity setup ----------------------------------------------------

    static void boot() {
        Vars.headless = true;
        Vars.platform = new mindustry.core.Platform(){};
        Core.files = new SdlFiles();
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
        Core.settings = new arc.Settings();
        Core.audio = new arc.audio.Audio(true);
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Groups.init();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();
        Vars.net = new mindustry.net.Net(Vars.platform.getNet());
        Vars.state = new GameState();
        Vars.state.rules.disableUnitCap = true;
        Vars.world = new World();
    }

    static void resetWorld() {
        clearUnits();
        clearBuildings();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();
        Vars.state.teams.updateTeamStats();
    }

    static void clearBuildings() {
        List<Building> copy = new ArrayList<>();
        Groups.build.each(copy::add);
        for (Building build : copy) {
            if (build != null && !build.dead) {
                build.remove();
            }
        }
        Groups.build.clear();
    }

    static void clearUnits() {
        List<Unit> copy = new ArrayList<>();
        Groups.unit.each(copy::add);
        for (Unit u : copy) {
            if (u.isAdded()) u.remove();
        }
        Groups.unit.clear();
    }

    static Building place(int x, int y, mindustry.world.Block block, Team team) {
        Tile tile = tile(x, y);
        tile.setBlock(block, team, 0);
        return tile.build;
    }

    static Tile tileAtPos(int pos) {
        Point2 p = Point2.unpack(pos);
        return tile(p.x, p.y);
    }

    static Tile tile(int x, int y) {
        return Vars.world.tiles.get(x, y);
    }

    static int tilePos(int x, int y) {
        return tile(x, y).pos();
    }

    static Carrier carrierAt(float x, float y, int idHint) {
        Unit mega = UnitTypes.mega.create(Team.sharded);
        mega.set(x, y);
        mega.add();
        Player player = Player.create();
        player.team(Team.sharded);
        player.unit(mega);
        return new Carrier(player, mega);
    }

    static Unit spawnDagger(int flag, float x, float y, float elevation, boolean ai) {
        Unit dagger = UnitTypes.dagger.create(Team.sharded);
        dagger.set(x, y);
        dagger.elevation = elevation;
        dagger.flag(flag);
        dagger.add();
        if (!ai) {
            Player p = Player.create();
            p.team(Team.sharded);
            p.unit(dagger);
        }
        return dagger;
    }

    static void giveBuild(Unit carrier, mindustry.world.Block block, Team team) {
        BuildPayload payload = new BuildPayload(block, team);
        ((Payloadc) carrier).addPayload(payload);
    }

    static void normalizePower(Building a, Building b) {
        a.updatePowerGraph();
        b.updatePowerGraph();
        a.placed();
        b.placed();
    }

    static Unit findUnitNear(float x, float y, mindustry.type.UnitType type) {
        for (Unit unit : Groups.unit) {
            if (unit.type() == type && unit.x() == x && unit.y() == y) {
                return unit;
            }
        }
        return null;
    }

    // --- Observation / dump ------------------------------------------------------

    static Case dump(Carrier carrier, int originPos, int targetUnitId, boolean success,
                     float dropX, float dropY, int dropTileX, int dropTileY) {
        Case c = new Case();
        c.success = success;
        c.carrierPayloadCount = payloadCount(carrier.unit);
        c.carrierPayloadTypes = payloadTypes(carrier.unit);
        if (originPos >= 0) {
            Tile t = tileAtPos(originPos);
            c.originBuildingExists = t != null && t.build != null && !t.build.dead;
        }
        if (targetUnitId >= 0) {
            c.targetUnitExists = findUnitById(targetUnitId) != null;
        }
        if (dropTileX >= 0) {
            c.dropX = dropX;
            c.dropY = dropY;
            c.dropTileX = dropTileX;
            c.dropTileY = dropTileY;
        }
        c.worldBuildings = countBuildings();
        c.worldUnits = Groups.unit.size();
        return c;
    }

    static int payloadCount(Unit unit) {
        if (!(unit instanceof Payloadc payloadc)) {
            return 0;
        }
        return payloadc.payloads().size;
    }

    static List<String> payloadTypes(Unit unit) {
        if (!(unit instanceof Payloadc payloadc)) {
            return List.of();
        }
        List<String> out = new ArrayList<>();
        for (Payload payload : payloadc.payloads()) {
            if (payload instanceof UnitPayload) {
                out.add("unit");
            } else if (payload instanceof BuildPayload) {
                out.add("build");
            } else {
                out.add("other");
            }
        }
        return out;
    }

    static Unit findUnitById(int id) {
        for (Unit unit : Groups.unit) {
            if (unit.id == id) {
                return unit;
            }
        }
        return null;
    }

    static int countBuildings() {
        int count = 0;
        for (Building build : Groups.build) {
            if (build != null && !build.dead) {
                count++;
            }
        }
        return count;
    }

    static List<int[]> footprintOf(mindustry.world.Block block, int tileX, int tileY) {
        List<int[]> cells = new ArrayList<>();
        int size = block.size;
        for (int dx = 0; dx < size; dx++) {
            for (int dy = 0; dy < size; dy++) {
                cells.add(new int[]{tileX + dx, tileY + dy});
            }
        }
        cells.sort((a, b) -> a[0] != b[0] ? Integer.compare(a[0], b[0]) : Integer.compare(a[1], b[1]));
        return cells;
    }

    static List<int[]> linkTiles(Building node) {
        List<int[]> out = new ArrayList<>();
        if (node == null || node.power == null) {
            return out;
        }
        arc.struct.Seq<Building> links = node.getPowerConnections(new arc.struct.Seq<>());
        for (Building other : links) {
            if (other != null && other.tile != null) {
                out.add(new int[]{other.tile.x, other.tile.y});
            }
        }
        out.sort((a, b) -> a[0] != b[0] ? Integer.compare(a[0], b[0]) : Integer.compare(a[1], b[1]));
        return out;
    }

    static String scenario(String name, Case c) {
        return "    \"" + name + "\": " + c.toJson();
    }

    static final class Carrier {
        final Player player;
        final Unit unit;

        Carrier(Player player, Unit unit) {
            this.player = player;
            this.unit = unit;
        }
    }

    static final class Case {
        boolean success;
        int carrierPayloadCount;
        List<String> carrierPayloadTypes = List.of();
        boolean originBuildingExists;
        boolean targetUnitExists;
        float dropX;
        float dropY;
        int dropTileX = -1;
        int dropTileY = -1;
        List<int[]> footprint = List.of();
        List<int[]> nodeLinksAfterPickup = List.of();
        List<int[]> nodeLinksAfterDrop = List.of();
        int worldBuildings;
        int worldUnits;
        boolean secondSuccess;
        int secondPayloadCount;
        boolean loaderStillExists;
        boolean extraOriginExists;
        boolean droppedUnitExists;
        boolean emitNodeLinks;
        boolean emitRace;

        String toJson() {
            StringBuilder json = new StringBuilder();
            json.append("{");
            json.append("\"success\": ").append(success);
            json.append(", \"carrier_payload_count\": ").append(carrierPayloadCount);
            json.append(", \"carrier_payload_types\": ").append(jsonStringList(carrierPayloadTypes));
            json.append(", \"origin_building_exists\": ").append(originBuildingExists);
            json.append(", \"target_unit_exists\": ").append(targetUnitExists);
            if (dropTileX >= 0) {
                json.append(", \"drop_x\": ").append(num(dropX));
                json.append(", \"drop_y\": ").append(num(dropY));
                json.append(", \"drop_tile_x\": ").append(dropTileX);
                json.append(", \"drop_tile_y\": ").append(dropTileY);
            }
            if (!footprint.isEmpty()) {
                json.append(", \"footprint\": ").append(jsonFootprint(footprint));
            }
            if (emitNodeLinks) {
                json.append(", \"node_links_after_pickup\": ").append(jsonFootprint(nodeLinksAfterPickup));
                json.append(", \"node_links_after_drop\": ").append(jsonFootprint(nodeLinksAfterDrop));
            } else if (!nodeLinksAfterPickup.isEmpty() || !nodeLinksAfterDrop.isEmpty()) {
                json.append(", \"node_links_after_pickup\": ").append(jsonFootprint(nodeLinksAfterPickup));
                json.append(", \"node_links_after_drop\": ").append(jsonFootprint(nodeLinksAfterDrop));
            }
            json.append(", \"world_buildings\": ").append(worldBuildings);
            json.append(", \"world_units\": ").append(worldUnits);
            if (emitRace) {
                json.append(", \"second_success\": ").append(secondSuccess);
                json.append(", \"second_payload_count\": ").append(secondPayloadCount);
            } else if (secondSuccess || secondPayloadCount > 0) {
                json.append(", \"second_success\": ").append(secondSuccess);
                json.append(", \"second_payload_count\": ").append(secondPayloadCount);
            }
            if (loaderStillExists || extraOriginExists) {
                json.append(", \"loader_still_exists\": ").append(loaderStillExists);
            }
            if (extraOriginExists) {
                json.append(", \"extra_origin_exists\": true");
            }
            if (droppedUnitExists) {
                json.append(", \"dropped_unit_exists\": true");
            }
            if (dropTileX < 0 && droppedUnitExists) {
                json.append(", \"drop_x\": ").append(num(dropX));
                json.append(", \"drop_y\": ").append(num(dropY));
            }
            json.append("}");
            return json.toString();
        }
    }

    // --- JSON helpers ------------------------------------------------------------

    static String jsonFootprint(List<int[]> cells) {
        StringBuilder out = new StringBuilder("[");
        for (int i = 0; i < cells.size(); i++) {
            if (i > 0) out.append(", ");
            out.append("[").append(cells.get(i)[0]).append(", ").append(cells.get(i)[1]).append("]");
        }
        return out.append("]").toString();
    }

    static String jsonStringList(List<String> values) {
        StringBuilder out = new StringBuilder("[");
        for (int i = 0; i < values.size(); i++) {
            if (i > 0) out.append(", ");
            out.append(jsonString(values.get(i)));
        }
        return out.append("]").toString();
    }

    static String jsonString(String value) {
        return "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"") + "\"";
    }

    static String num(float value) {
        return String.format(Locale.US, "%.6f", value);
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParPayload158.class.getResourceAsStream("/version.properties")) {
            if (in == null) {
                return "unknown";
            }
            Properties props = new Properties();
            props.load(in);
            String build = props.getProperty("build", "?");
            String type = props.getProperty("type", "?");
            return type + " " + build;
        }
    }
}
