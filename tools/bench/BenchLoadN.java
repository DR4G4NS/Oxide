import java.io.ByteArrayOutputStream;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Locale;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;

import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Queue;
import arc.struct.Seq;
import arc.util.serialization.Base64Coder;
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
import mindustry.gen.KickCallPacket;
import mindustry.gen.PingCallPacket;
import mindustry.gen.PingResponseCallPacket;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.gen.UnitDeathCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.Block;

/**
 * Concurrent ArcNet load generator with optional build/break/combat workload.
 *
 * Args:
 *   --port N --players N --duration-sec N --build N --ramp-ms N
 *   --snapshot-hz N --host IP --warmup-sec N --uuid-bytes 8|16
 *   --workload idle|build  --block-id N
 */
public final class BenchLoadN {
    private static final int WRITE_BUFFER = 262144;
    private static final int OBJECT_BUFFER = 262144;
    private static final int COPPER_WALL_ID = 216;

    private static Net clientNet() {
        return new Net(new Net.NetProvider() {
            public void connectClient(String ip, int port, Runnable success) {}
            public void sendClient(Object object, boolean reliable) {}
            public void disconnectClient() {}
            public void discoverServers(arc.func.Cons<Host> found, Runnable done) {
                done.run();
            }
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

    private static String uuidFor(int index, int uuidBytes) {
        if (uuidBytes != 8 && uuidBytes != 16) {
            throw new IllegalArgumentException("--uuid-bytes must be 8 or 16");
        }
        byte[] raw = new byte[uuidBytes];
        long high = 0xBEEFCAFE_00000000L ^ (index + 1L);
        long low = 0xA11CE_00000000L ^ ((long)index * 0x9E3779B97F4A7C15L);
        for (int i = 0; i < uuidBytes; i++) {
            long src = i < 8 ? high : low;
            int shift = (7 - (i % 8)) * 8;
            raw[i] = (byte)((src >>> shift) & 0xff);
        }
        return new String(Base64Coder.encode(raw));
    }

    private static int argInt(String[] args, String key, int fallback) {
        for (int i = 0; i < args.length - 1; i++) {
            if (args[i].equals(key)) {
                return Integer.parseInt(args[i + 1]);
            }
        }
        return fallback;
    }

    private static String argStr(String[] args, String key, String fallback) {
        for (int i = 0; i < args.length - 1; i++) {
            if (args[i].equals(key)) {
                return args[i + 1];
            }
        }
        return fallback;
    }

    private static long percentile(long[] sorted, double p) {
        if (sorted.length == 0) {
            return -1L;
        }
        int index = (int)Math.ceil(p * sorted.length) - 1;
        index = Math.max(0, Math.min(sorted.length - 1, index));
        return sorted[index];
    }

    private static String jsonEscape(String value) {
        return value
            .replace("\\", "\\\\")
            .replace("\"", "\\\"")
            .replace("\n", "\\n")
            .replace("\r", "\\r");
    }

    private static final class Bot {
        final int index;
        final String name;
        final String uuid;
        final float x;
        final float y;
        final int tileX;
        final int tileY;
        final boolean workloadBuild;
        final Block buildBlock;
        final Client client =
            new Client(WRITE_BUFFER, OBJECT_BUFFER, new ArcNetProvider.PacketSerializer());
        final CountDownLatch joined = new CountDownLatch(1);
        final AtomicBoolean confirmed = new AtomicBoolean();
        final AtomicBoolean kicked = new AtomicBoolean();
        final AtomicInteger streamId = new AtomicInteger(-1);
        final AtomicInteger streamTotal = new AtomicInteger(-1);
        final ByteArrayOutputStream world = new ByteArrayOutputStream();
        final AtomicLong connectNanos = new AtomicLong();
        final AtomicLong joinNanos = new AtomicLong();
        final AtomicInteger snapshotId = new AtomicInteger();
        final AtomicInteger connectionId = new AtomicInteger(-1);
        final AtomicInteger liveUnitId = new AtomicInteger(-1);
        final LongAdder packetsRx = new LongAdder();
        final LongAdder entitySnapshotsRx = new LongAdder();
        final LongAdder stateSnapshotsRx = new LongAdder();
        final LongAdder snapshotsTx = new LongAdder();
        final LongAdder buildSnapshotsTx = new LongAdder();
        final LongAdder breakSnapshotsTx = new LongAdder();
        final LongAdder shootSnapshotsTx = new LongAdder();
        final LongAdder beginPlaceRx = new LongAdder();
        final LongAdder constructFinishRx = new LongAdder();
        final LongAdder beginBreakRx = new LongAdder();
        final LongAdder deconstructFinishRx = new LongAdder();
        final LongAdder unitDeathRx = new LongAdder();
        final LongAdder createBulletRx = new LongAdder();
        final LongAdder pingsTx = new LongAdder();
        final LongAdder pingSamples = new LongAdder();
        final LongAdder pingTotalMs = new LongAdder();
        final AtomicLong pingMaxMs = new AtomicLong();
        final AtomicLong lastPingSentMs = new AtomicLong();
        volatile String failReason = "";
        volatile boolean connected;

        Bot(int index, int uuidBytes, boolean workloadBuild, Block buildBlock) {
            this.index = index;
            this.name = "bench-" + index;
            this.uuid = uuidFor(index, uuidBytes);
            this.workloadBuild = workloadBuild;
            this.buildBlock = buildBlock;
            // Keep builders near the maze core (tile ~40,100) with unique cells.
            this.tileX = 48 + (index % 28);
            this.tileY = 88 + (index / 28);
            this.x = tileX * 8f + 4f;
            this.y = tileY * 8f + 4f;
            client.addListener(new NetListener() {
                @Override
                public void connected(Connection connection) {
                    connected = true;
                    connectionId.set(connection.getID());
                    liveUnitId.set(2_000_000 + connection.getID());
                    connectNanos.set(System.nanoTime());
                    Packets.ConnectPacket packet = new Packets.ConnectPacket();
                    packet.version = Version.build;
                    packet.versionType = "official";
                    packet.mods = new Seq<>();
                    packet.name = name;
                    packet.locale = "en";
                    packet.uuid = uuid;
                    packet.usid = "bench-usid-" + index;
                    packet.color = 0xffa665ff;
                    connection.sendTCP(packet);
                }

                @Override
                public synchronized void received(Connection connection, Object object) {
                    packetsRx.increment();
                    if (object instanceof Packets.StreamBegin begin) {
                        streamId.set(begin.id);
                        streamTotal.set(begin.total);
                        world.reset();
                    } else if (object instanceof Packets.StreamChunk chunk
                            && chunk.id == streamId.get()) {
                        world.write(chunk.data, 0, chunk.data.length);
                        if (world.size() == streamTotal.get()
                                && confirmed.compareAndSet(false, true)) {
                            connection.sendTCP(new ConnectConfirmCallPacket());
                            connection.sendTCP(makeSnapshot(1, 0));
                            snapshotsTx.increment();
                        }
                    } else if (object instanceof PlayerSpawnCallPacket) {
                        if (joinNanos.get() == 0L) {
                            joinNanos.set(System.nanoTime());
                            joined.countDown();
                        }
                    } else if (object instanceof StateSnapshotCallPacket) {
                        stateSnapshotsRx.increment();
                        if (joinNanos.get() == 0L) {
                            joinNanos.set(System.nanoTime());
                            joined.countDown();
                        }
                    } else if (object instanceof EntitySnapshotCallPacket) {
                        entitySnapshotsRx.increment();
                    } else if (object instanceof BeginPlaceCallPacket) {
                        beginPlaceRx.increment();
                    } else if (object instanceof ConstructFinishCallPacket) {
                        constructFinishRx.increment();
                    } else if (object instanceof BeginBreakCallPacket) {
                        beginBreakRx.increment();
                    } else if (object instanceof DeconstructFinishCallPacket) {
                        deconstructFinishRx.increment();
                    } else if (object instanceof UnitDeathCallPacket) {
                        unitDeathRx.increment();
                    } else if (object instanceof CreateBulletCallPacket) {
                        createBulletRx.increment();
                    } else if (object instanceof KickCallPacket kick) {
                        kicked.set(true);
                        failReason = "kick:" + kick.reason;
                        joined.countDown();
                    } else if (object instanceof PingResponseCallPacket) {
                        long sent = lastPingSentMs.get();
                        if (sent > 0) {
                            long rtt = Math.max(0L, System.currentTimeMillis() - sent);
                            pingSamples.increment();
                            pingTotalMs.add(rtt);
                            pingMaxMs.accumulateAndGet(rtt, Math::max);
                            lastPingSentMs.set(0L);
                        }
                    }
                }

                @Override
                public void disconnected(Connection connection, DcReason reason) {
                    connected = false;
                    if (joinNanos.get() == 0L && failReason.isEmpty()) {
                        failReason = "disconnect:" + reason;
                        joined.countDown();
                    }
                }
            });
        }

        private ClientSnapshotCallPacket makeSnapshot(int id, int phase) {
            ClientSnapshotCallPacket packet = new ClientSnapshotCallPacket();
            packet.snapshotID = id;
            int unit = liveUnitId.get();
            packet.unitID = unit > 0 ? unit : (2_000_000 + Math.max(0, connectionId.get()));
            packet.dead = false;
            packet.x = x;
            packet.y = y;
            packet.pointerX = x + 48f;
            packet.pointerY = y + 24f;
            packet.rotation = 45f;
            packet.baseRotation = 45f;
            packet.xVelocity = 0f;
            packet.yVelocity = 0f;
            packet.mining = null;
            packet.boosting = false;
            packet.chatting = false;
            packet.plans = new Queue<>();
            packet.viewX = x;
            packet.viewY = y;
            packet.viewWidth = 640f;
            packet.viewHeight = 480f;
            packet.building = false;
            packet.shooting = false;
            packet.selectedBlock = null;
            packet.selectedRotation = 0;

            if (workloadBuild && buildBlock != null) {
                // 40-tick duty cycle: build -> shoot -> break -> idle
                int step = phase % 40;
                if (step < 14) {
                    packet.building = true;
                    packet.selectedBlock = buildBlock;
                    packet.plans.add(new BuildPlan(tileX, tileY, 0, buildBlock, null));
                    buildSnapshotsTx.increment();
                } else if (step < 20) {
                    packet.shooting = true;
                    shootSnapshotsTx.increment();
                } else if (step < 34) {
                    packet.building = true;
                    BuildPlan breaking = new BuildPlan();
                    breaking.x = tileX;
                    breaking.y = tileY;
                    breaking.breaking = true;
                    packet.plans.add(breaking);
                    breakSnapshotsTx.increment();
                } else {
                    packet.shooting = (step % 2) == 0;
                    if (packet.shooting) {
                        shootSnapshotsTx.increment();
                    }
                }
            }
            return packet;
        }

        void tickTraffic() {
            if (!confirmed.get() || !client.isConnected()) {
                return;
            }
            try {
                int id = snapshotId.incrementAndGet();
                client.sendUDP(makeSnapshot(id, id));
                snapshotsTx.increment();
                if (lastPingSentMs.get() == 0L) {
                    long now = System.currentTimeMillis();
                    lastPingSentMs.set(now);
                    PingCallPacket ping = new PingCallPacket();
                    ping.time = now;
                    client.sendTCP(ping);
                    pingsTx.increment();
                }
            } catch (Throwable ignored) {
            }
        }
    }

    public static void main(String[] args) throws Exception {
        int port = argInt(args, "--port", 6590);
        int players = argInt(args, "--players", 500);
        int durationSec = argInt(args, "--duration-sec", 30);
        int build = argInt(args, "--build", 159);
        int rampMs = argInt(args, "--ramp-ms", 40);
        int snapshotHz = Math.max(1, argInt(args, "--snapshot-hz", 10));
        int connectTimeoutMs = argInt(args, "--connect-timeout-ms", 8000);
        int warmupSec = argInt(args, "--warmup-sec", 5);
        int uuidBytes = argInt(args, "--uuid-bytes", 16);
        int blockId = argInt(args, "--block-id", COPPER_WALL_ID);
        String host = argStr(args, "--host", "127.0.0.1");
        String workload = argStr(args, "--workload", "build");
        boolean workloadBuild = !workload.equalsIgnoreCase("idle");

        Version.build = build;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.net = clientNet();
        Block buildBlock = Vars.content.block(blockId);
        if (workloadBuild && buildBlock == null) {
            throw new IllegalStateException("unknown block id " + blockId);
        }

        System.err.printf(
            Locale.US,
            "BenchLoadN host=%s port=%d players=%d duration=%ds build=%d rampMs=%d snapshotHz=%d uuidBytes=%d workload=%s block=%s%n",
            host, port, players, durationSec, build, rampMs, snapshotHz, uuidBytes, workload,
            buildBlock == null ? "none" : buildBlock.name);

        List<Bot> bots = new ArrayList<>(players);
        for (int i = 0; i < players; i++) {
            bots.add(new Bot(i, uuidBytes, workloadBuild, buildBlock));
        }

        long startNs = System.nanoTime();
        int connectFailures = 0;
        for (int i = 0; i < bots.size(); i++) {
            Bot bot = bots.get(i);
            bot.client.start();
            try {
                bot.client.connect(connectTimeoutMs, host, port, port);
            } catch (Throwable error) {
                connectFailures++;
                bot.failReason = "connect:" + error.getClass().getSimpleName();
                bot.joined.countDown();
            }
            if (rampMs > 0) {
                Thread.sleep(rampMs);
            }
            if ((i + 1) % 50 == 0 || i + 1 == bots.size()) {
                System.err.printf(Locale.US, "connected_attempted=%d/%d failures=%d%n",
                    i + 1, bots.size(), connectFailures);
            }
        }

        long joinDeadlineNs = System.nanoTime() + TimeUnit.SECONDS.toNanos(Math.max(45, players / 4));
        int joined = 0;
        while (System.nanoTime() < joinDeadlineNs) {
            joined = 0;
            for (Bot bot : bots) {
                if (bot.joinNanos.get() > 0L) {
                    joined++;
                }
            }
            if (joined == players) {
                break;
            }
            Thread.sleep(100);
        }
        long allJoinedNs = System.nanoTime();
        System.err.printf(Locale.US, "joined=%d/%d after_ms=%.1f%n",
            joined, players, (allJoinedNs - startNs) / 1_000_000.0);

        long warmupEnd = System.nanoTime() + TimeUnit.SECONDS.toNanos(warmupSec);
        long steadyEnd = warmupEnd + TimeUnit.SECONDS.toNanos(durationSec);
        long snapshotIntervalNs = 1_000_000_000L / snapshotHz;
        long nextTick = System.nanoTime();
        while (System.nanoTime() < steadyEnd) {
            long now = System.nanoTime();
            if (now >= nextTick) {
                for (Bot bot : bots) {
                    bot.tickTraffic();
                }
                nextTick = now + snapshotIntervalNs;
            }
            Thread.sleep(1);
        }

        List<Long> joinMs = new ArrayList<>();
        int confirmed = 0;
        int kicked = 0;
        int stillConnected = 0;
        long packetsRx = 0;
        long entityRx = 0;
        long stateRx = 0;
        long snapshotsTx = 0;
        long buildTx = 0;
        long breakTx = 0;
        long shootTx = 0;
        long beginPlace = 0;
        long constructFinish = 0;
        long beginBreak = 0;
        long deconstructFinish = 0;
        long unitDeath = 0;
        long createBullet = 0;
        long pingsTx = 0;
        long pingSamples = 0;
        long pingTotalMs = 0;
        long pingMaxMs = 0;
        List<String> failures = new ArrayList<>();
        for (Bot bot : bots) {
            if (bot.confirmed.get()) {
                confirmed++;
            }
            if (bot.kicked.get()) {
                kicked++;
            }
            if (bot.client.isConnected()) {
                stillConnected++;
            }
            if (bot.joinNanos.get() > 0L && bot.connectNanos.get() > 0L) {
                joinMs.add((bot.joinNanos.get() - bot.connectNanos.get()) / 1_000_000L);
            }
            packetsRx += bot.packetsRx.sum();
            entityRx += bot.entitySnapshotsRx.sum();
            stateRx += bot.stateSnapshotsRx.sum();
            snapshotsTx += bot.snapshotsTx.sum();
            buildTx += bot.buildSnapshotsTx.sum();
            breakTx += bot.breakSnapshotsTx.sum();
            shootTx += bot.shootSnapshotsTx.sum();
            beginPlace += bot.beginPlaceRx.sum();
            constructFinish += bot.constructFinishRx.sum();
            beginBreak += bot.beginBreakRx.sum();
            deconstructFinish += bot.deconstructFinishRx.sum();
            unitDeath += bot.unitDeathRx.sum();
            createBullet += bot.createBulletRx.sum();
            pingsTx += bot.pingsTx.sum();
            pingSamples += bot.pingSamples.sum();
            pingTotalMs += bot.pingTotalMs.sum();
            pingMaxMs = Math.max(pingMaxMs, bot.pingMaxMs.get());
            if (!bot.failReason.isEmpty() && failures.size() < 12) {
                failures.add(bot.name + "=" + bot.failReason);
            }
            try {
                bot.client.stop();
            } catch (Throwable ignored) {
            }
        }

        long[] sortedJoin = joinMs.stream().mapToLong(Long::longValue).sorted().toArray();
        double avgJoin = sortedJoin.length == 0
            ? -1.0
            : Arrays.stream(sortedJoin).average().orElse(-1.0);
        double avgPing = pingSamples == 0 ? -1.0 : (double)pingTotalMs / (double)pingSamples;
        long elapsedMs = (System.nanoTime() - startNs) / 1_000_000L;

        StringBuilder failureJson = new StringBuilder("[");
        for (int i = 0; i < failures.size(); i++) {
            if (i > 0) {
                failureJson.append(',');
            }
            failureJson.append('"').append(jsonEscape(failures.get(i))).append('"');
        }
        failureJson.append(']');

        String json = String.format(
            Locale.US,
            "{"
                + "\"host\":\"%s\","
                + "\"port\":%d,"
                + "\"players_requested\":%d,"
                + "\"connect_failures\":%d,"
                + "\"confirmed\":%d,"
                + "\"joined\":%d,"
                + "\"kicked\":%d,"
                + "\"still_connected\":%d,"
                + "\"join_ms_avg\":%.2f,"
                + "\"join_ms_p50\":%d,"
                + "\"join_ms_p95\":%d,"
                + "\"join_ms_p99\":%d,"
                + "\"join_ms_max\":%d,"
                + "\"ramp_complete_ms\":%.1f,"
                + "\"elapsed_ms\":%d,"
                + "\"packets_rx\":%d,"
                + "\"entity_snapshots_rx\":%d,"
                + "\"state_snapshots_rx\":%d,"
                + "\"snapshots_tx\":%d,"
                + "\"build_snapshots_tx\":%d,"
                + "\"break_snapshots_tx\":%d,"
                + "\"shoot_snapshots_tx\":%d,"
                + "\"begin_place_rx\":%d,"
                + "\"construct_finish_rx\":%d,"
                + "\"begin_break_rx\":%d,"
                + "\"deconstruct_finish_rx\":%d,"
                + "\"unit_death_rx\":%d,"
                + "\"create_bullet_rx\":%d,"
                + "\"pings_tx\":%d,"
                + "\"ping_samples\":%d,"
                + "\"ping_rtt_ms_avg\":%.2f,"
                + "\"ping_rtt_ms_max\":%d,"
                + "\"snapshot_hz\":%d,"
                + "\"warmup_sec\":%d,"
                + "\"duration_sec\":%d,"
                + "\"build\":%d,"
                + "\"uuid_bytes\":%d,"
                + "\"workload\":\"%s\","
                + "\"block_id\":%d,"
                + "\"failures\":%s"
                + "}",
            jsonEscape(host),
            port,
            players,
            connectFailures,
            confirmed,
            joined,
            kicked,
            stillConnected,
            avgJoin,
            percentile(sortedJoin, 0.50),
            percentile(sortedJoin, 0.95),
            percentile(sortedJoin, 0.99),
            sortedJoin.length == 0 ? -1L : sortedJoin[sortedJoin.length - 1],
            (allJoinedNs - startNs) / 1_000_000.0,
            elapsedMs,
            packetsRx,
            entityRx,
            stateRx,
            snapshotsTx,
            buildTx,
            breakTx,
            shootTx,
            beginPlace,
            constructFinish,
            beginBreak,
            deconstructFinish,
            unitDeath,
            createBullet,
            pingsTx,
            pingSamples,
            avgPing,
            pingMaxMs,
            snapshotHz,
            warmupSec,
            durationSec,
            build,
            uuidBytes,
            jsonEscape(workload),
            blockId,
            failureJson
        );
        System.out.println("BENCH_JSON " + json);
        if (joined < players) {
            System.err.println("warning: only " + joined + "/" + players + " joined");
            System.exit(2);
        }
    }
}
