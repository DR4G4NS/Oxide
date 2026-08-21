import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.io.FileInputStream;
import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.zip.InflaterInputStream;

import arc.files.Fi;
import arc.mock.MockApplication;
import arc.mock.MockAudio;
import arc.Core;
import arc.Settings;
import arc.util.io.Reads;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.core.Control;
import mindustry.core.GameState;
import mindustry.core.NetClient;
import mindustry.core.World;
import mindustry.entities.Units;
import mindustry.gen.Groups;
import mindustry.gen.Player;
import mindustry.gen.CommandBuildingCallPacket;
import mindustry.gen.BlockSnapshotCallPacket;
import mindustry.gen.CommandUnitsCallPacket;
import mindustry.gen.ConstructFinishCallPacket;
import mindustry.gen.SetUnitCommandCallPacket;
import mindustry.gen.SetUnitStanceCallPacket;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.gen.UnitEnteredPayloadCallPacket;
import mindustry.gen.PickedBuildPayloadCallPacket;
import mindustry.gen.Payloadc;
import mindustry.content.Blocks;
import mindustry.game.Team;
import mindustry.world.blocks.ConstructBlock.ConstructBuild;
import mindustry.world.blocks.payloads.UnitPayload;
import mindustry.world.blocks.payloads.BuildPayload;
import mindustry.world.blocks.payloads.PayloadConveyor.PayloadConveyorBuild;
import mindustry.world.blocks.payloads.PayloadMassDriver.PayloadDriverBuild;
import mindustry.world.blocks.payloads.PayloadLoader.PayloadLoaderBuild;
import mindustry.world.blocks.payloads.PayloadDeconstructor.PayloadDeconstructorBuild;
import mindustry.world.blocks.payloads.Constructor.ConstructorBuild;
import mindustry.content.Blocks;
import mindustry.game.Team;
import mindustry.input.DesktopInput;
import mindustry.io.TypeIO;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.NetworkIO;

/**
 * Decodes post-join fixtures emitted by Rust with the exact Mindustry 158 API.
 *
 * Generate fixtures with:
 * cargo test --lib export_desktop_158_post_join_fixtures -- --ignored
 */
