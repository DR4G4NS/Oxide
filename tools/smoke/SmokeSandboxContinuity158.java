import java.io.ByteArrayOutputStream;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

import arc.Core;
import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Queue;
import arc.struct.Seq;
import mindustry.Vars;
import mindustry.content.Items;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.Groups;
import mindustry.entities.units.BuildPlan;
import mindustry.gen.BlockSnapshotCallPacket;
import mindustry.gen.ClientSnapshotCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.EntitySnapshotCallPacket;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.blocks.production.Drill;

/**
 * Sandbox continuity smoke against desktop 158.1 (SOL-AUDIT round 72r).
 *
 * Joins a --mode sandbox server, builds an ItemSource -> conveyor chain ->
 * container plus a mechanical drill, then observes BlockSnapshots for more
 * than one official blockSyncTime (6 s). The authoritative snapshots must
 * keep the container inventory growing and the drill warmup positive —
 * a rollback (the historical 6 s reset) makes the snapshot inventory drop,
 * which fails the smoke.
 */
public class SmokeSandboxContinuity158 {
    private static final int CONTAINER_X = 50;
    private static final int SOURCE_X = 46;
    private static final int DRILL_X = 35;
    private static final int Y = 100;

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6595 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.headless = true;
        Core.settings = new arc.Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.net = clientNet();
        Groups.init();
        Vars.world.resize(128, 128).fill();

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        AtomicInteger snapshots = new AtomicInteger();
        AtomicLong lastItems = new AtomicLong(-1);
        AtomicLong maxItems = new AtomicLong(0);
        AtomicBoolean rollback = new AtomicBoolean();
        AtomicBoolean drillWarmup = new AtomicBoolean();
        AtomicInteger snapshotItems = new AtomicInteger(-1);
        AtomicInteger snapshotWarmup = new AtomicInteger(-1);
        CountDownLatch placed = new CountDownLatch(1);
        CountDownLatch done = new CountDownLatch(1);

