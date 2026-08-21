import java.io.ByteArrayOutputStream;
import java.lang.reflect.Field;
import java.nio.ByteBuffer;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Queue;
import arc.struct.Seq;
import mindustry.Vars;
import mindustry.core.Version;
import mindustry.gen.ClientSnapshotCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.EntitySnapshotCallPacket;
import mindustry.gen.PlayerDisconnectCallPacket;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.gen.UnitDespawnCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;

/** Two simultaneous real ArcNet clients compiled against desktop build 158.1. */
public final class SmokeMultiplayer158 {
    private static final Field ENTITY_PACKET_DATA;

    static {
        try {
            ENTITY_PACKET_DATA = EntitySnapshotCallPacket.class.getDeclaredField("DATA");
            ENTITY_PACKET_DATA.setAccessible(true);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    private static Integer firstUnitId(EntitySnapshotCallPacket packet) {
        try {
            byte[] payload = (byte[])ENTITY_PACKET_DATA.get(packet);
            if (payload.length < 8) return null;
            ByteBuffer input = ByteBuffer.wrap(payload);
            int amount = input.getShort();
            int dataLength = input.getShort() & 0xffff;
            if (amount <= 0 || dataLength < 5 || dataLength > input.remaining()) return null;
            return input.getInt();
        } catch (IllegalAccessException error) {
            throw new RuntimeException(error);
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

    private static ClientSnapshotCallPacket snapshot(Connection connection, int id, float x) {
        ClientSnapshotCallPacket packet = new ClientSnapshotCallPacket();
        packet.snapshotID = id;
        packet.unitID = 2_000_000 + connection.getID();
        packet.x = x;
        packet.y = 800f;
        packet.pointerX = x + 40f;
        packet.pointerY = 800f;
        packet.rotation = 0f;
        packet.baseRotation = 0f;
        packet.plans = new Queue<>();
        packet.viewX = x;
        packet.viewY = 800f;
        packet.viewWidth = 640f;
        packet.viewHeight = 480f;
        return packet;
    }

    private static final class Peer {
        final Client client =
            new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        final String name;
        final String uuid;
        final float x;
        final CountDownLatch joined = new CountDownLatch(2);
        final CountDownLatch peerRemoved = new CountDownLatch(2);
        final Set<Integer> observedUnits = ConcurrentHashMap.newKeySet();
        final ByteArrayOutputStream world = new ByteArrayOutputStream();
        final AtomicInteger streamId = new AtomicInteger(-1);
        final AtomicInteger streamTotal = new AtomicInteger(-1);
        final AtomicBoolean confirmed = new AtomicBoolean();
        volatile int connectionId;

        Peer(String name, String uuid, float x) {
            this.name = name;
            this.uuid = uuid;
            this.x = x;
            client.addListener(new NetListener() {
                @Override
                public void connected(Connection connection) {
                    connectionId = connection.getID();
                    Packets.ConnectPacket packet = new Packets.ConnectPacket();
                    packet.versionType = "official";
                    packet.mods = new Seq<>();
                    packet.name = name;
                    packet.locale = "en";
                    packet.uuid = uuid;
                    packet.usid = "";
                    packet.color = 0xffa665ff;
                    connection.sendTCP(packet);
                }

                @Override
                public synchronized void received(Connection connection, Object object) {
                    if (object instanceof Packets.StreamBegin begin) {
                        streamId.set(begin.id);
                        streamTotal.set(begin.total);
                        world.reset();
                    } else if (object instanceof Packets.StreamChunk chunk
                            && chunk.id == streamId.get()) {
                        world.writeBytes(chunk.data);
                        if (world.size() == streamTotal.get()
                                && confirmed.compareAndSet(false, true)) {
                            connection.sendTCP(new ConnectConfirmCallPacket());
                            connection.sendTCP(snapshot(connection, 1, x));
                        }
                    } else if (object instanceof PlayerSpawnCallPacket) {
                        joined.countDown();
                    } else if (object instanceof StateSnapshotCallPacket) {
                        joined.countDown();
                    } else if (object instanceof EntitySnapshotCallPacket entities) {
                        Integer unitId = firstUnitId(entities);
                        if (unitId != null) {
                            observedUnits.add(unitId);
                        }
                    } else if (object instanceof PlayerDisconnectCallPacket) {
                        peerRemoved.countDown();
                    } else if (object instanceof UnitDespawnCallPacket) {
                        peerRemoved.countDown();
                    }
                }

                @Override
                public void disconnected(Connection connection, DcReason reason) {
                    if (!confirmed.get()) {
                        System.err.println(name + " disconnected during join: " + reason);
                    }
                }
            });
        }

        int unitId() {
            return 2_000_000 + connectionId;
        }
    }

    private static boolean awaitUnits(Peer peer, int first, int second) throws Exception {
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(8);
        while (System.nanoTime() < deadline) {
            if (peer.observedUnits.contains(first) && peer.observedUnits.contains(second)) {
                return true;
            }
            Thread.sleep(50);
        }
        return false;
    }

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6579 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.net = clientNet();
        Peer alpha = new Peer("multi-alpha", "AQIDBAUGBwg=", 324f);
        Peer beta = new Peer("multi-beta", "CAcGBQQDAgE=", 328f);

        alpha.client.start();
        beta.client.start();
        try {
            alpha.client.connect(5000, "127.0.0.1", port, port);
            beta.client.connect(5000, "127.0.0.1", port, port);
            if (!alpha.joined.await(10, TimeUnit.SECONDS)
                    || !beta.joined.await(10, TimeUnit.SECONDS)) {
                throw new AssertionError("both clients did not complete post-join traffic");
            }
            if (!awaitUnits(alpha, alpha.unitId(), beta.unitId())
                    || !awaitUnits(beta, alpha.unitId(), beta.unitId())) {
                throw new AssertionError(
                    "cross replication failed: alpha=" + alpha.observedUnits
                        + " beta=" + beta.observedUnits);
            }

            Thread.sleep(1200);
            alpha.client.stop();
            if (!beta.peerRemoved.await(5, TimeUnit.SECONDS)) {
                throw new AssertionError(
                    "remaining peer missed disconnect/despawn packets: "
                        + beta.peerRemoved.getCount());
            }
            if (!beta.client.isConnected()) {
                throw new AssertionError("remaining peer disconnected unexpectedly");
            }
            System.out.println(
                "ok simultaneous=true alphaUnit=" + alpha.unitId()
                    + " betaUnit=" + beta.unitId()
                    + " crossSnapshots=true disconnect=true despawn=true");
        } finally {
            alpha.client.stop();
            beta.client.stop();
        }
    }
}
