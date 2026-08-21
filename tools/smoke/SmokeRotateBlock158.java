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
import mindustry.gen.RequestBlockSnapshotCallPacket;
import mindustry.gen.RotateBlockCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.blocks.distribution.Sorter.SorterBuild;

/** Rotates a live building through exact desktop 158.1 packets and verifies state. */
public final class SmokeRotateBlock158 {
    private static final int POSITION = (45 << 16) | 100;
    private static final Field ROTATE_DATA;

    static {
        try {
            ROTATE_DATA = RotateBlockCallPacket.class.getDeclaredField("DATA");
            ROTATE_DATA.setAccessible(true);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
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
        int port = args.length == 0 ? 6583 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.net = clientNet();
        Groups.init();
        Vars.world.resize(128, 128).fill();
        Vars.world.tile(45, 100).setBlock(Blocks.sorter, Team.sharded, 0);

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicInteger snapshots = new AtomicInteger();
        AtomicBoolean rotateEcho = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(1);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "rotate-block-158";
                packet.locale = "en";
                packet.uuid = "MTIzNDU2Nzg=";
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
                    } else if (object instanceof BlockSnapshotCallPacket packet) {
                        packet.handled();
                        int seen = snapshots.incrementAndGet();
                        verifySnapshot(packet, seen == 1 ? 0 : 1);
                        if (seen == 1) {
                            RotateBlockCallPacket rotate = new RotateBlockCallPacket();
                            rotate.build = Vars.world.tile(45, 100).build;
                            rotate.direction = true;
                            connection.sendTCP(rotate);
                        } else if (rotateEcho.get()) {
                            verified.countDown();
                        }
                    } else if (object instanceof RotateBlockCallPacket packet) {
                        verifyEcho(packet);
                        rotateEcho.set(true);
                        RequestBlockSnapshotCallPacket request =
                            new RequestBlockSnapshotCallPacket();
                        request.pos = POSITION;
                        connection.sendTCP(request);
                    }
                } catch (Throwable error) {
                    error.printStackTrace();
                    verified.countDown();
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (verified.getCount() != 0) {
                    System.err.println("disconnected before rotation verification: " + reason);
                    verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(10, TimeUnit.SECONDS)) {
                throw new AssertionError("timed out waiting for rotated snapshot");
            }
            if (!client.isConnected() || snapshots.get() < 2 || !rotateEcho.get()) {
                throw new AssertionError(
                    "rotation path failed: connected=" + client.isConnected()
                        + " snapshots=" + snapshots.get() + " echo=" + rotateEcho.get());
            }
            Thread.sleep(1200);
            System.out.println(
                "ok rotateBlock=true direction=true rotatedSnapshot=1 persisted=true");
        } finally {
            client.stop();
        }
    }

    private static void verifyEcho(RotateBlockCallPacket packet) throws Exception {
        byte[] raw = (byte[])ROTATE_DATA.get(packet);
        ByteArrayInputStream bytes = new ByteArrayInputStream(raw);
        DataInputStream input = new DataInputStream(bytes);
        // Consume the player entity id. Arc `Server.generateId` is
        // `Rand.nextInt()` (any i32 except 0); this port uses
        // `1_000_000.wrapping_add(connection_id)`, so the id is often
        // negative. Requiring playerId > 0 is false at baseline and flakes
        // the smoke. The layout is still i32 player + i32 pos + bool.
        int playerId = input.readInt();
        int position = input.readInt();
        boolean direction = input.readBoolean();
        if (position != POSITION || !direction || bytes.available() != 0) {
            throw new AssertionError(
                "invalid RotateBlock echo: player=" + playerId + " position=" + position
                    + " direction=" + direction + " remaining=" + bytes.available());
        }
    }

    private static void verifySnapshot(BlockSnapshotCallPacket packet, int rotation)
            throws Exception {
        if (packet.amount != 1) {
            throw new AssertionError("expected one sorter snapshot, got " + packet.amount);
        }
        ByteArrayInputStream bytes = new ByteArrayInputStream(packet.data);
        DataInputStream input = new DataInputStream(bytes);
        int position = input.readInt();
        int block = input.readShort();
        SorterBuild build = (SorterBuild)Vars.world.tile(45, 100).build;
        build.readSync(Reads.get(input), build.version());
        if (position != POSITION || block != Blocks.sorter.id || build.rotation != rotation
                || bytes.available() != 0) {
            throw new AssertionError(
                "rotated snapshot mismatch: position=" + position + " block=" + block
                    + " rotation=" + build.rotation + " expected=" + rotation
                    + " remaining=" + bytes.available());
        }
    }
}
