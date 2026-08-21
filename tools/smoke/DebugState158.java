import java.io.ByteArrayOutputStream;
import java.lang.reflect.Field;
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
import mindustry.gen.StateSnapshotCallPacket;
import mindustry.net.ArcNetProvider;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;
import mindustry.net.Packets;

public final class DebugState158 {
    private static final Field STATE_DATA;
    static {
        try {
            STATE_DATA = StateSnapshotCallPacket.class.getDeclaredField("DATA");
            STATE_DATA.setAccessible(true);
        } catch (ReflectiveOperationException error) {
            throw new ExceptionInInitializerError(error);
        }
    }

    public static void main(String[] args) throws Exception {
        int port = args.length == 0 ? 6567 : Integer.parseInt(args[0]);
        Version.build = 158;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.net = new Net(new Net.NetProvider() {
            public void connectClient(String ip, int port, Runnable success) {}
            public void sendClient(Object object, boolean reliable) {}
            public void disconnectClient() {}
            public void discoverServers(arc.func.Cons<Host> found, Runnable done) { done.run(); }
            public void pingHost(String address, int port, arc.func.Cons<Host> valid, arc.func.Cons<Exception> failed) {}
            public void hostServer(int port) {}
            public Iterable<? extends NetConnection> getConnections() { return java.util.List.of(); }
            public void closeServer() {}
        });
        Client client = new Client(16384, 16384, new ArcNetProvider.PacketSerializer());
        CountDownLatch joined = new CountDownLatch(1);
        AtomicInteger streamId = new AtomicInteger(-1);
        AtomicInteger streamTotal = new AtomicInteger(-1);
        ByteArrayOutputStream world = new ByteArrayOutputStream();
        AtomicBoolean confirmed = new AtomicBoolean();
        client.addListener(new NetListener() {
            public void connected(Connection connection) {
                Packets.ConnectPacket packet = new Packets.ConnectPacket();
                packet.version = 158;
                packet.versionType = "official";
                packet.mods = new Seq<>();
                packet.name = "debug-state";
                packet.locale = "en";
                packet.uuid = "AQIDBAUGBwg=";
                packet.usid = "";
                packet.mobile = false;
                packet.color = 0xffa665ff;
                connection.sendTCP(packet);
            }
            public synchronized void received(Connection connection, Object object) {
                if (object instanceof Packets.StreamBegin begin) {
                    streamId.set(begin.id);
                    streamTotal.set(begin.total);
                    world.reset();
                } else if (object instanceof Packets.StreamChunk chunk && chunk.id == streamId.get()) {
                    world.writeBytes(chunk.data);
                    if (world.size() == streamTotal.get() && confirmed.compareAndSet(false, true)) {
                        connection.sendTCP(new mindustry.gen.ConnectConfirmCallPacket());
                        joined.countDown();
                    }
                } else if (object instanceof StateSnapshotCallPacket state) {
                    try {
                        byte[] data = (byte[]) STATE_DATA.get(state);
                        System.out.println("STATE len=" + data.length + " hex=" + hex(data));
                    } catch (IllegalAccessException error) {
                        throw new RuntimeException(error);
                    }
                }
            }
            public void disconnected(Connection connection, DcReason reason) {
                System.out.println("DISCONNECTED reason=" + reason + " protocolError=" + connection.getLastProtocolError());
            }
        });
        client.start();
        client.connect(5000, "127.0.0.1", port, port);
        joined.await(20, TimeUnit.SECONDS);
        Thread.sleep(1500);
        client.stop();
    }

    static String hex(byte[] data) {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < data.length; i++) {
            sb.append(String.format("%02x", data[i]));
            if ((i + 1) % 16 == 0) sb.append('\n');
            else sb.append(' ');
        }
        return sb.toString();
    }
}
