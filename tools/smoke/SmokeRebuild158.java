import java.io.ByteArrayOutputStream;
import java.lang.reflect.Field;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import arc.files.Fi;
import arc.net.Client;
import arc.net.Connection;
import arc.net.DcReason;
import arc.net.NetListener;
import arc.struct.Seq;
import mindustry.ai.UnitCommand;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.core.Control;
import mindustry.core.GameState;
import mindustry.core.NetClient;
import mindustry.core.Version;
import mindustry.core.World;
import mindustry.gen.BeginPlaceCallPacket;
import mindustry.gen.ConstructFinishCallPacket;
import mindustry.gen.Groups;
import mindustry.gen.Player;
import mindustry.gen.PlayerSpawnCallPacket;
import mindustry.gen.SetUnitCommandCallPacket;
import mindustry.input.DesktopInput;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;

/** Real ArcNet verification of autonomous BuilderAI-compatible rebuild. */
public final class SmokeRebuild158 {
    private static final int POLY_ID = 3_100_021;

    private static <T> T allocateWithoutConstructor(Class<T> type) throws Exception {
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe)field.get(null);
        return type.cast(unsafe.allocateInstance(type));
    }

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
        int port = args.length == 0 ? 6594 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.world = new World();
        Vars.player = Player.create();
        Vars.control = allocateWithoutConstructor(Control.class);
        Vars.control.input = allocateWithoutConstructor(DesktopInput.class);
        Vars.net = clientNet();
        Vars.netClient = new NetClient();
        Vars.customMapDirectory = new Fi("/tmp");
        Groups.init();

        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        AtomicBoolean commandSent = new AtomicBoolean();
        AtomicBoolean beginSeen = new AtomicBoolean();
        AtomicBoolean finishSeen = new AtomicBoolean();
        CountDownLatch finished = new CountDownLatch(1);

        client.addListener(new NetListener() {
            @Override
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "rebuild-158";
                packet.locale = "en";
                packet.uuid = "cmVidWlsZDE=";
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
                            connection.sendTCP(new mindustry.gen.ConnectConfirmCallPacket());
                        }
                    } else if (object instanceof PlayerSpawnCallPacket
                            && commandSent.compareAndSet(false, true)) {
                        SetUnitCommandCallPacket command = new SetUnitCommandCallPacket();
                        command.unitIds = new int[]{POLY_ID};
                        command.command = UnitCommand.rebuildCommand;
                        connection.sendTCP(command);
                    } else if (object instanceof BeginPlaceCallPacket packet) {
                        packet.handled();
                        if (packet.result != null && packet.result.id == 216
                                && packet.x == 50 && packet.y == 100
                                && packet.rotation == 2) {
                            beginSeen.set(true);
                        }
                    } else if (object instanceof ConstructFinishCallPacket packet) {
                        packet.handled();
                        if (packet.block != null && packet.block.id == 216
                                && packet.rotation == 2 && packet.team.id == 1) {
                            finishSeen.set(true);
                            finished.countDown();
                        }
                    }
                } catch (Throwable error) {
                    error.printStackTrace();
                    finished.countDown();
                }
            }

            @Override
            public void disconnected(Connection connection, DcReason reason) {
                if (!finishSeen.get()) {
                    System.err.println("disconnected before rebuild: " + reason);
                    finished.countDown();
                }
            }
        });

        client.start();
        try {
            client.connect(5000, "127.0.0.1", port, port);
            if (!finished.await(12, TimeUnit.SECONDS) || !client.isConnected()
                    || !commandSent.get() || !beginSeen.get() || !finishSeen.get()) {
                throw new AssertionError(
                    "rebuild failed: connected=" + client.isConnected()
                        + " command=" + commandSent.get()
                        + " begin=" + beginSeen.get()
                        + " finish=" + finishSeen.get());
            }
            System.out.println(
                "ok rebuild=true beginPlace=true constructFinish=true builderPoly=true");
        } finally {
            client.stop();
        }
    }
}
