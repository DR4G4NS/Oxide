import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Seq;
import arc.util.io.Reads;
import mindustry.Vars;
import mindustry.content.Blocks;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.BlockSnapshotCallPacket;
import mindustry.gen.BuildHealthUpdateCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.RequestBlockSnapshotCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.blocks.defense.MendProjector.MendBuild;

/** Verifies Mender healing and exact MendBuild sync against desktop 158.1. */
public final class SmokeMender158 {
    private static final int WALL = (44 << 16) | 100;
    private static final int MENDER = (45 << 16) | 100;

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
        int port = args.length == 0 ? 6586 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.net = clientNet();
        Groups.init();
        Vars.world.resize(128, 128).fill();
        Vars.world.tile(44, 100).setBlock(Blocks.copperWall, Team.sharded, 0);
        Vars.world.tile(45, 100).setBlock(Blocks.mender, Team.sharded, 0);
        Vars.world.tile(46, 100).setBlock(Blocks.solarPanel, Team.sharded, 0);

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicBoolean requested = new AtomicBoolean();
        AtomicBoolean snapshotVerified = new AtomicBoolean();
        AtomicBoolean healingVerified = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(2);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "mender-158";
                packet.locale = "en";
                packet.uuid = "YWJjZGVmZ2g=";
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
                        RequestBlockSnapshotCallPacket request =
                            new RequestBlockSnapshotCallPacket();
                        request.pos = MENDER;
                        connection.sendTCP(request);
                    } else if (object instanceof BlockSnapshotCallPacket packet) {
                        packet.handled();
                        if (packet.amount == 1 && verifyMender(packet)
                                && snapshotVerified.compareAndSet(false, true)) {
                            verified.countDown();
                        }
                    } else if (object instanceof BuildHealthUpdateCallPacket packet) {
                        packet.handled();
                        for (int i = 0; i + 1 < packet.buildings.size; i += 2) {
                            int position = packet.buildings.get(i);
                            float health = Float.intBitsToFloat(packet.buildings.get(i + 1));
                            if (position == WALL && health > 100f
                                    && healingVerified.compareAndSet(false, true)) {
                                verified.countDown();
                            }
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
                    System.err.println("disconnected before Mender verification: " + reason);
                    while (verified.getCount() > 0) verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(8, TimeUnit.SECONDS)
                    || !client.isConnected()
                    || !snapshotVerified.get()
                    || !healingVerified.get()) {
                throw new AssertionError(
                    "Mender path failed: connected=" + client.isConnected()
                        + " snapshot=" + snapshotVerified.get()
                        + " healing=" + healingVerified.get());
            }
            Thread.sleep(1200);
            System.out.println(
                "ok mender=true officialReadSync=true powerLimited=true"
                    + " buildHealthUpdate=true persistedHealing=true");
        } finally {
            client.stop();
        }
    }

    private static boolean verifyMender(BlockSnapshotCallPacket packet) throws Exception {
        ByteArrayInputStream bytes = new ByteArrayInputStream(packet.data);
        DataInputStream input = new DataInputStream(bytes);
        int position = input.readInt();
        int block = input.readShort();
        if (position != MENDER || block != Blocks.mender.id) return false;
        MendBuild build = (MendBuild)Vars.world.tile(45, 100).build;
        build.readSync(Reads.get(input), build.version());
        if (build.heat < 0.39f || build.heat > 0.41f || build.phaseHeat != 0f
                || build.power == null || build.power.status < 0.39f
                || build.power.status > 0.41f
                || bytes.available() != 0) {
            throw new AssertionError(
                "MendBuild mismatch: heat=" + build.heat
                    + " phaseHeat=" + build.phaseHeat
                    + " power=" + (build.power == null ? null : build.power.status)
                    + " remaining=" + bytes.available());
        }
        return true;
    }
}
