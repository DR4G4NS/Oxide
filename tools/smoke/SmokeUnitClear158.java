import java.lang.reflect.Field;
import java.nio.ByteBuffer;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Seq;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.core.Version;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.EntitySnapshotCallPacket;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.UnitClearCallPacket;
import mindustry.gen.UnitDespawnCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;

/** Requests a voluntary core-unit respawn through exact desktop 158.1 UnitClear. */
public final class SmokeUnitClear158 {
    private static final Field ENTITY_DATA;
    private static final Field DESPAWN_DATA;

    static {
        try {
            ENTITY_DATA = EntitySnapshotCallPacket.class.getDeclaredField("DATA");
            ENTITY_DATA.setAccessible(true);
            DESPAWN_DATA = UnitDespawnCallPacket.class.getDeclaredField("DATA");
            DESPAWN_DATA.setAccessible(true);
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

    private static Integer firstUnitId(EntitySnapshotCallPacket packet) throws Exception {
        byte[] payload = (byte[])ENTITY_DATA.get(packet);
        if (payload.length < 8) return null;
        ByteBuffer input = ByteBuffer.wrap(payload);
        int amount = input.getShort();
        int dataLength = input.getShort() & 0xffff;
        if (amount <= 0 || dataLength < 5 || dataLength > input.remaining()) return null;
        return input.getInt();
    }

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6585 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.net = clientNet();

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicInteger worldBytes = new AtomicInteger();
        AtomicInteger spawnCount = new AtomicInteger();
        AtomicInteger oldUnitId = new AtomicInteger(-1);
        AtomicInteger newUnitId = new AtomicInteger(-1);
        AtomicBoolean despawnVerified = new AtomicBoolean();
        AtomicBoolean newEntityVerified = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(3);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                oldUnitId.set(2_000_000 + connection.getID());
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "unit-clear-158";
                packet.locale = "en";
                packet.uuid = "UVJTVFVWV1g=";
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
                        worldBytes.set(0);
                    } else if (object instanceof Packets.StreamChunk chunk
                            && chunk.id == streamId.get()) {
                        int total = worldBytes.addAndGet(chunk.data.length);
                        if (total == streamTotal.get()) {
                            connection.sendTCP(new ConnectConfirmCallPacket());
                        }
                    } else if (object instanceof PlayerSpawnCallPacket) {
                        int count = spawnCount.incrementAndGet();
                        if (count == 1) {
                            connection.sendTCP(new UnitClearCallPacket());
                        } else if (count == 2) {
                            verified.countDown();
                        }
                    } else if (object instanceof UnitDespawnCallPacket packet) {
                        byte[] raw = (byte[])DESPAWN_DATA.get(packet);
                        ByteBuffer input = ByteBuffer.wrap(raw);
                        int kind = input.get() & 0xff;
                        int id = input.getInt();
                        if (kind != 2 || id != oldUnitId.get() || input.hasRemaining()) {
                            throw new AssertionError(
                                "invalid UnitDespawn: kind=" + kind + " id=" + id);
                        }
                        if (despawnVerified.compareAndSet(false, true)) verified.countDown();
                    } else if (object instanceof EntitySnapshotCallPacket packet) {
                        Integer id = firstUnitId(packet);
                        if (id != null && id >= 2_500_000) {
                            newUnitId.set(id);
                            if (newEntityVerified.compareAndSet(false, true)) verified.countDown();
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
                    System.err.println("disconnected before UnitClear verification: " + reason);
                    while (verified.getCount() > 0) verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(10, TimeUnit.SECONDS)
                    || !client.isConnected()
                    || spawnCount.get() < 2
                    || !despawnVerified.get()
                    || !newEntityVerified.get()) {
                throw new AssertionError(
                    "UnitClear path failed: connected=" + client.isConnected()
                        + " spawns=" + spawnCount.get()
                        + " despawn=" + despawnVerified.get()
                        + " newUnit=" + newUnitId.get());
            }
            Thread.sleep(1200);
            System.out.println(
                "ok unitClear=true oldUnit=" + oldUnitId.get()
                    + " despawn=true playerSpawn=true newUnit=" + newUnitId.get()
                    + " persisted=true");
        } finally {
            client.stop();
        }
    }
}
