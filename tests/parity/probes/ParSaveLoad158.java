import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Base64;
import java.util.Collections;
import java.util.Comparator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.files.Fi;
import arc.struct.StringMap;
import arc.util.Time;
import mindustry.Vars;
import mindustry.ai.BlockIndexer;
import mindustry.ai.types.CommandAI;
import mindustry.ai.types.LogicAI;
import mindustry.content.Blocks;
import mindustry.content.Items;
import mindustry.content.Liquids;
import mindustry.content.StatusEffects;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.Logic;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.entities.Puddles;
import mindustry.game.Team;
import mindustry.game.Teams;
import mindustry.game.Waves;
import mindustry.gen.Building;
import mindustry.gen.Groups;
import mindustry.gen.Payloadc;
import mindustry.gen.Puddle;
import mindustry.gen.Unit;
import mindustry.io.SaveIO;
import mindustry.logic.GlobalVars;
import mindustry.mod.Mods;
import mindustry.type.Item;
import mindustry.type.Liquid;
import mindustry.type.StatusEffect;
import mindustry.world.Tile;
import mindustry.world.Tiles;
import mindustry.world.blocks.logic.LogicBlock.LogicBuild;
import mindustry.world.blocks.payloads.BuildPayload;
import mindustry.world.blocks.payloads.Payload;
import mindustry.world.blocks.payloads.PayloadConveyor.PayloadConveyorBuild;
import mindustry.world.blocks.payloads.UnitPayload;

/**
 * P2-C1 differential probe: server-authoritative MSAV round-trip on
 * desktop.jar 158.1 (Save11).
 *
 * Campaigns:
 *   Java save → load → 300 ticks
 *   Rust MSAV → Java load → 300 ticks (when RUST_MSAV_PATH is set)
 *   Rust MSAV → Java load → Java save (reexport bytes)
 *
 * Version gate: official 158.1 only.
 */
public final class ParSaveLoad158 {
    static final int WORLD = 16;
    static final int CAMPAIGN_TICKS = 300;
    static final int GHOST_POS = (40 << 16) | 40;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParSaveLoad158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

        boot();
        Time.setDeltaProvider(() -> 1f);

        Fi saveFile = Core.files.local("parity-save-load.msav");
        if (saveFile.exists()) saveFile.delete();

        Map<String, Object> afterLoad;
        Map<String, Object> after300;
        String javaMsavB64;
        Map<String, Object> outcomes = new LinkedHashMap<>();

        buildCampaignWorld();
        SaveIO.save(saveFile);
        javaMsavB64 = Base64.getEncoder().encodeToString(saveFile.readBytes());
        SaveIO.load(saveFile, Vars.world.context);
        afterLoad = campaignSnapshot();
        tickCampaign(CAMPAIGN_TICKS);
        after300 = campaignSnapshot();

        outcomes.put("logic_program_survives", hasLogic());
        outcomes.put("nested_payload_survives", nestedPayloadPresent());
        outcomes.put("multiblock_payload_survives", multiblockPayloadPresent());
        outcomes.put("status_expired_after_300", !anyStatus(StatusEffects.slow));
        outcomes.put("rules_multiplier_survives",
            Math.abs(Vars.state.rules.unitDamageMultiplier - 1.5f) < 0.001f);

        resetWorld(saveFile);
        outcomes.put("stale_power_pruned", runStalePower(saveFile));
        resetWorld(saveFile);
        outcomes.put("missing_command_target_dropped", runMissingCommandTarget(saveFile));
        resetWorld(saveFile);
        outcomes.put("destroyed_processor_releases_lease", runDestroyedProcessor(saveFile));

