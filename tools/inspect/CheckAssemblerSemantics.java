import arc.Core;
import arc.Settings;
import mindustry.Vars;
import mindustry.content.Blocks;
import mindustry.core.ContentLoader;
import mindustry.type.PayloadStack;
import mindustry.world.Block;
import mindustry.world.blocks.units.UnitAssembler;
import mindustry.world.consumers.ConsumePayloadDynamic;

/**
 * Behavioral oracle for the independently implemented assembler simulation.
 * Run with the pinned 160.5 JAR:
 * java --class-path "$MINDUSTRY_CURRENT_JAR" tools/inspect/CheckAssemblerSemantics.java
 * No server saves or external state are written.
 */
public final class CheckAssemblerSemantics {
    private static void require(boolean value, String message) {
        if (!value) throw new AssertionError(message);
    }

    public static void main(String[] args) throws Exception {
        arc.util.Log.logger = (level, text) -> {};
        Vars.headless = true;
        Core.app = new arc.mock.MockApplication();
        Core.audio = new arc.mock.MockAudio();
        Core.settings = new Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        require(Blocks.basicAssemblerModule.sync, "assembler module requires periodic synchronization");
        int plansChecked = 0;
        int geometryChecked = 0;
        float[][] destinations = {{232f, 160f}, {160f, 232f}, {88f, 160f}, {160f, 88f}};
        for (Block raw : new Block[]{Blocks.tankAssembler, Blocks.shipAssembler, Blocks.mechAssembler}) {
            UnitAssembler assembler = (UnitAssembler)raw;
            require(assembler.size == 5 && assembler.areaSize == 13, assembler.name + " dimensions");
            for (int tier = 0; tier < assembler.plans.size; tier++) {
                var plan = assembler.plans.get(tier);
                require(plan.itemReq == null, assembler.name + " must not substitute core items");
                var build = assembler.new UnitAssemblerBuild();
                build.currentTier = tier;
                var consumer = new ConsumePayloadDynamic(b -> plan.requirements);
                require(consumer.efficiency(build) == 0f, "empty local payloads must block production");
                for (PayloadStack stack : plan.requirements) build.getPayloads().add(stack.item, stack.amount);
                require(consumer.efficiency(build) == 1f, "complete local payload stacks must permit production");
                consumer.trigger(build);
                require(build.getPayloads().isEmpty(), "completion must consume the local payload stacks");
                plansChecked++;
            }
            var build = assembler.new UnitAssemblerBuild();
            build.x = 160f;
            build.y = 160f;
            for (int rotation = 0; rotation < 4; rotation++) {
                build.rotation = rotation;
                var spawn = build.getUnitSpawn().cpy();
                require(spawn.x == destinations[rotation][0] && spawn.y == destinations[rotation][1],
                    assembler.name + " spawn rotation " + rotation);
                require(build.moduleFits(Blocks.basicAssemblerModule, spawn.x + 72f, spawn.y, 2), "valid perimeter module");
                require(!build.moduleFits(Blocks.basicAssemblerModule, spawn.x + 72f, spawn.y, 0), "wrong-facing module");
                require(!build.moduleFits(Blocks.basicAssemblerModule, spawn.x + 64f, spawn.y, 2), "inset module");
                geometryChecked++;
            }
        }
        require(plansChecked == 6 && geometryChecked == 12, "complete vanilla assembler matrix");
        // AssemblerAI.inPosition delegates to these predicates. Check the
        // subtle boundaries separately from movement's 1/5-unit thresholds.
        var position = new arc.math.geom.Vec2(0f, 0f);
        require(!position.within(10f, 0f, 10f), "position distance boundary is strict");
        require(position.within(9.999f, 0f, 10f), "inside position distance");
        require(arc.math.Angles.within(0f, 15f, 15f), "angular boundary is inclusive");
        System.out.println("ASSEMBLER_SEMANTICS_PASS plans=" + plansChecked + " geometry=" + geometryChecked);
        if (args.length == 1) checkSnapshots(java.nio.file.Path.of(args[0]));
    }

    private static void readSnapshot(UnitAssembler.UnitAssemblerBuild build, java.nio.file.Path path) throws Exception {
        var input = new java.io.DataInputStream(new java.io.ByteArrayInputStream(java.nio.file.Files.readAllBytes(path)));
        build.readSync(arc.util.io.Reads.get(input), build.version());
        require(input.available() == 0, path + " has unread bytes=" + input.available());
        require(build.getPayloads().get(mindustry.content.UnitTypes.stell) == 8, "stell decoded as a unit");
        require(build.getPayloads().get(Blocks.tungstenWallLarge) == 20, "wall decoded as a block");
    }

