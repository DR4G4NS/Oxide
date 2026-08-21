import java.io.ByteArrayOutputStream;
import java.lang.reflect.Field;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

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
import mindustry.gen.ClientSnapshotCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.ConstructFinishCallPacket;
import mindustry.gen.EntitySnapshotCallPacket;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.gen.UnitDeathCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.Block;
import mindustry.world.Tile;

/**
 * Long end-to-end driver using Mindustry desktop 158.1's exact serializer.
 *
 * Modes:
 *   build  [port]        - fresh join: build economy (2 drills, conveyor
 *                          chains, 1 Duo), then monitor waves/enemies/items
 *                          until wave >= 60, copper grew, enemies seen and
 *                          killed, connection stable.
 *   verify [port] [savedWave] [savedCopper]
 *                        - fresh client after restart: report initial state,
 *                          then require the world to advance (wave grows 3+,
 *                          economy restored, enemies appear) while staying
 *                          connected; copper may dip while the Duo fires.
 *
 * Machine-readable protocol lines (printed to stdout):
 *   E2E READY <wave> <copper> <enemies>
 *   E2E PROGRESS <wave> <copper> <enemies> <deaths> <connected>
 *   E2E PHASE1_OK <wave> <copper> <enemies> <deaths>
 *   E2E PHASE1_FAIL <reason>
 *   E2E VERIFY_READY <wave> <copper> <enemies>
 *   E2E VERIFY_OK <wave> <copper> <enemies> <deaths>
 *   E2E VERIFY_FAIL <reason>
 */
public final class E2ELong158 {
    // Economy layout near the maze core (tile 40,100; core nucleus 5x5 at
    // x=38..42 y=98..102).  {x, y, block, rotation}
    static final int[][] ECONOMY = {
        {34, 101, 325, 0}, // mechanical drill (copper 2x2)
        {36, 101, 257, 0}, {37, 101, 257, 0}, // conveyor -> core (38,101)
        {33, 103, 325, 0}, // second mechanical drill (copper 2x2)
        {35, 103, 257, 0}, {36, 103, 257, 0}, {37, 103, 257, 0},
        {38, 103, 257, 1}, {38, 104, 257, 0}, // ammo chain south
        {39, 104, 349, 0}, // Duo
    };
    static final int ECONOMY_COST = 66;

    private static final Field STATE_DATA;
    private static final Field CONSTRUCT_DATA;

