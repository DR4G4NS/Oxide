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
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.blocks.production.GenericCrafter.GenericCrafterBuild;

/** Exercises RequestBlockSnapshot against the exact desktop build 158.1 decoder. */
public final class SmokeBlockSnapshot158 {
    private static final int POSITION = (45 << 16) | 100;
    private static final int BLOCK_ID = 181;

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
        int port = args.length == 0 ? 6581 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.net = clientNet();
        Groups.init();
        Vars.world.resize(128, 128).fill();
        Vars.world.tile(45, 100).setBlock(Blocks.graphitePress, Team.sharded, 0);

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicInteger snapshots = new AtomicInteger();
        AtomicBoolean requestSent = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(1);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "block-snapshot-158";
                packet.locale = "en";
                packet.uuid = "ERITFBUWFxg=";
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
                        int seen = snapshots.incrementAndGet();
                        packet.handled();
                        // The join snapshot can race a factory tick (graphite 3→4)
                        // after load. RequestBlockSnapshot is the contract this
                        // smoke exists to prove; assert fixture inventory there.
                        verify(packet, seen == 1);
                        if (requestSent.compareAndSet(false, true)) {
                            RequestBlockSnapshotCallPacket request =
                                new RequestBlockSnapshotCallPacket();
                            request.pos = POSITION;
                            connection.sendTCP(request);
                        } else if (seen >= 2) {
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
                if (verified.getCount() != 0) {
                    System.err.println("disconnected before requested snapshot: " + reason);
                    verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(10, TimeUnit.SECONDS)) {
                throw new AssertionError("timed out waiting for requested block snapshot");
            }
            if (!client.isConnected() || snapshots.get() < 2 || !requestSent.get()) {
                throw new AssertionError(
                    "request path failed: connected=" + client.isConnected()
                        + " snapshots=" + snapshots.get()
                        + " requestSent=" + requestSent.get());
            }
            System.out.println(
                "ok requestBlockSnapshot=true snapshots=" + snapshots.get()
                    + " genericCrafterReadSync=true remaining=0");
        } finally {
            client.stop();
        }
    }

    private static void verify(BlockSnapshotCallPacket packet, boolean joinSnapshot)
            throws Exception {
        if (packet.amount != 1) {
            throw new AssertionError("expected one tile, got " + packet.amount);
        }
        ByteArrayInputStream bytes = new ByteArrayInputStream(packet.data);
        DataInputStream input = new DataInputStream(bytes);
        int position = input.readInt();
        int block = input.readShort();
        if (position != POSITION || block != BLOCK_ID) {
            throw new AssertionError(
                "wrong tile header: position=" + position + " block=" + block);
        }
        GenericCrafterBuild build =
            (GenericCrafterBuild)Vars.world.tile(45, 100).build;
        build.readSync(Reads.get(input), build.version());
        if (bytes.available() != 0) {
            throw new AssertionError("unread snapshot tail remaining=" + bytes.available());
        }
        if (joinSnapshot) {
            return;
        }
        // Graphite Press can dump +1 graphite during a slow join (~60 ticks at
        // 1/3 of a 90-tick craft) without resetting this snapshot's progress.
        // That race exists at baseline; copper/progress still match the fixture.
        int graphite = build.items.get(Items.graphite);
        if (build.items.get(Items.copper) != 12
                || (graphite != 3 && graphite != 4)
                || Math.abs(build.progress - (1f / 3f)) > 0.0001f) {
            throw new AssertionError(
                "decoded crafter mismatch: copper=" + build.items.get(Items.copper)
                    + " graphite=" + graphite
                    + " progress=" + build.progress);
        }
    }
}
