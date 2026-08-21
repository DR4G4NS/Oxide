import java.io.ByteArrayOutputStream;
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
import arc.struct.Queue;
import arc.struct.Seq;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.core.Version;
import mindustry.entities.units.BuildPlan;
import mindustry.gen.BeginBreakCallPacket;
import mindustry.gen.BeginPlaceCallPacket;
import mindustry.gen.ClientSnapshotCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.ConstructFinishCallPacket;
import mindustry.gen.CreateBulletCallPacket;
import mindustry.gen.DeconstructFinishCallPacket;
import mindustry.gen.EntitySnapshotCallPacket;
import mindustry.gen.GameOverCallPacket;
import mindustry.gen.KickCallPacket;
import mindustry.gen.PingCallPacket;
import mindustry.gen.PingResponseCallPacket;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.SendChatMessageCallPacket;
import mindustry.gen.SendMessageCallPacket;
import mindustry.gen.SendMessageCallPacket2;
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.gen.UnitDeathCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.Tile;

/**
 * Real-socket smoke client using Mindustry desktop 158.1's exact serializer.
 */
public final class SmokeJoin158 {
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

    private static ClientSnapshotCallPacket snapshot(
            Connection connection, int snapshotId, int planMode) {
        ClientSnapshotCallPacket snapshot = new ClientSnapshotCallPacket();
        snapshot.snapshotID = snapshotId;
        snapshot.unitID = 2_000_000 + connection.getID();
        snapshot.dead = false;
        snapshot.x = 324f;
        snapshot.y = 800f;
        snapshot.pointerX = 400f;
        snapshot.pointerY = 800f;
        snapshot.rotation = 0f;
        snapshot.baseRotation = 0f;
        snapshot.xVelocity = 0f;
        snapshot.yVelocity = 0f;
        snapshot.mining = null;
        snapshot.boosting = false;
        snapshot.shooting = planMode == 4;
        snapshot.chatting = false;
        snapshot.building = planMode == 1 || planMode == 2 || planMode == 5;
        snapshot.selectedBlock =
            planMode == 1 || planMode == 5 ? Vars.content.block(216) : null;
        snapshot.selectedRotation = 0;
        snapshot.plans = new Queue<>();
        if (planMode == 1 || planMode == 5) {
            int buildX = planMode == 1 ? 45 : 46;
            snapshot.plans.add(
                new BuildPlan(buildX, 100, 0, Vars.content.block(216), null));
        } else if (planMode == 2) {
            BuildPlan breaking = new BuildPlan();
            breaking.x = 45;
            breaking.y = 100;
            breaking.breaking = true;
            snapshot.plans.add(breaking);
        } else if (planMode == 3) {
            snapshot.mining = new Tile(35, 100);
        }
        snapshot.viewX = 324f;
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
        int port = args.length == 0 ? 6567 : Integer.parseInt(args[0]);
        boolean joinOnly = args.length > 1 && args[1].equals("join-only");
        boolean consoleMode = args.length > 1 && args[1].equals("console");
        Version.build = 158;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.net = clientNet();

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        CountDownLatch joinedTraffic = new CountDownLatch(3);
        CountDownLatch interactionResponses = new CountDownLatch(2);
        CountDownLatch constructionResponses = new CountDownLatch(2);
        CountDownLatch deconstructionResponses = new CountDownLatch(2);
        CountDownLatch combatResponses = new CountDownLatch(2);
        CountDownLatch playerLifecycle = new CountDownLatch(2);
        CountDownLatch consolePackets = new CountDownLatch(4);
        AtomicBoolean confirmed = new AtomicBoolean();
        AtomicBoolean breakingStarted = new AtomicBoolean();
        AtomicBoolean miningStarted = new AtomicBoolean();
        AtomicBoolean deadBuildSent = new AtomicBoolean();
        AtomicBoolean postRespawnMovementSent = new AtomicBoolean();
        AtomicInteger playerSpawns = new AtomicInteger();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        ByteArrayOutputStream world = new ByteArrayOutputStream();

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.version = 158;
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "smoke-158";
                packet.locale = "en";
                packet.uuid = "AQIDBAUGBwg=";
                packet.usid = "";
                packet.mobile = false;
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
                        if (joinOnly || consoleMode) {
                            return;
                        }
                        connection.sendUDP(snapshot(connection, 1, 1));
                        Thread buildUpdates = new Thread(() -> {
                            try {
                                for (int id = 2; id <= 12; id++) {
                                    Thread.sleep(100);
                                    connection.sendUDP(snapshot(connection, id, 1));
                                }
                            } catch (InterruptedException interrupted) {
                                Thread.currentThread().interrupt();
                            }
                        }, "smoke-158-build-updates");
                        buildUpdates.setDaemon(true);
                        buildUpdates.start();

                        PingCallPacket ping = new PingCallPacket();
                        ping.time = 123_456_789L;
                        connection.sendTCP(ping);

                        SendChatMessageCallPacket chat = new SendChatMessageCallPacket();
                        chat.message = "smoke-chat-158";
                        connection.sendTCP(chat);
                    }
                } else if (object instanceof PlayerSpawnCallPacket) {
                    joinedTraffic.countDown();
                    if (playerSpawns.incrementAndGet() >= 2) {
                        playerLifecycle.countDown();
                    }
                } else if (object instanceof EntitySnapshotCallPacket entities) {
                    joinedTraffic.countDown();
                    Integer id = firstUnitId(entities);
                    if (id != null && id >= 2_500_000 && id < 3_000_000
                            && postRespawnMovementSent.compareAndSet(false, true)) {
                        ClientSnapshotCallPacket moved = snapshot(connection, 500, 0);
                        moved.unitID = id;
                        moved.x = 328f;
                        moved.viewX = 328f;
                        connection.sendUDP(moved);
                    }
                } else if (object instanceof StateSnapshotCallPacket) {
                    joinedTraffic.countDown();
                } else if (object instanceof PingResponseCallPacket) {
                    interactionResponses.countDown();
                } else if (object instanceof SendMessageCallPacket
                        || object instanceof SendMessageCallPacket2) {
                    if (consoleMode) {
                        consolePackets.countDown();
                    } else {
                        interactionResponses.countDown();
                    }
                } else if (object instanceof GameOverCallPacket) {
                    consolePackets.countDown();
                } else if (object instanceof KickCallPacket) {
                    consolePackets.countDown();
                } else if (object instanceof BeginPlaceCallPacket) {
                    constructionResponses.countDown();
                } else if (object instanceof ConstructFinishCallPacket) {
                    constructionResponses.countDown();
                    if (breakingStarted.compareAndSet(false, true)) {
                        Thread breakUpdates = new Thread(() -> {
                            try {
                                for (int id = 100; id <= 112; id++) {
                                    connection.sendUDP(snapshot(connection, id, 2));
                                    Thread.sleep(100);
                                }
                            } catch (InterruptedException interrupted) {
                                Thread.currentThread().interrupt();
                            }
                        }, "smoke-158-break-updates");
                        breakUpdates.setDaemon(true);
                        breakUpdates.start();
                    }
                } else if (object instanceof BeginBreakCallPacket) {
                    deconstructionResponses.countDown();
                } else if (object instanceof DeconstructFinishCallPacket) {
                    deconstructionResponses.countDown();
                    if (miningStarted.compareAndSet(false, true)) {
                        Thread miningUpdates = new Thread(() -> {
                            try {
                                for (int id = 200; id <= 202; id++) {
                                    connection.sendUDP(snapshot(connection, id, 3));
                                    Thread.sleep(100);
                                }
                                for (int id = 300; id <= 308; id++) {
                                    connection.sendUDP(snapshot(connection, id, 4));
                                    Thread.sleep(100);
                                }
                            } catch (InterruptedException interrupted) {
                                Thread.currentThread().interrupt();
                            }
                        }, "smoke-158-mining-updates");
                        miningUpdates.setDaemon(true);
                        miningUpdates.start();
                    }
                } else if (object instanceof CreateBulletCallPacket bullet) {
                    bullet.handled();
                    if (bullet.type != null && bullet.type.id == 65) {
                        combatResponses.countDown();
                    }
                } else if (object instanceof UnitDeathCallPacket death) {
                    death.handled();
                    if (death.uid == 3_000_000) {
                        combatResponses.countDown();
                    } else if (death.uid == 2_000_000 + connection.getID()) {
                        playerLifecycle.countDown();
                        if (deadBuildSent.compareAndSet(false, true)) {
                            Thread deadBuild = new Thread(() -> {
                                try {
                                    for (int id = 400; id <= 402; id++) {
                                        connection.sendUDP(snapshot(connection, id, 5));
                                        Thread.sleep(100);
                                    }
                                } catch (InterruptedException interrupted) {
                                    Thread.currentThread().interrupt();
                                }
                            }, "smoke-158-dead-build");
                            deadBuild.setDaemon(true);
                            deadBuild.start();
                        }
                    }
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (joinedTraffic.getCount() != 0) {
                    System.err.println(
                        "disconnected before post-join traffic: " + reason
                            + " protocolError=" + connection.getLastProtocolError());
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!joinedTraffic.await(10, TimeUnit.SECONDS)) {
                throw new AssertionError(
                    "timed out: world=" + world.size() + "/" + streamTotal.get()
                        + " confirmed=" + confirmed.get()
                        + " missingPostJoinPackets=" + joinedTraffic.getCount()
                        + " protocolError=" + client.getLastProtocolError());
            }
            if (joinOnly) {
                Thread.sleep(1200);
                if (!client.isConnected()) {
                    throw new AssertionError(
                        "reconnection closed after post-join packets: "
                            + client.getLastProtocolError());
                }
                System.out.println(
                    "ok reconnect=true worldBytes=" + world.size()
                        + " connectConfirm=true postJoinPackets=3");
                return;
            }
            if (consoleMode) {
                if (!consolePackets.await(10, TimeUnit.SECONDS)) {
                    throw new AssertionError(
                        "missing console RPCs=" + consolePackets.getCount()
                            + " protocolError=" + client.getLastProtocolError());
                }
                System.out.println(
                    "ok consoleSay=true consoleGameOver=true consoleKick=true");
                return;
            }
            if (!interactionResponses.await(5, TimeUnit.SECONDS)) {
                throw new AssertionError(
                    "missing ping/chat responses=" + interactionResponses.getCount()
                        + " protocolError=" + client.getLastProtocolError());
            }
            if (!constructionResponses.await(5, TimeUnit.SECONDS)) {
                throw new AssertionError(
                    "missing construction responses=" + constructionResponses.getCount()
                        + " protocolError=" + client.getLastProtocolError());
            }
            if (!deconstructionResponses.await(5, TimeUnit.SECONDS)) {
                throw new AssertionError(
                    "missing deconstruction responses=" + deconstructionResponses.getCount()
                        + " protocolError=" + client.getLastProtocolError());
            }
            if (!combatResponses.await(5, TimeUnit.SECONDS)) {
                throw new AssertionError(
                    "missing combat responses=" + combatResponses.getCount()
                        + " protocolError=" + client.getLastProtocolError());
            }
            if (!playerLifecycle.await(10, TimeUnit.SECONDS)) {
                throw new AssertionError(
                    "missing player death/respawn=" + playerLifecycle.getCount()
                        + " protocolError=" + client.getLastProtocolError());
            }
            Thread.sleep(1200);
            if (!client.isConnected()) {
                throw new AssertionError(
                    "connection closed after post-join packets: "
                        + client.getLastProtocolError());
            }
            System.out.println(
                "ok connected=true worldBytes=" + world.size()
                    + " connectConfirm=true postJoinPackets=3"
                    + " movement=true pingResponse=true chatEcho=true"
                    + " beginPlace=true constructFinish=true"
                    + " beginBreak=true deconstructFinish=true"
                    + " mining=true createBullet=true unitDeath=true"
                    + " playerDeath=true respawn=true deadBuildBlocked=true");
        } finally {
            client.stop();
        }
    }
}