    static {
        try {
            STATE_DATA = StateSnapshotCallPacket.class.getDeclaredField("DATA");
            STATE_DATA.setAccessible(true);
            CONSTRUCT_DATA = ConstructFinishCallPacket.class.getDeclaredField("DATA");
            CONSTRUCT_DATA.setAccessible(true);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    /** Decodes the server state payload (Rust layout documented in tests). */
    static final class ServerState {
        float waveTime;
        int wave;
        int enemies;
        boolean paused;
        boolean gameOver;
        int copper;

        static ServerState decode(byte[] data) {
            ByteBuffer input = ByteBuffer.wrap(data).order(ByteOrder.BIG_ENDIAN);
            ServerState state = new ServerState();
            state.waveTime = input.getFloat();
            state.wave = input.getInt();
            state.enemies = input.getInt();
            state.paused = input.get() != 0;
            state.gameOver = input.get() != 0;
            input.getInt(); // timeData
            input.get();    // tps
            input.getLong(); // rand0
            input.getLong(); // rand1
            int coreLength = input.getShort() & 0xffff;
            if (coreLength > input.remaining()) {
                throw new IllegalArgumentException("coreData truncated");
            }
            ByteBuffer core = ByteBuffer.wrap(data, input.position(), coreLength)
                .order(ByteOrder.BIG_ENDIAN);
            int teams = core.get() & 0xff;
            for (int t = 0; t < teams; t++) {
                core.get(); // team id
                int count = core.getShort() & 0xffff;
                for (int i = 0; i < count; i++) {
                    int item = core.getShort();
                    int amount = core.getInt();
                    if (item == 0) {
                        state.copper = amount;
                    }
                }
            }
            return state;
        }
    }

    private static ClientSnapshotCallPacket snapshot(
            Connection connection, int snapshotId, Queue<BuildPlan> plans,
            boolean shooting) {
        ClientSnapshotCallPacket snapshot = new ClientSnapshotCallPacket();
        snapshot.snapshotID = snapshotId;
        snapshot.unitID = 2_000_000 + connection.getID();
        snapshot.dead = false;
        snapshot.x = 320f;
        snapshot.y = 800f;
        snapshot.pointerX = 400f;
        snapshot.pointerY = 800f;
        snapshot.rotation = 0f;
        snapshot.baseRotation = 0f;
        snapshot.xVelocity = 0f;
        snapshot.yVelocity = 0f;
        snapshot.mining = null;
        snapshot.boosting = false;
        snapshot.shooting = shooting;
        snapshot.chatting = false;
        snapshot.building = !plans.isEmpty();
        snapshot.selectedBlock = plans.isEmpty() ? null : plans.first().block;
        snapshot.selectedRotation = 0;
        snapshot.plans = plans;
        snapshot.viewX = 320f;
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

    private static Queue<BuildPlan> economyPlans() {
        Queue<BuildPlan> plans = new Queue<>();
        for (int[] plan : ECONOMY) {
            plans.add(new BuildPlan(plan[0], plan[1], plan[2] == 349 ? 0 : plan[3],
                Vars.content.block(plan[2]), null));
        }
        return plans;
    }

    private static boolean planMatches(BuildPlan plan, int[] target) {
        return plan.x == target[0] && plan.y == target[1] && plan.block.id == target[2];
    }

    private static boolean isEconomyTile(int position, int block) {
        int x = position >> 16;
        int y = position & 0xffff;
        for (int[] plan : ECONOMY) {
            if (x == plan[0] && y == plan[1] && block == plan[2]) {
                return true;
            }
        }
        return false;
    }

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6590 : Integer.parseInt(args[0]);
        String mode = args.length < 2 ? "build" : args[1];
        int savedWave = args.length > 2 ? Integer.parseInt(args[2]) : 0;
        int savedCopper = args.length > 3 ? Integer.parseInt(args[3]) : 0;
        Version.build = 158;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.net = clientNet();

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        CountDownLatch joined = new CountDownLatch(1);
        AtomicBoolean confirmed = new AtomicBoolean();
        AtomicInteger buildsSeen = new AtomicInteger();
        AtomicInteger enemyPeak = new AtomicInteger();
        AtomicInteger deaths = new AtomicInteger();
        AtomicInteger playerSpawns = new AtomicInteger();
        AtomicLong lastStateAt = new AtomicLong();
        AtomicReference<ServerState> latest = new AtomicReference<>();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicReference<Queue<BuildPlan>> plans = new AtomicReference<>(new Queue<>());
        AtomicBoolean shooting = new AtomicBoolean();

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.version = 158;
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "e2e-" + mode;
                packet.locale = "en";
                packet.uuid = mode.equals("verify") ? "CQoLDA0ODxA=" : "AQIDBAUGBwg=";
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
                        joined.countDown();
                        if (mode.equals("build")) {
                            plans.set(economyPlans());
                        }
                        Thread sender = new Thread(() -> {
                            int id = 1;
                            try {
                                while (!Thread.currentThread().isInterrupted()) {
                                    connection.sendUDP(snapshot(
                                        connection, id++, plans.get(), shooting.get()));
                                    Thread.sleep(100);
                                }
                            } catch (InterruptedException interrupted) {
                                Thread.currentThread().interrupt();
                            } catch (RuntimeException error) {
                                error.printStackTrace(System.err);
                                System.exit(3);
                            }
                        }, "e2e-snapshots");
                        sender.setDaemon(true);
                        sender.start();
                    }
                } else if (object instanceof StateSnapshotCallPacket state) {
                    try {
                        byte[] data = (byte[]) STATE_DATA.get(state);
                        ServerState decoded = ServerState.decode(data);
                        latest.set(decoded);
                        lastStateAt.set(System.currentTimeMillis());
                    } catch (IllegalAccessException error) {
                        throw new RuntimeException(error);
                    }
                } else if (object instanceof EntitySnapshotCallPacket entities) {
                    // read() only stores the raw payload; amount is decoded
                    // by handled() (NetClient.entitySnapshot would do this).
                    try {
                        entities.handled();
                        int seen = entities.amount;
                        enemyPeak.accumulateAndGet(seen, Math::max);
                    } catch (Exception error) {
                        error.printStackTrace(System.err);
                    }
                } else if (object instanceof UnitDeathCallPacket death) {
                    death.handled();
                    deaths.incrementAndGet();
                } else if (object instanceof ConstructFinishCallPacket finish) {
                    // read() only stores the raw payload; decode the packed
                    // tile + block id directly (Rust layout: i32 pos, i16
                    // block, unit ref, rotation, team, config).
                    try {
                        byte[] payload = (byte[]) CONSTRUCT_DATA.get(finish);
                        ByteBuffer input = ByteBuffer.wrap(payload)
                            .order(ByteOrder.BIG_ENDIAN);
                        int position = input.getInt();
                        int block = input.getShort();
                        if (isEconomyTile(position, block)) {
                            buildsSeen.incrementAndGet();
                        }
                    } catch (IllegalAccessException error) {
                        throw new RuntimeException(error);
                    }
                } else if (object instanceof PlayerSpawnCallPacket) {
                    playerSpawns.incrementAndGet();
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                System.err.println(
                    "e2e disconnected: " + reason
                        + " protocolError=" + connection.getLastProtocolError());
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!joined.await(20, TimeUnit.SECONDS)) {
                throw new AssertionError(
                    "timed out: world=" + world.size() + "/" + streamTotal.get()
                        + " confirmed=" + confirmed.get()
                        + " protocolError=" + client.getLastProtocolError());
            }
            ServerState initial = null;
            long joinDeadline = System.currentTimeMillis() + 10_000;
            while (initial == null) {
                if (System.currentTimeMillis() > joinDeadline) {
                    throw new AssertionError(
                        "no state snapshot after join; connected=" + client.isConnected());
                }
                initial = latest.get();
                if (initial == null) {
                    Thread.sleep(50);
                }
            }
            System.out.println("E2E " + mode + " joined wave=" + initial.wave
                + " copper=" + initial.copper + " enemies=" + initial.enemies);

            if (mode.equals("verify")) {
                runVerify(client, latest, enemyPeak, deaths, playerSpawns,
                    savedWave, savedCopper);
            } else {
                runBuild(client, latest, buildsSeen, enemyPeak, deaths,
                    playerSpawns, plans, shooting);
            }
        } finally {
            client.stop();
        }
    }

    private static void runBuild(
            Client client, AtomicReference<ServerState> latest,
            AtomicInteger buildsSeen, AtomicInteger enemyPeak,
            AtomicInteger deaths, AtomicInteger playerSpawns,
            AtomicReference<Queue<BuildPlan>> plans, AtomicBoolean shooting)
            throws Exception {
        long deadline = System.currentTimeMillis() + 60_000;
        while (buildsSeen.get() < ECONOMY.length) {
            if (System.currentTimeMillis() > deadline) {
                System.out.println("E2E PHASE1_FAIL buildsSeen="
                    + buildsSeen.get() + "/" + ECONOMY.length
                    + " last=" + latest.get().wave + "/" + latest.get().copper);
                System.exit(1);
            }
            Thread.sleep(200);
        }
        // Economy in place: stop building, start shooting at approaching units.
        plans.set(new Queue<>());
        shooting.set(true);
        ServerState ready = latest.get();
        int readyCopper = ready.copper;
        System.out.println("E2E READY " + ready.wave + " " + ready.copper
            + " " + ready.enemies + " builds=" + buildsSeen.get());

        long started = System.currentTimeMillis();
        long lastProgress = 0;
        while (System.currentTimeMillis() - started < 320_000) {
            ServerState state = latest.get();
            long now = System.currentTimeMillis();
            if (now - lastProgress >= 5_000) {
                lastProgress = now;
                System.out.println("E2E PROGRESS " + state.wave + " " + state.copper
                    + " " + enemyPeak.get() + " " + deaths.get()
                    + " connected=" + client.isConnected());
            }
            if (!client.isConnected()) {
                System.out.println("E2E PHASE1_FAIL disconnected");
                System.exit(1);
            }
            if (state.gameOver) {
                System.out.println("E2E PHASE1_FAIL gameOver wave=" + state.wave);
                System.exit(1);
            }
            if (state.wave >= 60 && state.copper > readyCopper
                    && enemyPeak.get() >= 3 && deaths.get() >= 2
                    && playerSpawns.get() >= 1) {
                System.out.println("E2E PHASE1_OK wave=" + state.wave
                    + " copper=" + state.copper + " enemies=" + enemyPeak.get()
                    + " deaths=" + deaths.get() + " readyCopper=" + readyCopper
                    + " connected=" + client.isConnected());
                return;
            }
            Thread.sleep(200);
        }
        ServerState state = latest.get();
        System.out.println("E2E PHASE1_FAIL timeout wave=" + state.wave
            + " copper=" + state.copper + " enemyPeak=" + enemyPeak.get()
            + " deaths=" + deaths.get());
        System.exit(1);
    }

    private static void runVerify(
            Client client, AtomicReference<ServerState> latest,
            AtomicInteger enemyPeak, AtomicInteger deaths,
            AtomicInteger playerSpawns, int savedWave, int savedCopper)
            throws Exception {
        Thread.sleep(1_000);
        ServerState initial = latest.get();
        int initialCopper = initial.copper;
        if (initial.wave < savedWave) {
            System.out.println("E2E VERIFY_FAIL wave=" + initial.wave
                + " savedWave=" + savedWave);
            System.exit(1);
        }
        if (initial.copper < savedCopper - 20) {
            System.out.println("E2E VERIFY_FAIL copper=" + initial.copper
                + " savedCopper=" + savedCopper);
            System.exit(1);
        }
        System.out.println("E2E VERIFY_READY wave=" + initial.wave
            + " copper=" + initial.copper + " enemies=" + initial.enemies);

        long started = System.currentTimeMillis();
        long lastProgress = 0;
        while (System.currentTimeMillis() - started < 180_000) {
            ServerState state = latest.get();
            long now = System.currentTimeMillis();
            if (now - lastProgress >= 5_000) {
                lastProgress = now;
                System.out.println("E2E PROGRESS " + state.wave + " " + state.copper
                    + " " + enemyPeak.get() + " " + deaths.get()
                    + " connected=" + client.isConnected());
            }
            if (!client.isConnected()) {
                System.out.println("E2E VERIFY_FAIL disconnected");
                System.exit(1);
            }
            if (state.gameOver) {
                System.out.println("E2E VERIFY_FAIL gameOver wave=" + state.wave);
                System.exit(1);
            }
            if (state.wave >= savedWave + 3 && state.copper >= initialCopper - 40
                    && enemyPeak.get() >= 1) {
                System.out.println("E2E VERIFY_OK wave=" + state.wave
                    + " copper=" + state.copper + " enemies=" + enemyPeak.get()
                    + " deaths=" + deaths.get() + " initialCopper=" + initialCopper
                    + " connected=" + client.isConnected());
                return;
            }
            Thread.sleep(200);
        }
        ServerState state = latest.get();
        System.out.println("E2E VERIFY_FAIL timeout wave=" + state.wave
            + " copper=" + state.copper + " enemyPeak=" + enemyPeak.get());
        System.exit(1);
    }
}
