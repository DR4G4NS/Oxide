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
import mindustry.content.Liquids;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.BlockSnapshotCallPacket;
import mindustry.gen.BuildHealthUpdateCallPacket;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.RequestBlockSnapshotCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;
import mindustry.world.blocks.defense.RegenProjector.RegenProjectorBuild;
import mindustry.world.modules.LiquidModule;

/** Verifies RegenProjector base sync and continuous healing on desktop 158.1. */
public final class SmokeRegen158 {
    private static final int REGEN = (50 << 16) | 100;
    private static final int WALL = (54 << 16) | 100;

    private static Net clientNet() {
        return new Net(new Net.NetProvider() {
            public void connectClient(String ip, int port, Runnable success) {}
            public void sendClient(Object object, boolean reliable) {}
            public void disconnectClient() {}
            public void discoverServers(arc.func.Cons<Host> found, Runnable done) { done.run(); }
            public void pingHost(String address, int port, arc.func.Cons<Host> valid,
                                 arc.func.Cons<Exception> failed) {}
            public void hostServer(int port) {}
            public Iterable<? extends NetConnection> getConnections() {
                return java.util.List.of();
            }
            public void closeServer() {}
        });
    }

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6589 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.net = clientNet();
        Groups.init();
        Vars.world.resize(128, 128).fill();
        Vars.world.tile(50, 100).setBlock(Blocks.regenProjector, Team.sharded, 0);

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicBoolean requested = new AtomicBoolean();
        AtomicBoolean snapshotVerified = new AtomicBoolean();
        AtomicBoolean healingVerified = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(2);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "regen-158";
                packet.locale = "en";
                packet.uuid = "cmVnZW4xNTg=";
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
                            && requested.compareAndSet(false, true)) {
                        RequestBlockSnapshotCallPacket request =
                            new RequestBlockSnapshotCallPacket();
                        request.pos = REGEN;
                        connection.sendTCP(request);
                    } else if (object instanceof BlockSnapshotCallPacket packet) {
                        packet.handled();
                        if (packet.amount == 1 && verifyRegen(packet)
                                && snapshotVerified.compareAndSet(false, true)) {
                            verified.countDown();
                        }
                    } else if (object instanceof BuildHealthUpdateCallPacket packet) {
                        packet.handled();
                        for (int i = 0; i + 1 < packet.buildings.size; i += 2) {
                            if (packet.buildings.get(i) == WALL
                                    && Float.intBitsToFloat(packet.buildings.get(i + 1)) > 100f
                                    && healingVerified.compareAndSet(false, true)) {
                                verified.countDown();
                            }
                        }
                    }
                } catch (Throwable error) {
                    error.printStackTrace();
                    while (verified.getCount() > 0) verified.countDown();
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (verified.getCount() > 0) {
                    System.err.println("disconnected before Regen verification: " + reason);
                    while (verified.getCount() > 0) verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(5, TimeUnit.SECONDS) || !snapshotVerified.get()
                    || !healingVerified.get() || !client.isConnected()) {
                throw new AssertionError(
                    "Regen path failed: snapshot=" + snapshotVerified.get()
                        + " healing=" + healingVerified.get());
            }
            Thread.sleep(600);
            System.out.println(
                "ok regenProjector=true officialReadSync=true"
                    + " phaseFabric=true healing=true");
        } finally {
            client.stop();
        }
    }

    private static boolean verifyRegen(BlockSnapshotCallPacket packet) throws Exception {
        ByteArrayInputStream bytes = new ByteArrayInputStream(packet.data);
        DataInputStream input = new DataInputStream(bytes);
        int position = input.readInt();
        int block = input.readShort();
        if (position != REGEN || block != Blocks.regenProjector.id) return false;
        RegenProjectorBuild build =
            (RegenProjectorBuild)Vars.world.tile(50, 100).build;
        build.readSync(Reads.get(input), build.version());
        // Official 158.1 RegenProjectorBuild has NO write() override and no
        // LiquidModule: the hydrogen is a ConsumeLiquid (server-authoritative),
        // not part of the snapshot. Layout = base(bitmask 11) + ItemModule +
        // PowerModule + eff/optEff (19 bytes, verified against desktop.jar).
        if (build.power == null || build.power.status < 0.99f
                || build.items == null || build.items.get(Items.phaseFabric) != 1
                || bytes.available() != 0) {
            throw new AssertionError(
                "RegenProjectorBuild mismatch: power="
                    + (build.power == null ? null : build.power.status)
                    + " phase=" + (build.items == null ? null
                        : build.items.get(Items.phaseFabric))
                    + " remaining=" + bytes.available());
        }
        return true;
    }
}