public final class VerifyProtocol158 {
    private static <T> T allocateWithoutConstructor(Class<T> type) throws Exception {
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe)field.get(null);
        return type.cast(unsafe.allocateInstance(type));
    }

    private static Reads reads(byte[] data) {
        return Reads.get(new DataInputStream(new ByteArrayInputStream(data)));
    }

    private static void verifyInvalidConstructFinishConfig(Path fixtureDir) throws Exception {
        // Regression: a legacy DynamicTile config may contain a Building tail
        // beginning with 127. Rust must sanitize it to one null TypeIO object;
        // otherwise handled() throws "Unknown object type: 127".
        byte[] bytes =
            Files.readAllBytes(fixtureDir.resolve("construct-finish-invalid-config.bin"));
        var packet = new ConstructFinishCallPacket();
        packet.read(reads(bytes), bytes.length);
        packet.handled();
        if (packet.tile == null
                || packet.tile.pos() != (41 << 16 | 101)
                || packet.block == null
                || packet.block.id != 216
                || packet.builder == null
                || packet.builder.id != 3_100_021
                || packet.config != null) {
            throw new AssertionError("ConstructFinish invalid config was not sanitized");
        }
    }

    public static void main(String[] args) throws Exception {
        Path fixtureDir = Path.of(args.length == 0 || args[0].startsWith("--")
            ? "target/protocol-158-fixtures"
            : args[0]);
        boolean constructOnly = java.util.Arrays.asList(args).contains("--construct-only");

        Vars.content = new ContentLoader();
        Core.settings = new Settings();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.content.init();
        Vars.world = new World();
        Vars.player = Player.create();
        Vars.control = allocateWithoutConstructor(Control.class);
        Vars.control.input = allocateWithoutConstructor(DesktopInput.class);
        Vars.net = new Net(new Net.NetProvider() {
            public void connectClient(String ip, int port, Runnable success) {}
            public void sendClient(Object object, boolean reliable) {}
            public void disconnectClient() {}
            public void discoverServers(arc.func.Cons<Host> found, Runnable done) { done.run(); }
            public void pingHost(
                    String address, int port, arc.func.Cons<Host> valid,
                    arc.func.Cons<Exception> failed) {}
            public void hostServer(int port) {}
            public Iterable<? extends NetConnection> getConnections() {
                return java.util.List.of();
            }
            public void closeServer() {}
        });
        Vars.netClient = new NetClient();
        Vars.net.setClientConnected();
        Vars.customMapDirectory = new Fi("/tmp");
        Groups.init();

        try (var world = new InflaterInputStream(
                new FileInputStream(fixtureDir.resolve("world.bin").toFile()))) {
            NetworkIO.loadWorld(world);
        }

        Vars.headless = true;
        var constructTile = Vars.world.tile(102, 182);
        constructTile.setBlock(Vars.content.block(5), Team.sharded, 3);
        var constructBuild = (ConstructBuild)constructTile.build;
        constructBuild.current = Vars.content.block(270);
        constructBuild.previous = Blocks.air;
        constructBuild.health = 1f;
        constructBuild.rotation = 0;
        byte[] constructSnapshotBytes =
            Files.readAllBytes(fixtureDir.resolve("construct-snapshot.bin"));
        var constructSnapshot = new BlockSnapshotCallPacket();
        constructSnapshot.read(reads(constructSnapshotBytes), constructSnapshotBytes.length);
        constructSnapshot.handled();
        if (constructTile.block() != Vars.content.block(5)
                || constructTile.build != constructBuild
                || constructBuild.current.id != 270) {
            throw new AssertionError("ConstructBuild BlockSnapshot mismatch: block="
                + constructTile.block().id + " same=" + (constructTile.build == constructBuild)
                + " current=" + constructBuild.current.id + " health=" + constructBuild.health);
        }

        byte[] spawnBytes = Files.readAllBytes(fixtureDir.resolve("spawn.bin"));
        var spawn = new PlayerSpawnCallPacket();
        spawn.read(reads(spawnBytes), spawnBytes.length);
        spawn.handled();
        if (spawn.tile == null || spawn.player == null) {
            throw new AssertionError("PlayerSpawn decoded a null tile or player");
        }

        byte[] snapshotBytes = Files.readAllBytes(fixtureDir.resolve("entities.bin"));
        var snapshotIn = new DataInputStream(new ByteArrayInputStream(snapshotBytes));
        int amount = snapshotIn.readShort();
        int dataLength = snapshotIn.readUnsignedShort();
        byte[] entityData = snapshotIn.readNBytes(dataLength);
        if (entityData.length != dataLength || snapshotIn.available() != 0) {
            throw new AssertionError("Invalid EntitySnapshot TypeIO byte envelope");
        }
        var entityIn = new DataInputStream(new ByteArrayInputStream(entityData));
        var entityReads = Reads.get(entityIn);
        for (int i = 0; i < amount; i++) {
            NetClient.readSyncEntity(entityIn, entityReads);
        }
        if (entityIn.available() != 0) {
            throw new AssertionError(
                "EntitySnapshot left " + entityIn.available() + " unread bytes");
        }
        if (Vars.player.unit().item() == null || Vars.player.unit().stack().amount != 0) {
            throw new AssertionError("Player snapshot decoded an unsafe empty ItemStack");
        }

        Vars.state.rules.unitCap = 1000;
        byte[] unitSpawnBytes =
            Files.readAllBytes(fixtureDir.resolve("unit-spawns.bin"));
        var unitSpawnIn = new DataInputStream(new ByteArrayInputStream(unitSpawnBytes));
        int unitAmount = unitSpawnIn.readUnsignedShort();
        if (unitAmount != 35) {
            throw new AssertionError("Expected 35 Serpulo unit fixtures, got " + unitAmount);
        }
        for (int type = 0; type < unitAmount; type++) {
            int length = unitSpawnIn.readUnsignedShort();
            byte[] payload = unitSpawnIn.readNBytes(length);
            var payloadIn = new DataInputStream(new ByteArrayInputStream(payload));
            var decodedUnit = TypeIO.readUnitContainer(Reads.get(payloadIn));
            if (decodedUnit != null && decodedUnit.unit != null
                    && (decodedUnit.unit.item() == null || decodedUnit.unit.stack().amount != 0)) {
                throw new AssertionError(
                    "Unit type " + type + " decoded an unsafe empty ItemStack");
            }
            if (payloadIn.available() != 0) {
                throw new AssertionError(
                    "Unit type " + type + " left " + payloadIn.available()
                        + " unread sync bytes");
            }
            var unit = Groups.unit.getByID(3_100_000 + type);
            if (unit == null || unit.type == null || unit.type.id != type) {
                throw new AssertionError("Unit type " + type + " was not created correctly");
            }
            if (unit.item() == null || unit.stack.amount != 0) {
                throw new AssertionError(
                    "Unit type " + type + " decoded an unsafe empty item stack");
            }
            if (type == 5 && unit.elevation != 1f) {
                throw new AssertionError("Boosted Nova elevation did not survive sync");
            }
            if (type == 21) {
                if (!unit.updateBuilding() || unit.plans().size != 1) {
                    throw new AssertionError("Assist Poly build plan was not synchronized");
                }
                var plan = unit.plans().first();
                if (plan == null || plan.breaking || plan.x != 41 || plan.y != 100
                        || plan.block == null || plan.block.id != 216
                        || plan.rotation != 0 || plan.config != null) {
                    throw new AssertionError(
                        "Assist Poly BuildPlan payload mismatch: plan=" + plan
                            + " x=" + (plan == null ? -1 : plan.x)
                            + " y=" + (plan == null ? -1 : plan.y)
                            + " block=" + (plan == null || plan.block == null
                                ? -1 : plan.block.id)
                            + " rotation=" + (plan == null ? -1 : plan.rotation)
                            + " config=" + (plan == null ? null : plan.config));
                }
            }
            if (type == 22) {
                if (!(unit instanceof Payloadc carrier) || carrier.payloads().size != 2
                        || !(carrier.payloads().first() instanceof UnitPayload carried)
                        || carried.unit == null
                        || carried.unit.type == null || carried.unit.type.id != 0) {
                    throw new AssertionError("Mega nested unit payload was not synchronized");
                }
                if (!(carrier.payloads().get(1) instanceof BuildPayload building)
                        || building.build == null || building.build.block == null
                        || building.build.block.id != 216 || building.build.team.id != 1) {
                    throw new AssertionError("Mega nested BuildPayload was not synchronized");
                }
            }
        }
        if (unitSpawnIn.available() != 0) {
            throw new AssertionError("Trailing all-unit fixture bytes");
        }
        verifyInvalidConstructFinishConfig(fixtureDir);
        if (constructOnly) {
            System.out.println("ConstructFinish TypeIO regression: OK");
            return;
        }

        byte[] conveyorBytes =
            Files.readAllBytes(fixtureDir.resolve("payload-conveyor.bin"));
        var conveyor = (PayloadConveyorBuild)Blocks.payloadConveyor
            .newBuilding().create(Blocks.payloadConveyor, Team.sharded);
        var conveyorInput = new DataInputStream(new ByteArrayInputStream(conveyorBytes));
        conveyor.readSync(Reads.get(conveyorInput), (byte)0);
        if (conveyorInput.available() != 0 || conveyor.itemRotation != 180f
                || !(conveyor.item instanceof BuildPayload conveyorPayload)
                || conveyorPayload.build == null || conveyorPayload.build.block.id != 216
                || conveyorPayload.build.team.id != 1) {
            throw new AssertionError("PayloadConveyor sync mismatch");
        }

        byte[] driverBytes =
            Files.readAllBytes(fixtureDir.resolve("payload-mass-driver.bin"));
        var driver = (PayloadDriverBuild)Blocks.payloadMassDriver
            .newBuilding().create(Blocks.payloadMassDriver, Team.sharded);
        var driverInput = new DataInputStream(new ByteArrayInputStream(driverBytes));
        driver.readSync(Reads.get(driverInput), (byte)1);
        if (driverInput.available() != 0 || driver.link != -1
                || driver.turretRotation != 135f || driver.charge != 45f
                || !driver.loaded || !driver.charging
                || !(driver.payload instanceof BuildPayload driverPayload)
                || driverPayload.build == null || driverPayload.build.block.id != 216
                || driverPayload.build.team.id != 1) {
            throw new AssertionError("PayloadMassDriver sync mismatch");
        }

        byte[] loaderBytes = Files.readAllBytes(fixtureDir.resolve("payload-loader.bin"));
        var loader = (PayloadLoaderBuild)Blocks.payloadLoader
            .newBuilding().create(Blocks.payloadLoader, Team.sharded);
        var loaderInput = new DataInputStream(new ByteArrayInputStream(loaderBytes));
        loader.readSync(Reads.get(loaderInput), (byte)1);
        if (loaderInput.available() != 0 || !loader.exporting
                || loader.items.get(Vars.content.item(0)) != 12
                || loader.liquids.get(Vars.content.liquid(0)) != 20f
                || loader.payload == null
                || loader.payload.build == null || loader.payload.build.block.id != 345
                || loader.payload.build.items.get(Vars.content.item(9)) != 7) {
            throw new AssertionError("PayloadLoader sync mismatch");
        }

        byte[] deconstructorBytes =
            Files.readAllBytes(fixtureDir.resolve("payload-deconstructor.bin"));
        var deconstructor = (PayloadDeconstructorBuild)Blocks.smallDeconstructor
            .newBuilding().create(Blocks.smallDeconstructor, Team.sharded);
        var deconstructorInput =
            new DataInputStream(new ByteArrayInputStream(deconstructorBytes));
        deconstructor.readSync(Reads.get(deconstructorInput), (byte)0);
        if (deconstructorInput.available() != 0 || deconstructor.payload != null
                || deconstructor.progress != 0.5f || deconstructor.accum == null
                || deconstructor.accum.length != 1 || deconstructor.accum[0] != 0.5f
                || deconstructor.items.get(Vars.content.item(0)) != 2
                || !(deconstructor.deconstructing instanceof BuildPayload deconstructed)
                || deconstructed.build == null || deconstructed.build.block.id != 216) {
            throw new AssertionError("PayloadDeconstructor sync mismatch");
        }

        byte[] constructorBytes =
            Files.readAllBytes(fixtureDir.resolve("payload-constructor.bin"));
        var constructor = (ConstructorBuild)Blocks.constructor
            .newBuilding().create(Blocks.constructor, Team.sharded);
        var constructorInput =
            new DataInputStream(new ByteArrayInputStream(constructorBytes));
        constructor.readSync(Reads.get(constructorInput), (byte)0);
        if (constructorInput.available() != 0 || constructor.payload != null
                || constructor.progress != 72f || constructor.recipe == null
                || constructor.recipe.id != 236
                || constructor.items.get(Vars.content.item(16)) != 24) {
            throw new AssertionError("Constructor sync mismatch");
        }

        var largeCodecInput = new DataInputStream(new ByteArrayInputStream(
            Files.readAllBytes(fixtureDir.resolve("large-constructor-codecs.bin"))));
        int largeCodecCount = largeCodecInput.readUnsignedShort();
        for (int index = 0; index < largeCodecCount; index++) {
            int blockId = largeCodecInput.readShort();
            byte revision = largeCodecInput.readByte();
            int length = largeCodecInput.readInt();
            byte[] sync = largeCodecInput.readNBytes(length);
            if (sync.length != length) throw new AssertionError("truncated codec " + blockId);
            var block = Vars.content.block(blockId);
            var build = block.newBuilding().create(block, Team.sharded);
            var syncInput = new DataInputStream(new ByteArrayInputStream(sync));
            build.readSync(Reads.get(syncInput), revision);
            if (syncInput.available() != 0) {
                throw new AssertionError("trailing codec bytes block=" + blockId
                    + " revision=" + revision + " remaining=" + syncInput.available());
            }
        }
        if (largeCodecInput.available() != 0 || largeCodecCount != 49) {
            throw new AssertionError("large constructor codec batch mismatch");
        }

        byte[] stateBytes = Files.readAllBytes(fixtureDir.resolve("state.bin"));
        var state = new StateSnapshotCallPacket();
        state.read(reads(stateBytes), stateBytes.length);
        state.handled();

        byte[] commandBytes =
            Files.readAllBytes(fixtureDir.resolve("command-building.bin"));
        var command = new CommandBuildingCallPacket();
        command.read(reads(commandBytes), commandBytes.length);
        command.handled();
        if (command.player == null || command.player.id != Vars.player.id
                || command.buildings.length != 1
                || command.buildings[0] != (40 << 16 | 100)
                || command.target.x != 512.5f
                || command.target.y != 704.25f) {
            throw new AssertionError("CommandBuilding server-forward payload mismatch");
        }
        byte[] commandUnitsBytes =
            Files.readAllBytes(fixtureDir.resolve("command-units.bin"));
        var commandUnits = new CommandUnitsCallPacket();
        commandUnits.read(reads(commandUnitsBytes), commandUnitsBytes.length);
        commandUnits.handled();
        if (commandUnits.player == null || commandUnits.player.id != Vars.player.id
                || commandUnits.unitIds.length != 1
                || commandUnits.unitIds[0] != 3_100_000
                || commandUnits.buildTarget != null
                || commandUnits.unitTarget == null
                || commandUnits.unitTarget.id != 3_100_001
                || commandUnits.queueCommand
                || !commandUnits.finalBatch) {
            throw new AssertionError("CommandUnits server-forward payload mismatch");
        }
        byte[] setCommandBytes =
            Files.readAllBytes(fixtureDir.resolve("set-unit-command.bin"));
        var setCommand = new SetUnitCommandCallPacket();
        setCommand.read(reads(setCommandBytes), setCommandBytes.length);
        setCommand.handled();
        if (setCommand.player == null || setCommand.player.id != Vars.player.id
                || setCommand.unitIds.length != 1
                || setCommand.unitIds[0] != 3_100_020
                || setCommand.command == null
                || setCommand.command.id != 4) {
            throw new AssertionError("SetUnitCommand server-forward payload mismatch");
        }
        byte[] setStanceBytes =
            Files.readAllBytes(fixtureDir.resolve("set-unit-stance.bin"));
        var setStance = new SetUnitStanceCallPacket();
        setStance.read(reads(setStanceBytes), setStanceBytes.length);
        setStance.handled();
        if (setStance.player == null || setStance.player.id != Vars.player.id
                || setStance.unitIds.length != 1
                || setStance.unitIds[0] != 3_100_000
                || setStance.stance == null
                || setStance.stance.id != 1
                || !setStance.enable) {
            throw new AssertionError("SetUnitStance server-forward payload mismatch");
        }
        byte[] constructFinishBytes =
            Files.readAllBytes(fixtureDir.resolve("construct-finish-builder.bin"));
        var constructFinish = new ConstructFinishCallPacket();
        constructFinish.read(reads(constructFinishBytes), constructFinishBytes.length);
        constructFinish.handled();
        if (constructFinish.tile == null
                || constructFinish.tile.pos() != (41 << 16 | 100)
                || constructFinish.block == null
                || constructFinish.block.id != 216
                || constructFinish.builder == null
                || constructFinish.builder.id != 3_100_021
                || constructFinish.rotation != 3
                || constructFinish.team == null
                || constructFinish.team.id != 1
                || constructFinish.config != null) {
            throw new AssertionError("Builder ConstructFinish payload mismatch");
        }
        byte[] enteredBytes =
            Files.readAllBytes(fixtureDir.resolve("unit-entered-payload.bin"));
        var entered = new UnitEnteredPayloadCallPacket();
        entered.read(reads(enteredBytes), enteredBytes.length);
        entered.handled();
        if (entered.unit == null || entered.unit.id != 3_100_000
                || entered.build == null || entered.build.pos() != (40 << 16 | 100)) {
            throw new AssertionError("UnitEnteredPayload payload mismatch");
        }
        byte[] pickedBuildBytes =
            Files.readAllBytes(fixtureDir.resolve("picked-build-payload.bin"));
        var pickedBuild = new PickedBuildPayloadCallPacket();
        pickedBuild.read(reads(pickedBuildBytes), pickedBuildBytes.length);
        pickedBuild.handled();
        if (pickedBuild.unit == null || pickedBuild.unit.id != 3_100_022
                || pickedBuild.build == null
                || pickedBuild.build.pos() != (40 << 16 | 100)
                || !pickedBuild.onGround) {
            throw new AssertionError("PickedBuildPayload payload mismatch");
        }

        var deathProbe = Groups.unit.getByID(3_100_034);
        if (deathProbe == null || deathProbe.item() == null) {
            throw new AssertionError("Death probe unit has a null item");
        }
        Vars.headless = true;
        Core.app = new MockApplication();
        Core.audio = new MockAudio();
        Units.unitDeath(deathProbe.id);
        if (Groups.unit.getByID(3_100_034) != null) {
            throw new AssertionError("UnitDeath did not remove the death probe");
        }

        System.out.println(
            "ok map=" + Vars.world.width() + "x" + Vars.world.height()
                + " player=" + Vars.player.name
                + " playerId=" + Vars.player.id
                + " unit=" + Vars.player.unit().getClass().getSimpleName()
                + " allSerpuloUnits=" + unitAmount
                + " deathStackSafe=true"
                + " constructSnapshot=true"
                + " boostedNova=true"
                + " assistBuildPlan=true"
                + " nestedUnitPayload=true"
                + " nestedBuildPayload=true"
                + " payloadConveyor=true"
                + " payloadMassDriver=true"
                + " payloadLoader=true"
                + " payloadDeconstructor=true"
                + " payloadConstructor=true"
                + " largeConstructorCodecs=" + largeCodecCount
                + " commandBuilding=true"
                + " commandUnits=true"
                + " setUnitCommand=true"
                + " setUnitStance=true"
                + " builderConstructFinish=true"
                + " unitEnteredPayload=true"
                + " pickedBuildPayload=true"
                + " wave=" + state.wave
                + " entities=" + amount);
    }
}
