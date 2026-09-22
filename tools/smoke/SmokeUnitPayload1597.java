import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.lang.reflect.Field;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;
import java.util.zip.Inflater;

import arc.Core;
import arc.math.geom.Vec2;
import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Seq;
import arc.util.io.Reads;
import mindustry.Vars;
import mindustry.ai.BlockIndexer;
import mindustry.ai.Pathfinder;
import mindustry.ai.UnitCommand;
import mindustry.content.Blocks;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.Control;
import mindustry.core.GameState;
import mindustry.core.NetClient;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.game.Rules;
import mindustry.game.Team;
import mindustry.game.Teams;
import mindustry.gen.BlockSnapshotCallPacket;
import mindustry.gen.CommandBuildingCallPacket;
import mindustry.gen.CommandUnitsCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.EntitySnapshotCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.Player;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.RequestBlockSnapshotCallPacket;
import mindustry.gen.SetUnitCommandCallPacket;
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.gen.TileConfigCallPacket;
import mindustry.gen.UnitBlockSpawnCallPacket;
import mindustry.gen.UnitSpawnCallPacket;
import mindustry.input.DesktopInput;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.blocks.units.Reconstructor.ReconstructorBuild;
import mindustry.world.blocks.units.UnitFactory.UnitFactoryBuild;
import mindustry.world.modules.ItemModule;
import mindustry.world.modules.PowerModule;

/**
 * Current-target production smoke: enterPayload, finite-resource factory/reconstructor
 * chains, unit counts and plastanium withdrawal. Reads streamed Rules without
 * NetworkIO.readWorld and applies snapshots to the observed local buildings.
 */
public final class SmokeUnitPayload1597 {
    private static final boolean AIR_CYCLE = Boolean.getBoolean("oxide.smoke.airCycle");
    private static final boolean SURVIVAL_CYCLE = Boolean.getBoolean("oxide.smoke.survivalCycle");
    private static final int FX = 52;
    private static final int FY = 100;
    private static final int FACTORY = (FX << 16) | FY;
    private static final int RX = SURVIVAL_CYCLE ? 58 : 45;
    private static final int RY = 100;
    private static final int RECONSTRUCTOR = (RX << 16) | RY;
    private static final int SECOND_RECONSTRUCTOR = (62 << 16) | RY;
    private static final int BELT = (45 << 16) | 100;
    private static volatile int beltCount = -1;
    private static volatile boolean fullBeltSeen = false;
    private static final AtomicBoolean itemsTaken = new AtomicBoolean();
    private static final AtomicBoolean waveCountSeen = new AtomicBoolean();
    private static final AtomicBoolean waveUnitSeen = new AtomicBoolean();
    private static final java.util.Map<Integer, Float> progressByTile = new java.util.HashMap<>();

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6599 : Integer.parseInt(args[0]);
        Version.build = Integer.getInteger("oxide.smoke.build", 160);
        Vars.headless = true;
        Core.settings = new arc.Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Vars.state = new GameState();
        Vars.state.rules = new Rules();
        Vars.state.rules.unitCap = 100;
        Vars.state.teams = new Teams();
        Vars.world = new World();
        Vars.player = Player.create();
        Vars.control = allocateWithoutConstructor(Control.class);
        Vars.control.input = allocateWithoutConstructor(DesktopInput.class);
        Vars.net = clientNet();
        Vars.net.setClientConnected();
        Vars.netClient = new NetClient();
        Groups.init();
        Vars.world.resize(300, 300).fill();
        Vars.indexer = new BlockIndexer();
        Vars.controlPath = new mindustry.ai.ControlPathfinder();
        Vars.pathfinder = new Pathfinder();
        if (SURVIVAL_CYCLE) Vars.world.tile(40, 100).setBlock(Blocks.coreNucleus, Team.sharded, 0);
        Vars.world.tile(RX, RY).setBlock(Blocks.additiveReconstructor, Team.sharded, 0);
        Vars.world.tile(FX, FY).setBlock(AIR_CYCLE ? Blocks.airFactory : Blocks.groundFactory, Team.sharded, 0);
        if (AIR_CYCLE) Vars.world.tile(62, RY).setBlock(Blocks.multiplicativeReconstructor, Team.sharded, 0);
        if (SURVIVAL_CYCLE) {
            Vars.world.tile(45, 88).setBlock(Blocks.laserDrill, Team.sharded, 0);
            for (int y = 90; y <= 100; y++) Vars.world.tile(45, y).setBlock(Blocks.plastaniumConveyor, Team.sharded, 1);
        }

