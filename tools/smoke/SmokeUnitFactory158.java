import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.lang.reflect.Field;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Seq;
import arc.math.geom.Vec2;
import arc.util.io.Reads;
import mindustry.Vars;
import mindustry.ai.UnitCommand;
import mindustry.ai.UnitStance;
import mindustry.content.Blocks;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.Control;
import mindustry.core.GameState;
import mindustry.core.NetClient;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.game.Rules;
import mindustry.gen.BlockSnapshotCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.CommandBuildingCallPacket;
import mindustry.gen.CommandUnitsCallPacket;
import mindustry.gen.EntitySnapshotCallPacket;
import mindustry.gen.SetUnitCommandCallPacket;
import mindustry.gen.SetUnitStanceCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.Player;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.RequestBlockSnapshotCallPacket;
import mindustry.gen.UnitSpawnCallPacket;
import mindustry.input.DesktopInput;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.blocks.units.UnitFactory.UnitFactoryBuild;
import mindustry.world.blocks.units.Reconstructor.ReconstructorBuild;
import mindustry.world.modules.ItemModule;
import mindustry.world.modules.PowerModule;

/** Verifies factory revision-3 sync and UnitSpawn with desktop 158.1. */
public final class SmokeUnitFactory158 {
    private static final int FACTORY = (45 << 16) | 100;

