import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;
import java.util.zip.Inflater;

import arc.Core;
import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Queue;
import arc.struct.Seq;
import mindustry.Vars;
import mindustry.ai.BlockIndexer;
import mindustry.ai.Pathfinder;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.entities.units.BuildPlan;
import mindustry.game.Teams;
import mindustry.gen.BeginBreakCallPacket;
import mindustry.gen.BeginPlaceCallPacket;
import mindustry.gen.ClientSnapshotCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.ConstructFinishCallPacket;
import mindustry.gen.DeconstructFinishCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;

/**
 * Current-target sandbox build/break smoke. Inflates the world stream and reads the
 * Rules UTF-8 from the data-patch prefix (no NetworkIO.readWorld). Asserts
 * vanilla sandbox flags and that place/finish plus begin-break/finish arrive.
 */
public final class SmokeSandboxBuild1597 {
    private static final int TILE_X = 45;
    private static final int TILE_Y = 100;
    private static final int BLOCK_ID = 216;

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6598 : Integer.parseInt(args[0]);
        Version.build = Integer.getInteger("oxide.smoke.build", 160);
        Vars.headless = true;
        Core.settings = new arc.Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Vars.state = new GameState();
        Vars.state.teams = new Teams();
        Vars.world = new World();
        Vars.net = clientNet();
        Groups.init();
        Vars.world.resize(400, 400).fill();
        Vars.indexer = new BlockIndexer();
        Vars.pathfinder = new Pathfinder();