        Client client = new Client(32768, 32768, new ArcNetProvider.PacketSerializer());
        CountDownLatch spawned = new CountDownLatch(1);
        CountDownLatch done = new CountDownLatch(SURVIVAL_CYCLE ? 5 : 3);
        AtomicBoolean confirmed = new AtomicBoolean();
        AtomicBoolean rulesOk = new AtomicBoolean();
        AtomicBoolean factoryOk = new AtomicBoolean();
        AtomicBoolean absorbed = new AtomicBoolean();
        AtomicBoolean progressed = new AtomicBoolean();
        AtomicBoolean commandedOutside = new AtomicBoolean();
        AtomicBoolean maceReleased = new AtomicBoolean();
        AtomicBoolean deselectionSent = new AtomicBoolean();
        AtomicBoolean withdrawalSent = new AtomicBoolean();
        AtomicReference<Long> deselectedSince = new AtomicReference<>(-1L);
        AtomicReference<Throwable> failure = new AtomicReference<>();
        java.util.Set<Integer> commanded = java.util.concurrent.ConcurrentHashMap.newKeySet();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicInteger daggerId = new AtomicInteger(-1);
        AtomicReference<Float> lastProgress = new AtomicReference<>(-1f);
        AtomicReference<Connection> live = new AtomicReference<>();
        ByteArrayOutputStream world = new ByteArrayOutputStream();

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "smoke-unit-payload-1597";
                packet.locale = "en";
                packet.uuid = "c21va2UtcHk=";
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
                            String rules = readStreamedRules(world.toByteArray());
                            if (rules == null || rules.isEmpty()) {
                                throw new AssertionError("empty streamed rules");
                            }
                            if (SURVIVAL_CYCLE) {
                                Rules decoded = mindustry.io.JsonIO.read(Rules.class, rules);
                                if (!decoded.waves || decoded.infiniteResources) {
                                    throw new AssertionError("production cycle must use finite-resource survival rules");
                                }
                                Vars.state.rules = decoded;
                            }
                            rulesOk.set(true);
                            connection.sendTCP(new ConnectConfirmCallPacket());
                        }
                    } else if (object instanceof PlayerSpawnCallPacket) {
                        live.set(connection);
                        spawned.countDown();
                    } else if (object instanceof StateSnapshotCallPacket packet) {
                        packet.handled();
                        if (packet.enemies > 0) waveCountSeen.set(true);
                        live.set(connection);
                        spawned.countDown();
                    } else if (object instanceof mindustry.gen.TakeItemsCallPacket packet) {
                        packet.handled();
                        packet.handleClient();
                        if (packet.build != null && packet.build.pos() == BELT) {
                            if (packet.amount != 3 || packet.to == null || packet.to.stack.amount != 3) {
                                throw new AssertionError("withdrawal amount=" + packet.amount + " target=" + packet.to + " carried=" + (packet.to == null ? -1 : packet.to.stack.amount));
                            }
                            itemsTaken.set(true);
                        }
                    } else if (object instanceof UnitSpawnCallPacket packet) {
                        packet.handled();
                        packet.handleClient();
                        noteDagger(daggerId);
                    } else if (object instanceof UnitBlockSpawnCallPacket packet) {
                        packet.handled();
                        packet.handleClient();
                        noteDagger(daggerId);
                    } else if (object instanceof EntitySnapshotCallPacket packet) {
                        packet.handled();
                        packet.handleClient();
                        noteDagger(daggerId);
                        if (Groups.unit.find(unit -> unit.team == Vars.state.rules.waveTeam) != null) {
                            waveUnitSeen.set(true);
                        }
                        if (SURVIVAL_CYCLE && Groups.unit.find(unit -> unit.team == Team.sharded
                                && unit.type == (AIR_CYCLE ? UnitTypes.mega : UnitTypes.mace)) != null && maceReleased.compareAndSet(false, true)) {
                            done.countDown();
                        }
                    } else if (object instanceof BlockSnapshotCallPacket packet) {
                        packet.handled();
                        readSnapshot(packet, factoryOk, absorbed, progressed, lastProgress, deselectedSince, done);
                    }
                } catch (Throwable error) {
                    failure.compareAndSet(null, error);
                    error.printStackTrace();
                    while (done.getCount() > 0) {
                        done.countDown();
                    }
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (done.getCount() != 0) {
                    System.err.println("disconnected before payload smoke finished: " + reason);
                    while (done.getCount() > 0) {
                        done.countDown();
                    }
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
                throw new AssertionError("streamed rules were not read");
            }
            Connection connection = live.get();
            CommandBuildingCallPacket rally = new CommandBuildingCallPacket();
            rally.buildings = new int[]{FACTORY};
            rally.target = new Vec2(RX * 8f, RY * 8f);
            connection.sendTCP(rally);

            long deadline = System.currentTimeMillis() + (AIR_CYCLE ? 150_000 : SURVIVAL_CYCLE ? 90_000 : 20_000);
            while (System.currentTimeMillis() < deadline && done.getCount() > 0 && client.isConnected()) {
                if (SURVIVAL_CYCLE && maceReleased.get() && deselectionSent.compareAndSet(false, true)) {
                    TileConfigCallPacket clear = new TileConfigCallPacket();
                    clear.build = Vars.world.tile(FX, FY).build;
                    clear.value = Integer.valueOf(-1);
                    connection.sendTCP(clear);
                }
                if (!SURVIVAL_CYCLE) Groups.unit.each(unit -> {
                    if (unit.id != 3_000_000 || unit.team != Team.sharded || unit.type != UnitTypes.dagger) {
                        return;
                    }
                    daggerId.compareAndSet(-1, unit.id);
                    if (!commanded.add(unit.id)) {
                        return;
                    }
                    if (unit.dst(RX * 8f, RY * 8f) <= Blocks.additiveReconstructor.size * 4f + unit.hitSize / 2f) {
                        failure.compareAndSet(null, new AssertionError("dagger must begin outside reconstructor footprint"));
                        while (done.getCount() > 0) done.countDown();
                        return;
                    }
                    commandedOutside.set(true);
                    SetUnitCommandCallPacket setCommand = new SetUnitCommandCallPacket();
                    setCommand.unitIds = new int[]{unit.id};
                    setCommand.command = UnitCommand.enterPayloadCommand;
                    connection.sendTCP(setCommand);
                    CommandUnitsCallPacket command = new CommandUnitsCallPacket();
                    command.unitIds = new int[]{unit.id};
                    command.buildTarget = Vars.world.tile(RX, RY).build;
                    command.unitTarget = null;
                    command.posTarget = new Vec2(RX * 8f, RY * 8f);
                    command.queueCommand = false;
                    command.finalBatch = true;
                    connection.sendTCP(command);
                });
                if (SURVIVAL_CYCLE) {
                    if (beltCount == 10 && withdrawalSent.compareAndSet(false, true)) {
                        var request = new mindustry.gen.RequestItemCallPacket();
                        request.build = Vars.world.tile(45, 100).build;
                        request.item = mindustry.content.Items.silicon;
                        request.amount = 3;
                        connection.sendTCP(request);
                    }
                    var belt = new RequestBlockSnapshotCallPacket();
                    belt.pos = BELT;
                    connection.sendTCP(belt);
                }
                if (AIR_CYCLE) {
                    var second = new RequestBlockSnapshotCallPacket();
                    second.pos = SECOND_RECONSTRUCTOR;
                    connection.sendTCP(second);
                }
                RequestBlockSnapshotCallPacket factory = new RequestBlockSnapshotCallPacket();
                factory.pos = FACTORY;
                connection.sendTCP(factory);
                RequestBlockSnapshotCallPacket reconstructor = new RequestBlockSnapshotCallPacket();
                reconstructor.pos = RECONSTRUCTOR;
                connection.sendTCP(reconstructor);
                Thread.sleep(200);
            }
            if (failure.get() != null) {
                throw new AssertionError("client packet application failed", failure.get());
            }
            if ((!SURVIVAL_CYCLE && !commandedOutside.get()) || !factoryOk.get() || !absorbed.get()
                    || !progressed.get() || (SURVIVAL_CYCLE && !maceReleased.get()) || done.getCount() != 0) {
                throw new AssertionError(
                    "payload path failed: factory=" + factoryOk.get()
                        + " absorb=" + absorbed.get()
                        + " progress=" + progressed.get()
                        + " dagger=" + daggerId.get() + " maceReleased=" + maceReleased.get());
            }
            if (SURVIVAL_CYCLE) {
                Vars.state.teams.updateTeamStats();
                var type = AIR_CYCLE ? UnitTypes.mega : UnitTypes.mace;
                int count = Team.sharded.data().countType(type);
                if (count != 1) throw new AssertionError("expected one released " + type.name + ", counted " + count);
                if (!itemsTaken.get() || beltCount != 7) throw new AssertionError("plastanium items lost: " + beltCount);
                if (AIR_CYCLE && progressByTile.getOrDefault(SECOND_RECONSTRUCTOR, 0f) <= 360f) {
                    throw new AssertionError("multiplicative reconstructor did not cross a snapshot window");
                }
                System.out.println("SMOKE_OK unit-count type=" + type.name + " count=" + count
                    + " plastaniumWithdraw=3 remaining=7");
                if (Boolean.getBoolean("oxide.smoke.waves")) {
                    if (!waveCountSeen.get() || !waveUnitSeen.get()) {
                        throw new AssertionError("survival wave did not spawn and replicate its count");
                    }
                    System.out.println("SMOKE_OK survival-wave enemies=true counter=true");
                }
            }
            if (SURVIVAL_CYCLE) System.out.println("SMOKE_OK survival-production factory=true conveyor=true"
                + " reconstructorTicks360=true releasedUnit=" + (AIR_CYCLE ? "mega" : "mace")
                + " finiteResources=true deselectionAcrossSnapshot=true");
            else System.out.println(
                "SMOKE_OK unit-payload rules=true factorySnapshot=true"
                    + " commandedOutside=true enterPayload=true absorb=true reconstructorProgress=true");
        } finally {
            client.stop();
        }
    }

    private static void noteDagger(AtomicInteger daggerId) {
        var unit = Groups.unit.find(candidate ->
            candidate.team == Team.sharded && candidate.type == UnitTypes.dagger);
        if (unit != null) {
            daggerId.compareAndSet(-1, unit.id);
        }
    }

    private static void readSnapshot(
            BlockSnapshotCallPacket packet,
            AtomicBoolean factoryOk,
            AtomicBoolean absorbed,
            AtomicBoolean progressed,
            AtomicReference<Float> lastProgress,
            AtomicReference<Long> deselectedSince,
            CountDownLatch done) throws Exception {
        DataInputStream input = new DataInputStream(new ByteArrayInputStream(packet.data));
        for (int i = 0; i < packet.amount; i++) {
            int pos = input.readInt();
            short block = input.readShort();
            var content = Vars.content.block(block);
            var build = Vars.world.build(pos);
            if (build == null || build.block != content) build = content.newBuilding().create(content, Team.sharded);
            if (build.items == null) {
                build.items = new ItemModule();
            }
            if (build.power == null) {
                build.power = new PowerModule();
            }
            build.readSync(Reads.get(input), build.version());
            if (pos == FACTORY && build instanceof UnitFactoryBuild factory) {
                if (factory.currentPlan == (AIR_CYCLE ? 1 : 0) && (!SURVIVAL_CYCLE || factory.progress > 0f)
                        && factoryOk.compareAndSet(false, true)) {
                    done.countDown();
                }
                if (SURVIVAL_CYCLE) {
                    long since = deselectedSince.get();
                    if (factory.currentPlan == -1) {
                        if (since == -1L) deselectedSince.set(System.currentTimeMillis());
                        else if (since > 0L && System.currentTimeMillis() - since > 6500L) {
                            deselectedSince.set(-2L);
                            done.countDown();
                        }
                    } else if (since != -1L) {
                        throw new AssertionError("snapshot reselected factory plan " + factory.currentPlan);
                    }
                }
            }
            if (SURVIVAL_CYCLE && pos == BELT) {
                beltCount = build.items.get(mindustry.content.Items.silicon);
                var stack = (mindustry.world.blocks.distribution.StackConveyor.StackConveyorBuild)build;
                if (beltCount > 0 && (stack.link == -1 || Vars.world.tile(stack.link) == null || stack.lastItem == null)) {
                    throw new AssertionError("belt visual missing/moved on inspection: items=" + beltCount
                        + " link=" + stack.link + " cooldown=" + stack.cooldown + " item=" + stack.lastItem);
                }
                if (beltCount == 10) fullBeltSeen = true;
                if (fullBeltSeen && beltCount != 10 && beltCount != 7) {
                    throw new AssertionError("belt snapshot lost items: " + beltCount);
                }
            }
            if (AIR_CYCLE && pos == SECOND_RECONSTRUCTOR && build instanceof ReconstructorBuild second
                    && second.payload != null && second.payload.unit.type == UnitTypes.poly) {
                float previous = progressByTile.getOrDefault(pos, 0f);
                if (second.progress + 0.01f < previous) throw new AssertionError("multiplicative progress reset");
                progressByTile.put(pos, second.progress);
            }
            if (pos == RECONSTRUCTOR && build instanceof ReconstructorBuild reconstructor) {
                if (reconstructor.payload != null && absorbed.compareAndSet(false, true)) {
                    lastProgress.set(reconstructor.progress);
                    done.countDown();
                }
                Float previous = lastProgress.get();
                if (SURVIVAL_CYCLE && reconstructor.payload != null
                        && reconstructor.payload.unit.type == (AIR_CYCLE ? UnitTypes.mono : UnitTypes.dagger) && previous >= 0f) {
                    if (reconstructor.progress + 0.01f < previous) {
                        throw new AssertionError("reconstructor progress reset: " + previous + " -> " + reconstructor.progress);
                    }
                    if (reconstructor.efficiency <= 0f) throw new AssertionError("loaded reconstructor lost snapshot efficiency");
                    lastProgress.set(reconstructor.progress);
                }
                if (previous != null && previous >= 0f
                        && reconstructor.progress > previous + 0.5f
                        && (!SURVIVAL_CYCLE || reconstructor.progress > 360f)
                        && progressed.compareAndSet(false, true)) {
                    done.countDown();
                }
            }
        }
        if (input.available() != 0) throw new AssertionError("BlockSnapshot trailing bytes=" + input.available());
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
        String rules = input.readUTF();
        mindustry.io.JsonIO.read(mindustry.game.Rules.class, rules);
        return rules;
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

    private static <T> T allocateWithoutConstructor(Class<T> type) throws Exception {
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) field.get(null);
        return type.cast(unsafe.allocateInstance(type));
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
