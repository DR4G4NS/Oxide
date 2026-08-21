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
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.RequestBlockSnapshotCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.blocks.defense.OverdriveProjector.OverdriveBuild;

/** Verifies OverdriveBuild sync and an accelerated production window on desktop 158.1. */
public final class SmokeOverdrive158 {
    private static final int PROJECTOR = (50 << 16) | 100;

    private static Net clientNet() {
        return new Net(new Net.NetProvider() {
            public void connectClient(String ip, int port, Runnable success) {}
            public void sendClient(Object object, boolean reliable) {}
            public void disconnectClient() {}
            public void discoverServers(arc.func.Cons<Host> found, Runnable done) { done.run(); }
            public void pingHost(String address, int port, arc.func.Cons<Host> valid,
                                 arc.func.Cons<Exception> failed) {}
            public void hostServer(int port) {}
            public Iterable<? extends NetConnection> getConnections() {
                return java.util.List.of();
            }
            public void closeServer() {}
        });
    }

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6587 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.net = clientNet();
        Groups.init();
        Vars.world.resize(128, 128).fill();
        Vars.world.tile(50, 100).setBlock(Blocks.overdriveProjector, Team.sharded, 0);

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicBoolean requested = new AtomicBoolean();
        AtomicBoolean snapshotVerified = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(1);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "overdrive-158";
                packet.locale = "en";
                packet.uuid = "b3ZlcmRyaXY=";
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
                        request.pos = PROJECTOR;
                        connection.sendTCP(request);
                    } else if (object instanceof BlockSnapshotCallPacket packet) {
                        packet.handled();
                        if (packet.amount == 1 && verifyProjector(packet)
                                && snapshotVerified.compareAndSet(false, true)) {
                            verified.countDown();
                        }
                    }
                } catch (Throwable error) {
                    error.printStackTrace();
                    verified.countDown();
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (!snapshotVerified.get()) {
                    System.err.println("disconnected before Overdrive verification: " + reason);
                    verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(5, TimeUnit.SECONDS) || !snapshotVerified.get()) {
                throw new AssertionError("Overdrive snapshot was not verified");
            }
            Thread.sleep(1200);
            if (!client.isConnected()) throw new AssertionError("client disconnected");
            System.out.println("ok overdrive=true officialReadSync=true power=true");
        } finally {
            client.stop();
        }
    }

    private static boolean verifyProjector(BlockSnapshotCallPacket packet) throws Exception {
        ByteArrayInputStream bytes = new ByteArrayInputStream(packet.data);
        DataInputStream input = new DataInputStream(bytes);
        int position = input.readInt();
        int block = input.readShort();
        if (position != PROJECTOR || block != Blocks.overdriveProjector.id) return false;
        OverdriveBuild build = (OverdriveBuild)Vars.world.tile(50, 100).build;
        build.readSync(Reads.get(input), build.version());
        if (build.heat < 0.9f || build.phaseHeat != 0f
                || build.power == null || build.power.status < 0.99f
                || bytes.available() != 0) {
            throw new AssertionError(
                "OverdriveBuild mismatch: heat=" + build.heat
                    + " phaseHeat=" + build.phaseHeat
                    + " power=" + (build.power == null ? null : build.power.status)
                    + " remaining=" + bytes.available());
        }
        return true;
    }
}