        // The client only applies readSync to builds that already exist in
        // its local world; seed the chain so the authoritative snapshots
        // land on real buildings (the world stream predates the build).
        Vars.world.tile(SOURCE_X, Y).setBlock(Vars.content.block(412), Team.sharded, 0);
        for (int x = SOURCE_X + 1; x < CONTAINER_X; x++) {
            Vars.world.tile(x, Y).setBlock(Vars.content.block(257), Team.sharded, 0);
        }
        Vars.world.tile(CONTAINER_X, Y).setBlock(Vars.content.block(345), Team.sharded, 0);
        Vars.world.tile(DRILL_X, Y).setBlock(Vars.content.block(325), Team.sharded, 0);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "sandbox-continuity-158";
                packet.locale = "en";
                packet.uuid = "ISIjJCUmJyg="; // same 8-byte id the passing smokes use
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
                            || object instanceof StateSnapshotCallPacket) {
                        if (placed.getCount() != 0) {
                            // Build the chain: ItemSource -> conveyors ->
                            // container, plus a drill on the copper patch.
                            ClientSnapshotCallPacket build = snapshot(connection, 1);
                            connection.sendTCP(build);
                            placed.countDown();
                        }
                    } else if (object instanceof BlockSnapshotCallPacket packet) {
                        // handled() fills amount/data and applies to the
                        // local world; then decode the authoritative payload
                        // directly for the container/drill checks.
                        packet.handled();
                        int seen = snapshots.incrementAndGet();
                        long items = 0;
                        float warmup = 0f;
                        try {
                            // desktop 158.1 NetClient.blockSnapshot reads the
                            // payload with DataInputStream (big-endian); the
                            // local post-158.1 tree switched to a little-endian
                            // ByteBuffer, so the JAR is the authority here.
                            java.io.DataInputStream input = new java.io.DataInputStream(
                                new java.io.ByteArrayInputStream(packet.data));
                            // Sequential payload: pos + block + writeSync per
                            // building; the build's readSync consumes exactly
                            // its own sync bytes, keeping the stream aligned.
                            for (int i = 0; i < packet.amount; i++) {
                                int pos = input.readInt();
                                short block = input.readShort();
                                var content = Vars.content.block(block);
                                if (content == null) {
                                    throw new RuntimeException("unknown block " + block);
                                }
                                var build = content.newBuilding().create(content, Team.sharded);
                                build.readSync(arc.util.io.Reads.get(input), build.version());
                                if (pos == ((CONTAINER_X << 16) | Y)) {
                                    items = build.items != null ? build.items.total() : 0;
                                } else if (pos == ((DRILL_X << 16) | Y)
                                        && build instanceof Drill.DrillBuild d) {
                                    warmup = d.warmup;
                                }
                            }
                        } catch (Throwable error) {
                            error.printStackTrace();
                        }
                        if (seen == 1) {
                            // First snapshot may arrive before constructs finish.
                            snapshotItems.set((int) items);
                        } else {
                            long last = lastItems.get();
                            if (last >= 0 && items + 2 < last) {
                                rollback.set(true); // authoritative reset
                            }
                            if (items > maxItems.get()) maxItems.set(items);
                            snapshotItems.set((int) items);
                        }
                        lastItems.set(items);
                        if (warmup > 0.01f) drillWarmup.set(true);
                        snapshotWarmup.set((int) (warmup * 1000));
                        // Two snapshots (the periodic one + the request reply
                        // or a second periodic batch) prove >6 s continuity.
                        if (seen >= 3) {
                            done.countDown();
                        }
                    }
                } catch (Throwable error) {
                    error.printStackTrace();
                    done.countDown();
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                done.countDown();
            }
        });

        client.start();
        client.connect(5000, "127.0.0.1", port, port);
        if (!done.await(30, TimeUnit.SECONDS)) {
            System.err.println("Sandbox continuity timed out; snapshots=" + snapshots.get());
            client.stop();
            System.exit(1);
        }
        client.stop();

        if (rollback.get() || snapshots.get() < 2) {
            System.err.println("SANDBOX ROLLBACK DETECTED: snapshots=" + snapshots.get()
                + " items=" + snapshotItems.get() + " warmup=" + snapshotWarmup.get());
            System.exit(1);
        }
        if (snapshotItems.get() <= 0) {
            System.err.println("Container never received items: " + snapshotItems.get());
            System.exit(1);
        }
        if (!drillWarmup.get()) {
            System.err.println("Drill warmup never became positive: " + snapshotWarmup.get());
            System.exit(1);
        }
        System.out.println("ok sandboxContinuity=true snapshots=" + snapshots.get()
            + " items=" + snapshotItems.get() + " maxItems=" + maxItems.get()
            + " warmup=" + snapshotWarmup.get());
    }

    private static ClientSnapshotCallPacket snapshot(Connection connection, int snapshotId) {
        ClientSnapshotCallPacket snapshot = new ClientSnapshotCallPacket();
        snapshot.snapshotID = snapshotId;
        snapshot.unitID = 2_000_000 + connection.getID();
        snapshot.dead = false;
        snapshot.x = 420f; // near the build chain at x=50
        snapshot.y = 800f;
        snapshot.pointerX = 420f;
        snapshot.pointerY = 800f;
        snapshot.rotation = 0f;
        snapshot.baseRotation = 0f;
        snapshot.xVelocity = 0f;
        snapshot.yVelocity = 0f;
        snapshot.mining = null;
        snapshot.boosting = false;
        snapshot.shooting = false;
        snapshot.chatting = false;
        snapshot.building = true;
        snapshot.selectedBlock = Vars.content.block(412);
        snapshot.selectedRotation = 0;
        snapshot.plans = new Queue<>();
        // ItemSource -> conveyors -> container.
        snapshot.plans.add(new BuildPlan(SOURCE_X, Y, 0, Vars.content.block(412), Items.copper));
        for (int x = SOURCE_X + 1; x < CONTAINER_X; x++) {
            snapshot.plans.add(new BuildPlan(x, Y, 0, Vars.content.block(257), null));
        }
        snapshot.plans.add(new BuildPlan(CONTAINER_X, Y, 0, Vars.content.block(345), null));
        snapshot.plans.add(new BuildPlan(DRILL_X, Y, 0, Vars.content.block(325), null));
        snapshot.viewX = 420f;
        snapshot.viewY = 800f;
        snapshot.viewWidth = 640f;
        snapshot.viewHeight = 480f;
        return snapshot;
    }

    private static Net clientNet() {
        return new Net(new Net.NetProvider() {
            public void connectClient(String ip, int port, Runnable success) {}
            public void sendClient(Object object, boolean reliable) {}
            public void disconnectClient() {}
            public void discoverServers(arc.func.Cons<Host> found, Runnable done) { done.run(); }
            public void pingHost(String address, int port, arc.func.Cons<Host> valid, arc.func.Cons<Exception> failed) {}
            public void hostServer(int port) {}
            public Iterable<? extends NetConnection> getConnections() { return java.util.List.of(); }
            public void closeServer() {}
        });
    }

    // Shared world-stream state used by the listener above.
    static final AtomicInteger streamId = new AtomicInteger(-1);
    static final AtomicInteger streamTotal = new AtomicInteger(-1);
    static final ByteArrayOutputStream world = new ByteArrayOutputStream();
}
