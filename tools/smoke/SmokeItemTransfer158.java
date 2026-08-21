import java.io.ByteArrayOutputStream;
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
import mindustry.content.Blocks;
import mindustry.content.Items;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.ConnectConfirmCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.RequestItemCallPacket;
import mindustry.gen.TakeItemsCallPacket;
import mindustry.gen.TransferInventoryCallPacket;
import mindustry.gen.TransferItemToCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;

/** Withdraws from and deposits into the core through exact desktop 158.1 RPCs. */
public final class SmokeItemTransfer158 {
    private static final int CORE = (40 << 16) | 100;

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
        int port = args.length == 0 ? 6584 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.net = clientNet();
        Groups.init();
        Vars.world.resize(128, 128).fill();
        Vars.world.tile(40, 100).setBlock(Blocks.coreShard, Team.sharded, 0);
        Vars.world.tile(40, 100).build.items.add(Items.copper, 100);

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicInteger unitId = new AtomicInteger(-1);
        AtomicBoolean requested = new AtomicBoolean();
        AtomicBoolean withdrew = new AtomicBoolean();
        CountDownLatch verified = new CountDownLatch(1);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "item-transfer-158";
                packet.locale = "en";
                packet.uuid = "QUJDREVGR0g=";
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
                    } else if (object instanceof PlayerSpawnCallPacket spawn
                            && requested.compareAndSet(false, true)) {
                        unitId.set(2_000_000 + connection.getID());
                        var localUnit = UnitTypes.alpha.create(Team.sharded);
                        localUnit.id(unitId.get());
                        localUnit.set(320f, 800f);
                        Groups.unit.add(localUnit);
                        RequestItemCallPacket request = new RequestItemCallPacket();
                        request.build = Vars.world.tile(40, 100).build;
                        request.item = Items.copper;
                        request.amount = 7;
                        connection.sendTCP(request);
                    } else if (object instanceof TakeItemsCallPacket packet) {
                        verifyTake(packet, unitId.get());
                        packet.handleClient();
                        var unit = Groups.unit.getByID(unitId.get());
                        if (unit == null || unit.item() != Items.copper || unit.stack.amount != 7) {
                            throw new AssertionError("TakeItems did not update the client unit stack");
                        }
                        withdrew.set(true);
                        TransferInventoryCallPacket deposit = new TransferInventoryCallPacket();
                        deposit.build = Vars.world.tile(40, 100).build;
                        connection.sendTCP(deposit);
                    } else if (object instanceof TransferItemToCallPacket packet) {
                        verifyDeposit(packet, unitId.get());
                        packet.handleClient();
                        var unit = Groups.unit.getByID(unitId.get());
                        if (unit == null || unit.stack.amount != 0
                                || Vars.world.tile(40, 100).build.items.get(Items.copper) != 100) {
                            throw new AssertionError("TransferItemTo did not complete the client round trip");
                        }
                        verified.countDown();
                    }
                } catch (Throwable error) {
                    error.printStackTrace();
                    verified.countDown();
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (verified.getCount() != 0) {
                    System.err.println("disconnected before item transfer verification: " + reason);
                    verified.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!verified.await(10, TimeUnit.SECONDS)) {
                throw new AssertionError("timed out waiting for item deposit");
            }
            if (!client.isConnected() || !withdrew.get()) {
                throw new AssertionError(
                    "item path failed: connected=" + client.isConnected()
                        + " withdrew=" + withdrew.get());
            }
            Thread.sleep(1200);
            System.out.println(
                "ok requestItem=true takeItems=7 transferInventory=true"
                    + " transferItemTo=7 coreBalance=100");
        } finally {
            client.stop();
        }
    }

    private static void verifyTake(TakeItemsCallPacket packet, int expectedUnit)
            throws Exception {
        packet.handled();
        if (packet.build == null || packet.build.pos() != CORE || packet.item != Items.copper
                || packet.amount != 7 || packet.to == null || packet.to.id != expectedUnit) {
            throw new AssertionError(
                "official TakeItems decode failed: build=" + packet.build
                    + " item=" + packet.item + " amount=" + packet.amount
                    + " unit=" + packet.to);
        }
    }

    private static void verifyDeposit(TransferItemToCallPacket packet, int expectedUnit)
            throws Exception {
        packet.handled();
        if (packet.unit == null || packet.unit.id != expectedUnit
                || packet.item != Items.copper || packet.amount != 7
                || packet.x != 320f || packet.y != 800f
                || packet.build == null || packet.build.pos() != CORE) {
            throw new AssertionError(
                "official TransferItemTo decode failed: unit=" + packet.unit
                    + " item=" + packet.item + " amount=" + packet.amount
                    + " x=" + packet.x + " y=" + packet.y
                    + " build=" + packet.build);
        }
    }
}