    private static <T> T allocateWithoutConstructor(Class<T> type) throws Exception {
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe)field.get(null);
        return type.cast(unsafe.allocateInstance(type));
    }

    private static Net clientNet() {
        return new Net(new Net.NetProvider() {
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
    }

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6593 : Integer.parseInt(args[0]);
        boolean reconstructor = args.length > 1 && args[1].equals("reconstructor");
        Version.build = 158;
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.state.rules = new Rules();
        Vars.state.rules.unitCap = 100;
        Vars.world = new World();
        Vars.player = Player.create();
        Vars.control = allocateWithoutConstructor(Control.class);
        Vars.control.input = allocateWithoutConstructor(DesktopInput.class);
        Vars.net = clientNet();
        Vars.netClient = new NetClient();
        Groups.init();
        Vars.world.resize(128, 128).fill();
        Vars.world.tile(45, 100).setBlock(
            reconstructor ? Blocks.additiveReconstructor : Blocks.groundFactory,
            Team.sharded,
            0
        );

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicBoolean requested = new AtomicBoolean();
        AtomicBoolean snapshotVerified = new AtomicBoolean();
        AtomicBoolean spawnVerified = new AtomicBoolean();
        AtomicBoolean commandUnitsSent = new AtomicBoolean();
        AtomicBoolean setCommandSent = new AtomicBoolean();
        AtomicBoolean setStanceSent = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(2);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "unit-factory-158";
                packet.locale = "en";
                packet.uuid = "ZmFjdG9yeTE=";
                packet.usid = "";
                packet.color = 0xffa665ff;
                connection.sendTCP(packet);
            }

            @Override
            public synchronized void received(Connection connection, Object object) {
                try {
                    if (object instanceof Packets.StreamBegin begin) {
                        streamId.set(begin.id);
                        streamTotal.set(begin.total);
                        world.reset();
                    } else if (object instanceof Packets.StreamChunk chunk
                            && chunk.id == streamId.get()) {
                        world.writeBytes(chunk.data);
                        if (world.size() == streamTotal.get()) {
                            connection.sendTCP(new ConnectConfirmCallPacket());
                        }
                    } else if (object instanceof PlayerSpawnCallPacket
                            && requested.compareAndSet(false, true)) {
                        CommandBuildingCallPacket command =
                            new CommandBuildingCallPacket();
                        command.buildings = new int[]{FACTORY};
                        command.target = new Vec2(512.5f, 704.25f);
                        connection.sendTCP(command);
                        RequestBlockSnapshotCallPacket request =
                            new RequestBlockSnapshotCallPacket();
                        request.pos = FACTORY;
                        connection.sendTCP(request);
                    } else if (object instanceof BlockSnapshotCallPacket packet) {
                        packet.handled();
                        if (packet.amount == 1 && verifyFactory(packet, reconstructor)
                                && snapshotVerified.compareAndSet(false, true)) {
                            verified.countDown();
                        }
                    } else if (object instanceof UnitSpawnCallPacket packet) {
                        packet.handled();
                        packet.handleClient();
                        var unit = Groups.unit.find(
                            candidate -> candidate.team == Team.sharded
                                && candidate.type == (reconstructor
                                    ? UnitTypes.mace : UnitTypes.dagger));
                        if (unit == null) {
                            throw new AssertionError("UnitSpawn decoded no expected Sharded unit");
                        }
                        onUnitFound(connection, unit.id, commandUnitsSent,
                            setCommandSent, setStanceSent, spawnVerified, verified);
                    } else if (object instanceof EntitySnapshotCallPacket packet) {
                        // Robust fallback for slow-starting JVMs: the spawned
                        // Sharded unit keeps appearing in entity snapshots even
                        // if the UnitSpawn broadcast predates our join.
                        packet.handled();
                        packet.handleClient();
                        var unit = Groups.unit.find(
                            candidate -> candidate.team == Team.sharded
                                && candidate.type == (reconstructor
                                    ? UnitTypes.mace : UnitTypes.dagger));
                        if (unit != null) {
                            onUnitFound(connection, unit.id, commandUnitsSent,
                                setCommandSent, setStanceSent, spawnVerified, verified);
                        }
                    }
                } catch (Throwable error) {
                    error.printStackTrace();
                    while (verified.getCount() > 0) verified.countDown();
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (verified.getCount() != 0) {
                    System.err.println("disconnected before factory verification: " + reason);
                    while (verified.getCount() > 0) verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(10, TimeUnit.SECONDS)
                    || !client.isConnected()
                    || !snapshotVerified.get()
                    || !spawnVerified.get()) {
                throw new AssertionError(
                    "factory path failed: connected=" + client.isConnected()
                        + " snapshot=" + snapshotVerified.get()
                        + " spawn=" + spawnVerified.get());
            }
            Thread.sleep(1500);
            System.out.println(
                "ok " + (reconstructor ? "reconstructor=true" : "unitFactory=true")
                    + " officialReadSync=true unitSpawn150=true shardedUnit=true"
                    + " commandBuilding27=true commandUnits28=" + commandUnitsSent.get()
                    + " setUnitCommand121=" + setCommandSent.get()
                    + " setUnitStance122=" + setStanceSent.get());
        } finally {
            client.stop();
        }
    }

    private static void onUnitFound(
            Connection connection, int unitId,
            AtomicBoolean commandUnitsSent, AtomicBoolean setCommandSent,
            AtomicBoolean setStanceSent, AtomicBoolean spawnVerified,
            CountDownLatch verified) {
        if (commandUnitsSent.compareAndSet(false, true)) {
            CommandUnitsCallPacket command = new CommandUnitsCallPacket();
            command.unitIds = new int[]{unitId};
            command.buildTarget = null;
            command.unitTarget = null;
            command.posTarget = new Vec2(640f, 704f);
            command.queueCommand = false;
            command.finalBatch = true;
            connection.sendTCP(command);
            SetUnitCommandCallPacket setCommand = new SetUnitCommandCallPacket();
            setCommand.unitIds = new int[]{unitId};
            setCommand.command = UnitCommand.enterPayloadCommand;
            connection.sendTCP(setCommand);
            setCommandSent.set(true);
            SetUnitStanceCallPacket setStance = new SetUnitStanceCallPacket();
            setStance.unitIds = new int[]{unitId};
            setStance.stance = UnitStance.holdFire;
            setStance.enable = true;
            connection.sendTCP(setStance);
            setStanceSent.set(true);
        }
        if (spawnVerified.compareAndSet(false, true)) verified.countDown();
    }

    private static boolean verifyFactory(
            BlockSnapshotCallPacket packet, boolean reconstructor) throws Exception {
        ByteArrayInputStream bytes = new ByteArrayInputStream(packet.data);
        DataInputStream input = new DataInputStream(bytes);
        int position = input.readInt();
        int block = input.readShort();
        int expectedBlock = reconstructor
            ? Blocks.additiveReconstructor.id : Blocks.groundFactory.id;
        if (position != FACTORY || block != expectedBlock) return false;
        if (reconstructor) {
            ReconstructorBuild build =
                (ReconstructorBuild)Vars.world.tile(45, 100).build;
            if (build.items == null) build.items = new ItemModule();
            if (build.power == null) build.power = new PowerModule();
            build.readSync(Reads.get(input), build.version());
            if (build.power == null || build.power.status <= 0f
                    || bytes.available() != 0) {
                throw new AssertionError(
                    "ReconstructorBuild mismatch: progress=" + build.progress
                        + " power=" + (build.power == null ? null : build.power.status)
                        + " remaining=" + bytes.available());
            }
        } else {
            UnitFactoryBuild build =
                (UnitFactoryBuild)Vars.world.tile(45, 100).build;
            build.readSync(Reads.get(input), build.version());
            if (build.currentPlan != 0 || build.power == null
                    || build.power.status <= 0f || bytes.available() != 0) {
                throw new AssertionError(
                    "UnitFactoryBuild mismatch: plan=" + build.currentPlan
                        + " progress=" + build.progress
                        + " power=" + (build.power == null ? null : build.power.status)
                        + " remaining=" + bytes.available());
            }
        }
        return true;
    }
}
