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
import mindustry.content.Items;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.BlockSnapshotCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.RequestBlockSnapshotCallPacket;
import mindustry.gen.TileConfigCallPacket;
import mindustry.io.TypeIO;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.type.Item;
import mindustry.world.blocks.distribution.Sorter.SorterBuild;

/** Configures a live Sorter through exact desktop 158.1 TileConfig packets. */
public final class SmokeTileConfig158 {
    private static final int POSITION = (45 << 16) | 100;
    private static final Field TILE_CONFIG_DATA;

    static {
        try {
            TILE_CONFIG_DATA = TileConfigCallPacket.class.getDeclaredField("DATA");
            TILE_CONFIG_DATA.setAccessible(true);
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
        int port = args.length == 0 ? 6582 : Integer.parseInt(args[0]);
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
        AtomicBoolean configEcho = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(1);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "tile-config-158";
                packet.locale = "en";
                packet.uuid = "ISIjJCUmJyg=";
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
                        verifySorterSnapshot(packet, seen == 1 ? null : Items.graphite);
                        if (seen == 1) {
                            TileConfigCallPacket config = new TileConfigCallPacket();
                            config.build = Vars.world.tile(45, 100).build;
                            config.value = Items.graphite;
                            connection.sendTCP(config);
                        } else if (configEcho.get()) {
                            verified.countDown();
                        }
                    } else if (object instanceof TileConfigCallPacket packet) {
                        verifyConfigEcho(packet);
                        configEcho.set(true);
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
                    System.err.println("disconnected before config verification: " + reason);
                    verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(10, TimeUnit.SECONDS)) {
                throw new AssertionError("timed out waiting for configured Sorter snapshot");
            }
            if (!client.isConnected() || snapshots.get() < 2 || !configEcho.get()) {
                throw new AssertionError(
                    "TileConfig path failed: connected=" + client.isConnected()
                        + " snapshots=" + snapshots.get() + " echo=" + configEcho.get());
            }
            Thread.sleep(1200);
            System.out.println(
                "ok tileConfig=true officialObjectDecode=true configuredSnapshot=true"
                    + " persisted=true");
        } finally {
            client.stop();
        }
    }

    private static void verifyConfigEcho(TileConfigCallPacket packet) throws Exception {
        byte[] raw = (byte[])TILE_CONFIG_DATA.get(packet);
        ByteArrayInputStream bytes = new ByteArrayInputStream(raw);
        DataInputStream input = new DataInputStream(bytes);
        int playerId = input.readInt();
        int position = input.readInt();
        Object value = TypeIO.readObjectSafe(Reads.get(input));
        // Arc generateId is Rand.nextInt(); player ids are often negative.
        // See SmokeRotateBlock158.verifyEcho.
        if (position != POSITION || value != Items.graphite
                || bytes.available() != 0) {
            throw new AssertionError(
                "invalid TileConfig echo: player=" + playerId + " position=" + position
                    + " value=" + value + " remaining=" + bytes.available());
        }
    }

    private static void verifySorterSnapshot(BlockSnapshotCallPacket packet, Item expected)
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
        if (position != POSITION || block != Blocks.sorter.id || build.sortItem != expected
                || bytes.available() != 0) {
            throw new AssertionError(
                "sorter snapshot mismatch: position=" + position + " block=" + block
                    + " item=" + build.sortItem + " expected=" + expected
                    + " remaining=" + bytes.available());
        }
    }
}