    private static void checkSnapshots(java.nio.file.Path directory) throws Exception {
        Vars.state = new mindustry.core.GameState();
        Vars.state.rules.unitCap = 1000;
        Vars.net = new mindustry.net.Net(null);
        mindustry.gen.Groups.init();
        Vars.world = new mindustry.core.World();
        Vars.world.resize(60, 60).fill();
        Vars.indexer = new mindustry.ai.BlockIndexer();
        Vars.collisions = new mindustry.entities.EntityCollisions();
        Vars.world.tile(20, 20).setBlock(Blocks.tankAssembler, mindustry.game.Team.sharded, 0);
        var client = (UnitAssembler.UnitAssemblerBuild)Vars.world.build(20, 20);
        client.getPayloads().add(mindustry.content.UnitTypes.stell, 8);
        client.getPayloads().add(Blocks.tungstenWallLarge, 20);
        client.liquids.set(mindustry.content.Liquids.cyanogen, 100f);
        float[][] slots = {{284f, 212f}, {180f, 212f}, {180f, 108f}, {284f, 108f}};
        for (int i = 0; i < 4; i++) {
            var drone = mindustry.content.UnitTypes.assemblyDrone.create(mindustry.game.Team.sharded);
            drone.id(-100 - i);
            drone.set(slots[i][0], slots[i][1]);
            drone.rotation = 225f + 90f * i;
            ((mindustry.gen.BuildingTetherc)drone).building(client);
            drone.add();
            client.units.add(drone);
        }
        // A second build only decodes fixtures, keeping the official client
        // prediction independent until each simulated BlockSnapshot arrives.
        Vars.world.tile(45, 45).setBlock(Blocks.tankAssembler, mindustry.game.Team.sharded, 0);
        var expected = (UnitAssembler.UnitAssemblerBuild)Vars.world.build(45, 45);
        arc.util.Time.delta = 1f;
        for (int tick = 1; tick <= 480; tick++) {
            client.power.status = 1f;
            client.updateConsumption();
            client.updateTile();
            if (tick == 360 || tick == 480) {
                java.nio.file.Path path = directory.resolve("assembler-" + tick + "-sync.bin");
                readSnapshot(expected, path);
                float progress = client.progress;
                float liquid = client.liquids.get(mindustry.content.Liquids.cyanogen);
                require(Math.abs(progress - expected.progress) < 0.00001f,
                    "client progress drift at " + tick + ": " + progress + " vs " + expected.progress);
                require(Math.abs(liquid - expected.liquids.get(mindustry.content.Liquids.cyanogen)) < 0.0001f,
                    "client cyanogen drift at " + tick);
                readSnapshot(client, path);
                require(Math.abs(client.progress - progress) < 0.00001f, "snapshot rewound assembly");
                require(client.liquids.get(mindustry.content.Liquids.cyanogen) == liquid, "snapshot rewound cyanogen");
            }
        }
        System.out.println("ASSEMBLER_CLIENT_480_TICKS_SNAPSHOT_PASS progress=" + client.progress);
        for (String kind : new String[]{"unit", "block"}) {
            var path = directory.resolve("assembler-in-transit-" + kind + "-sync.bin");
            var input = new java.io.DataInputStream(new java.io.ByteArrayInputStream(java.nio.file.Files.readAllBytes(path)));
            expected.readSync(arc.util.io.Reads.get(input), expected.version());
            require(input.available() == 0, "in-transit snapshot unread bytes");
            require(expected.payVector.x == -20f && expected.payVector.y == 0f, "incoming payload vector");
            require(expected.payload != null, "incoming payload disappeared");
            if (kind.equals("unit")) require(expected.payload.content() == mindustry.content.UnitTypes.stell, "incoming unit type");
            else require(expected.payload.content() == Blocks.tungstenWallLarge, "incoming block type");
        }
        System.out.println("ASSEMBLER_TYPED_INPUT_SNAPSHOT_PASS unit=stell block=tungsten-wall-large");
        Vars.world.tile(38, 20).setBlock(Blocks.basicAssemblerModule, mindustry.game.Team.sharded, 2);
        var module = (mindustry.world.blocks.units.UnitAssemblerModule.UnitAssemblerModuleBuild)Vars.world.build(38, 20);
        module.link = client;
        module.lastChange = Vars.world.tileChanges;
        client.updateModules(module);
        for (String kind : new String[]{"unit", "block"}) {
            var path = directory.resolve("assembler-module-in-transit-" + kind + "-sync.bin");
            var input = new java.io.DataInputStream(new java.io.ByteArrayInputStream(java.nio.file.Files.readAllBytes(path)));
            module.readSync(arc.util.io.Reads.get(input), module.version());
            require(input.available() == 0, "module snapshot unread bytes");
            require(module.payVector.x == 0f && module.payVector.y == 20f, "module incoming vector");
            var content = kind.equals("unit") ? mindustry.content.UnitTypes.locus : Blocks.carbideWallLarge;
            require(module.payload != null && module.payload.content() == content, "module input content");
            arc.util.Time.delta = 30f;
            module.efficiency = 0f;
            module.updateTile();
            require(module.payload != null, "unpowered module must retain centred input");
            module.efficiency = 1f;
            module.updateTile();
            require(module.payload == null && client.getPayloads().get(content) == 1, "module deposits into linked assembler");
        }
        System.out.println("ASSEMBLER_MODULE_INPUT_SNAPSHOT_AND_FORWARD_PASS");
    }

}