        String rustMsavB64 = null;
        Map<String, Object> rustAfterLoad = null;
        Map<String, Object> rustAfter300 = null;
        String reexportB64 = null;
        String rustPath = System.getenv("RUST_MSAV_PATH");
        if (rustPath != null && !rustPath.isBlank()) {
            Path rustFile = Path.of(rustPath);
            if (!Files.isRegularFile(rustFile)) {
                System.err.println("ParSaveLoad158: RUST_MSAV_PATH is not a file: " + rustPath);
                System.exit(3);
            }
            rustMsavB64 = Base64.getEncoder().encodeToString(Files.readAllBytes(rustFile));
            resetWorld(saveFile);
            Fi rustFi = new Fi(rustFile.toFile());
            SaveIO.load(rustFi, Vars.world.context);
            rustAfterLoad = campaignSnapshot();
            Fi reexport = Core.files.local("parity-save-load-reexport.msav");
            if (reexport.exists()) reexport.delete();
            SaveIO.save(reexport);
            reexportB64 = Base64.getEncoder().encodeToString(reexport.readBytes());
            tickCampaign(CAMPAIGN_TICKS);
            rustAfter300 = campaignSnapshot();
        }

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("save-load")).append(",\n");
        json.append("  \"tick\": ").append(CAMPAIGN_TICKS).append(",\n");
        json.append("  \"java_msav_b64\": ").append(jsonString(javaMsavB64)).append(",\n");
        json.append("  \"rust_msav_b64\": ").append(rustMsavB64 == null ? "null" : jsonString(rustMsavB64)).append(",\n");
        json.append("  \"java_reexport_b64\": ").append(reexportB64 == null ? "null" : jsonString(reexportB64)).append(",\n");
        appendScenarioMap(json, "after_load", afterLoad, false);
        appendScenarioMap(json, "after_300", after300, false);
        json.append("  \"rust_to_java_after_load\": ");
        appendValue(json, rustAfterLoad, "  ");
        json.append(",\n");
        json.append("  \"rust_to_java_after_300\": ");
        appendValue(json, rustAfter300, "  ");
        json.append(",\n");
        appendScenarioMap(json, "outcomes", outcomes, true);
        json.append("}\n");
        System.out.print(json);
    }

    static void buildCampaignWorld() {
        Vars.state.rules.unitDamageMultiplier = 1.5f;
        Vars.state.rules.teams.get(Team.sharded).unitDamageMultiplier = 2f;
        Vars.state.rules.waves = false;
        Vars.state.rules.waveTimer = false;
        Vars.state.rules.disableUnitCap = true;

        Vars.world.tile(2, 2).setFloor(Blocks.stone.asFloor());
        Vars.world.tile(2, 2).setBlock(Blocks.powerSource, Team.sharded, 0);
        Vars.world.tile(4, 2).setFloor(Blocks.stone.asFloor());
        Vars.world.tile(4, 2).setBlock(Blocks.powerNode, Team.sharded, 0);
        Building source = Vars.world.tile(2, 2).build;
        Building node = Vars.world.tile(4, 2).build;
        node.configure(source.pos());

        Vars.world.tile(3, 8).setBlock(Blocks.vault, Team.sharded, 0);
        Vars.world.tile(3, 8).build.items.add(Items.copper, 50);

        Vars.world.tile(7, 8).setBlock(Blocks.liquidTank, Team.sharded, 0);
        Vars.world.tile(7, 8).build.liquids.add(Liquids.water, 80f);

        Vars.world.tile(10, 2).setBlock(Blocks.microProcessor, Team.sharded, 0);
        ((LogicBuild) Vars.world.tile(10, 2).build).updateCode("op add n n 1\nend");

        Vars.world.tile(12, 8).setBlock(Blocks.payloadConveyor, Team.sharded, 0);
        PayloadConveyorBuild conv = (PayloadConveyorBuild) Vars.world.tile(12, 8).build;
        conv.handlePayload(conv, new BuildPayload(Blocks.vault, Team.sharded));

        Vars.state.teams.get(Team.sharded).plans.add(
            new Teams.BlockPlan(14, 2, (short) 0, Blocks.copperWall, null)
        );

        Unit flare = UnitTypes.flare.create(Team.sharded);
        flare.set(80f, 80f);
        flare.apply(StatusEffects.slow, 10f);
        flare.add();
        commandAi(flare);

        Unit mega = UnitTypes.mega.create(Team.sharded);
        mega.set(88f, 80f);
        mega.add();
        Unit dagger = UnitTypes.dagger.create(Team.sharded);
        ((Payloadc) mega).addPayload(new UnitPayload(dagger));

        Puddles.deposit(Vars.world.tile(3, 3), Liquids.water, 40f);
        Vars.state.teams.updateTeamStats();
        Vars.state.set(GameState.State.playing);
    }

    static void tickCampaign(int ticks) {
        Vars.state.set(GameState.State.playing);
        Time.setDeltaProvider(() -> 1f);
        for (int i = 0; i < ticks; i++) {
            Groups.build.update();
            Groups.unit.update();
            Groups.puddle.update();
        }
    }

    static boolean runStalePower(Fi saveFile) throws Exception {
        Vars.world.tile(2, 2).setBlock(Blocks.powerSource, Team.sharded, 0);
        Vars.world.tile(4, 2).setBlock(Blocks.powerNode, Team.sharded, 0);
        Building source = Vars.world.tile(2, 2).build;
        Building node = Vars.world.tile(4, 2).build;
        node.configure(source.pos());
        node.power.links.addUnique(GHOST_POS);
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Building loaded = Vars.world.tile(4, 2).build;
        return loaded != null && loaded.power != null && !loaded.power.links.contains(GHOST_POS);
    }

    static boolean runMissingCommandTarget(Fi saveFile) throws Exception {
        Unit flare = UnitTypes.flare.create(Team.sharded);
        flare.set(80f, 80f);
        flare.add();
        CommandAI ai = commandAi(flare);
        Unit foe = UnitTypes.dagger.create(Team.crux);
        foe.set(120f, 120f);
        foe.add();
        ai.commandQueue(foe);
        foe.remove();
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Unit loaded = Groups.unit.find(u -> u.type == UnitTypes.flare);
        if (loaded == null || !(loaded.controller() instanceof CommandAI loadedAi)) {
            return true;
        }
        return loadedAi.commandQueue.find(t -> t instanceof Unit) == null;
    }

    static boolean runDestroyedProcessor(Fi saveFile) throws Exception {
        Tile procTile = Vars.world.tile(8, 8);
        procTile.setBlock(Blocks.microProcessor, Team.sharded, 0);
        Building proc = procTile.build;
        Unit unit = UnitTypes.flare.create(Team.sharded);
        unit.set(80f, 80f);
        unit.add();
        LogicAI ai = new LogicAI();
        ai.controller = proc;
        unit.controller(ai);
        procTile.setBlock(Blocks.air, Team.derelict, 0);
        SaveIO.save(saveFile);
        SaveIO.load(saveFile, Vars.world.context);
        Unit loaded = Groups.unit.find(u -> u.type == UnitTypes.flare);
        if (loaded == null) return true;
        if (!(loaded.controller() instanceof LogicAI loadedAi)) return true;
        return loadedAi.controller == null || loadedAi.controller.tile == null || loadedAi.controller.tile.build == null;
    }

    static Map<String, Object> campaignSnapshot() {
        Map<String, Object> out = new LinkedHashMap<>();
        List<Map<String, Object>> units = new ArrayList<>();
        for (Unit unit : Groups.unit) {
            units.add(snapshotUnit(unit));
        }
        units.sort(Comparator.comparingInt(u -> (Integer) u.get("id")));
        out.put("units", units);

        List<Map<String, Object>> buildings = new ArrayList<>();
        for (Building build : Groups.build) {
            if (build.tile == null) continue;
            buildings.add(snapshotBuilding(build));
        }
        buildings.sort(Comparator.comparingInt(b -> (Integer) b.get("pos")));
        out.put("buildings", buildings);

        List<Map<String, Object>> puddles = new ArrayList<>();
        for (Puddle puddle : Groups.puddle) {
            Map<String, Object> row = new LinkedHashMap<>();
            row.put("pos", puddle.tile == null ? -1 : puddle.tile.pos());
            row.put("liquid", puddle.liquid == null ? -1 : (int) puddle.liquid.id);
            row.put("amount", puddle.amount);
            puddles.add(row);
        }
        puddles.sort(Comparator.comparingInt(p -> (Integer) p.get("pos")));
        out.put("puddles", puddles);

        out.put("plan_count", Vars.state.teams.get(Team.sharded).plans.size);

        Map<String, Object> rules = new LinkedHashMap<>();
        rules.put("unitDamageMultiplier", Vars.state.rules.unitDamageMultiplier);
        rules.put("unitHealthMultiplier", Vars.state.rules.unitHealthMultiplier);
        rules.put("infiniteResources", Vars.state.rules.infiniteResources);
        List<Map<String, Object>> teams = new ArrayList<>();
        Map<String, Object> sharded = new LinkedHashMap<>();
        sharded.put("team", (int) Team.sharded.id);
        sharded.put("unitDamageMultiplier", Vars.state.rules.teams.get(Team.sharded).unitDamageMultiplier);
        teams.add(sharded);
        rules.put("teams", teams);
        out.put("rules", rules);
        return out;
    }

    static Map<String, Object> snapshotUnit(Unit unit) {
        Map<String, Object> out = new LinkedHashMap<>();
        out.put("id", unit.id);
        out.put("type", (int) unit.type.id);
        out.put("team", (int) unit.team.id);
        out.put("health", unit.health);
        out.put("x", unit.x);
        out.put("y", unit.y);
        out.put("is_logic_ai", unit.controller() instanceof LogicAI);
        out.put("is_command_ai", unit.controller() instanceof CommandAI);
        List<Map<String, Object>> statuses = new ArrayList<>();
        for (StatusEffect effect : Vars.content.statusEffects()) {
            if (effect == null || effect == StatusEffects.none) continue;
            float time = unit.getDuration(effect);
            if (time > 0f) {
                Map<String, Object> row = new LinkedHashMap<>();
                row.put("effect", (int) effect.id);
                row.put("time", time);
                statuses.add(row);
            }
        }
        out.put("statuses", statuses);
        List<Map<String, Object>> payloads = new ArrayList<>();
        if (unit instanceof Payloadc pay) {
            for (Payload payload : pay.payloads()) {
                payloads.add(snapshotPayload(payload));
            }
        }
        out.put("payloads", payloads);
        return out;
    }

    static Map<String, Object> snapshotBuilding(Building build) {
        Map<String, Object> out = new LinkedHashMap<>();
        out.put("pos", build.pos());
        out.put("block", (int) build.block.id);
        out.put("team", (int) build.team.id);
        out.put("health", build.health);
        List<List<Object>> inventory = new ArrayList<>();
        if (build.items != null) {
            for (Item item : Vars.content.items()) {
                int amount = build.items.get(item);
                if (amount > 0) {
                    inventory.add(List.of((int) item.id, amount));
                }
            }
        }
        out.put("inventory", inventory);
        List<List<Object>> liquids = new ArrayList<>();
        if (build.liquids != null) {
            for (Liquid liquid : Vars.content.liquids()) {
                float amount = build.liquids.get(liquid);
                if (amount > 0.001f) {
                    liquids.add(List.of((int) liquid.id, amount));
                }
            }
        }
        out.put("liquids", liquids);
        List<Integer> links = new ArrayList<>();
        if (build.power != null) {
            for (int i = 0; i < build.power.links.size; i++) {
                links.add(build.power.links.get(i));
            }
            Collections.sort(links);
        }
        out.put("power_links", links);
        Object payload = null;
        if (build instanceof PayloadConveyorBuild conv && conv.item != null) {
            payload = snapshotPayload(conv.item);
        }
        out.put("payload", payload);
        out.put("has_logic", build instanceof LogicBuild logic && logic.code != null && !logic.code.isEmpty());
        return out;
    }

    static Map<String, Object> snapshotPayload(Payload payload) {
        Map<String, Object> out = new LinkedHashMap<>();
        if (payload instanceof UnitPayload unitPay) {
            out.put("kind", "unit");
            out.put("type", (int) unitPay.unit.type.id);
            int nested = 0;
            if (unitPay.unit instanceof Payloadc inner) nested = inner.payloads().size;
            out.put("nested", nested);
        } else if (payload instanceof BuildPayload buildPay) {
            out.put("kind", "build");
            out.put("block", (int) buildPay.build.block.id);
        } else {
            out.put("kind", "other");
        }
        return out;
    }

    static boolean hasLogic() {
        for (Building build : Groups.build) {
            if (build instanceof LogicBuild logic && logic.code != null && !logic.code.isEmpty()) {
                return true;
            }
        }
        return false;
    }

    static boolean nestedPayloadPresent() {
        for (Unit unit : Groups.unit) {
            if (unit instanceof Payloadc pay) {
                for (Payload payload : pay.payloads()) {
                    if (payload instanceof UnitPayload unitPay && unitPay.unit.type == UnitTypes.dagger) {
                        return true;
                    }
                }
            }
        }
        return false;
    }

    static boolean multiblockPayloadPresent() {
        for (Building build : Groups.build) {
            if (build instanceof PayloadConveyorBuild conv
                && conv.item instanceof BuildPayload bp
                && bp.build.block == Blocks.vault) {
                return true;
            }
        }
        return false;
    }

    static boolean anyStatus(StatusEffect effect) {
        for (Unit unit : Groups.unit) {
            if (unit.getDuration(effect) > 0f) return true;
        }
        return false;
    }

    static CommandAI commandAi(Unit unit) {
        if (!(unit.controller() instanceof CommandAI ai)) {
            System.err.println("ParSaveLoad158: expected CommandAI controller, got "
                + unit.controller().getClass().getSimpleName());
            System.exit(3);
            return null;
        }
        return ai;
    }

    static void boot() {
        Version.build = 158;
        Vars.headless = true;
        Vars.platform = new mindustry.core.Platform(){};
        Vars.net = new mindustry.net.Net(Vars.platform.getNet());
        Core.files = new SdlFiles();
        Core.settings = new arc.Settings();
        Vars.dataDirectory = Core.files.local("mindustry-parity-data/");
        Vars.customMapDirectory = Vars.dataDirectory.child("maps/");
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.logic = new Logic();
        Vars.waves = new Waves();
        Vars.mods = new Mods();
        Vars.maps = new mindustry.maps.Maps();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();
        Vars.state = new GameState();
        Vars.state.map = new mindustry.maps.Map(StringMap.of(
            "name", "parity-save-load",
            "width", String.valueOf(WORLD),
            "height", String.valueOf(WORLD)
        ));
        Vars.state.rules.disableUnitCap = true;
        Groups.init();
        Vars.indexer = new BlockIndexer();
        Vars.world = new World();
        resetTiles();
    }

    static void resetWorld(Fi saveFile) throws Exception {
        if (saveFile.exists()) saveFile.delete();
        Vars.logic.reset();
        Groups.init();
        Vars.indexer = new BlockIndexer();
        Vars.state = new GameState();
        Vars.state.map = new mindustry.maps.Map(StringMap.of(
            "name", "parity-save-load",
            "width", String.valueOf(WORLD),
            "height", String.valueOf(WORLD)
        ));
        Vars.state.rules.disableUnitCap = true;
        resetTiles();
    }

    static void resetTiles() {
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();
        for (Tile tile : Vars.world.tiles) {
            tile.setFloor(Blocks.stone.asFloor());
        }
    }

    static void appendScenarioMap(StringBuilder json, String key, Map<String, Object> map, boolean last) {
        json.append("  \"").append(key).append("\": ");
        appendValue(json, map, "  ");
        json.append(last ? "\n" : ",\n");
    }

    @SuppressWarnings("unchecked")
    static void appendValue(StringBuilder json, Object value, String indent) {
        if (value == null) {
            json.append("null");
        } else if (value instanceof Boolean b) {
            json.append(b);
        } else if (value instanceof Integer i) {
            json.append(i);
        } else if (value instanceof Long l) {
            json.append(l);
        } else if (value instanceof Float f) {
            json.append(num(f));
        } else if (value instanceof Double d) {
            json.append(num(d.floatValue()));
        } else if (value instanceof Number n) {
            json.append(n.intValue());
        } else if (value instanceof String s) {
            json.append(jsonString(s));
        } else if (value instanceof List<?> list) {
            json.append("[");
            for (int i = 0; i < list.size(); i++) {
                if (i > 0) json.append(", ");
                appendValue(json, list.get(i), indent);
            }
            json.append("]");
        } else if (value instanceof Map<?, ?> map) {
            json.append("{\n");
            int i = 0;
            for (var entry : ((Map<String, Object>) map).entrySet()) {
                json.append(indent).append("  \"").append(entry.getKey()).append("\": ");
                appendValue(json, entry.getValue(), indent + "  ");
                json.append(i++ == map.size() - 1 ? "\n" : ",\n");
            }
            json.append(indent).append("}");
        } else {
            json.append(jsonString(String.valueOf(value)));
        }
    }

    static String num(float value) {
        if (Float.isInfinite(value)) return value > 0 ? "1e38" : "-1e38";
        if (Float.isNaN(value)) return "0.000000";
        return String.format(Locale.US, "%.6f", value);
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParSaveLoad158.class.getResourceAsStream("/version.properties")) {
            if (in == null) return "missing";
            Properties p = new Properties();
            p.load(in);
            return p.getProperty("type", "missing") + " " + p.getProperty("build", "missing");
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