        Client client = new Client(32768, 32768, new ArcNetProvider.PacketSerializer());
        CountDownLatch spawned = new CountDownLatch(1);
        CountDownLatch place = new CountDownLatch(2);
        CountDownLatch breaking = new CountDownLatch(2);
        AtomicBoolean confirmed = new AtomicBoolean();
        AtomicBoolean rulesOk = new AtomicBoolean();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicInteger snapshotId = new AtomicInteger();
        AtomicLong breakSentAt = new AtomicLong();
        AtomicLong beginBreakAt = new AtomicLong();
        AtomicLong finishBreakAt = new AtomicLong();
        AtomicReference<Connection> live = new AtomicReference<>();
        ByteArrayOutputStream world = new ByteArrayOutputStream();

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "smoke-sandbox-build-1597";
                packet.locale = "en";
                packet.uuid = "c21va2Utc2I=";
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
                        if (world.size() == streamTotal.get()
                                && confirmed.compareAndSet(false, true)) {
                            assertSandboxRules(readStreamedRules(world.toByteArray()));
                            rulesOk.set(true);
                            connection.sendTCP(new ConnectConfirmCallPacket());
                        }
                    } else if (object instanceof PlayerSpawnCallPacket
                            || object instanceof StateSnapshotCallPacket) {
                        live.set(connection);
                        spawned.countDown();
                    } else if (object instanceof BeginPlaceCallPacket packet) {
                        applyQuiet(packet::handled, packet::handleClient);
                        place.countDown();
                    } else if (object instanceof ConstructFinishCallPacket packet) {
                        applyQuiet(packet::handled, packet::handleClient);
                        place.countDown();
                    } else if (object instanceof BeginBreakCallPacket packet) {
                        applyQuiet(packet::handled, packet::handleClient);
                        beginBreakAt.compareAndSet(0, System.currentTimeMillis());
                        breaking.countDown();
                    } else if (object instanceof DeconstructFinishCallPacket packet) {
                        applyQuiet(packet::handled, packet::handleClient);
                        finishBreakAt.compareAndSet(0, System.currentTimeMillis());
                        breaking.countDown();
                    }
                } catch (Throwable error) {
                    error.printStackTrace();
                    while (place.getCount() > 0) {
                        place.countDown();
                    }
                    while (breaking.getCount() > 0) {
                        breaking.countDown();
                    }
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (spawned.getCount() != 0 || place.getCount() != 0 || breaking.getCount() != 0) {
                    System.err.println("disconnected before sandbox smoke finished: " + reason);
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!spawned.await(15, TimeUnit.SECONDS) || live.get() == null) {
                throw new AssertionError("never spawned; rules=" + rulesOk.get());
            }
            if (!rulesOk.get()) {
                throw new AssertionError("streamed sandbox rules were not read");
            }
            Connection connection = live.get();
            Thread builder = new Thread(() -> {
                try {
                    while (place.getCount() > 0 && client.isConnected()) {
                        connection.sendTCP(planSnapshot(connection, snapshotId.incrementAndGet(), false));
                        Thread.sleep(50);
                    }
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                }
            }, "smoke-sandbox-build");
            builder.setDaemon(true);
            builder.start();
            if (!place.await(8, TimeUnit.SECONDS)) {
                throw new AssertionError("missing place/finish packets=" + place.getCount());
            }

            breakSentAt.set(System.currentTimeMillis());
            Thread breaker = new Thread(() -> {
                try {
                    while (breaking.getCount() > 0 && client.isConnected()) {
                        connection.sendTCP(planSnapshot(connection, snapshotId.incrementAndGet(), true));
                        Thread.sleep(50);
                    }
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                }
            }, "smoke-sandbox-break");
            breaker.setDaemon(true);
            breaker.start();
            if (!breaking.await(8, TimeUnit.SECONDS)) {
                throw new AssertionError("missing begin-break/finish packets=" + breaking.getCount());
            }
            long begin = beginBreakAt.get();
            long finish = finishBreakAt.get();
            long beginToFinish = (begin > 0 && finish >= begin) ? (finish - begin) : -1;
            long sentToFinish = finish > 0 ? (finish - breakSentAt.get()) : -1;
            System.out.println(
                "SMOKE_OK sandbox-build rules=true beginPlace=true constructFinish=true"
                    + " beginBreak=true deconstructFinish=true"
                    + " begin_to_finish_ms=" + beginToFinish
                    + " plan_sent_to_finish_ms=" + sentToFinish);
        } finally {
            client.stop();
        }
    }

    private static ClientSnapshotCallPacket planSnapshot(
            Connection connection, int id, boolean breaking) {
        ClientSnapshotCallPacket snapshot = new ClientSnapshotCallPacket();
        snapshot.snapshotID = id;
        snapshot.unitID = 2_000_000 + connection.getID();
        snapshot.dead = false;
        snapshot.x = TILE_X * 8f;
        snapshot.y = TILE_Y * 8f;
        snapshot.pointerX = TILE_X * 8f;
        snapshot.pointerY = TILE_Y * 8f;
        snapshot.rotation = 0f;
        snapshot.baseRotation = 0f;
        snapshot.xVelocity = 0f;
        snapshot.yVelocity = 0f;
        snapshot.mining = null;
        snapshot.boosting = false;
        snapshot.shooting = false;
        snapshot.chatting = false;
        snapshot.building = true;
        snapshot.selectedBlock = Vars.content.block(BLOCK_ID);
        snapshot.selectedRotation = 0;
        snapshot.viewX = TILE_X * 8f;
        snapshot.viewY = TILE_Y * 8f;
        snapshot.viewWidth = 640f;
        snapshot.viewHeight = 480f;
        snapshot.plans = new Queue<>();
        if (breaking) {
            snapshot.plans.add(new BuildPlan(TILE_X, TILE_Y));
        } else {
            snapshot.plans.add(new BuildPlan(TILE_X, TILE_Y, 0, Vars.content.block(BLOCK_ID), null));
        }
        return snapshot;
    }

    private static void assertSandboxRules(String rules) {
        mindustry.io.JsonIO.read(mindustry.game.Rules.class, rules);
        if (!rulesFlag(rules, "infiniteResources", true)
                || !rulesFlag(rules, "instantBuild", false)
                || !rulesFlag(rules, "allowEditRules", true)
                || !rulesFlag(rules, "waves", true)
                || !rulesFlag(rules, "waveTimer", false)) {
            throw new AssertionError("sandbox Rules mismatch: " + rules);
        }
    }

    static String readStreamedRules(byte[] compressed) throws Exception {
        byte[] plain = inflate(compressed);
        if (plain.length < 10
                || plain[0] != 0
                || plain[1] != 0
                || plain[2] != 0
                || plain[3] != 2) {
            throw new AssertionError("current-target data-patch header missing after inflate");
        }
        DataInputStream input = new DataInputStream(
            new ByteArrayInputStream(plain, 8, plain.length - 8));
        return input.readUTF();
    }

    static byte[] inflate(byte[] compressed) throws Exception {
        Inflater inflater = new Inflater();
        inflater.setInput(compressed);
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        byte[] buf = new byte[4096];
        while (!inflater.finished()) {
            int n = inflater.inflate(buf);
            if (n == 0) {
                break;
            }
            out.write(buf, 0, n);
        }
        inflater.end();
        if (out.size() == 0) {
            throw new AssertionError("world stream did not inflate");
        }
        return out.toByteArray();
    }

    static boolean rulesFlag(String rules, String key, boolean expected) {
        return rules.contains(key + ":" + expected)
            || rules.contains("\"" + key + "\":" + expected);
    }

    interface Step {
        void run() throws Exception;
    }

    static void applyQuiet(Step decode, Step apply) {
        try {
            decode.run();
        } catch (Throwable ignored) {
            return;
        }
        try {
            apply.run();
        } catch (Throwable ignored) {
            // Headless harness lacks renderer/sound; the packet still arrived.
        }
    }

    static Net clientNet() {
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
}
