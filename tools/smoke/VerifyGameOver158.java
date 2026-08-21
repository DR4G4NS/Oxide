import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.util.Base64;

import arc.util.io.Reads;
import arc.util.io.Writes;
import mindustry.gen.GameOverCallPacket;
import mindustry.game.Team;
import arc.Core;
import arc.Settings;
import arc.struct.Seq;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.NetClient;
import mindustry.core.Version;
import mindustry.gen.Groups;
import mindustry.gen.Player;
import mindustry.net.Host;
import mindustry.net.Net;
import mindustry.net.NetConnection;

/**
 * Verifies that the Rust GameOverCallPacket payload (one byte: b teamId = 2,
 * the waveTeam/crux winner in survival) decodes with the official desktop
 * 158.1 client API without exception.
 *
 * Usage: java -cp desktop.jar:. VerifyGameOver158
 *   (no arguments: verifies the fixed payload byte 0x02; optional arguments
 *    are treated as hex bytes of the payload to verify)
 */
public final class VerifyGameOver158 {
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
        // Minimal 158.1 client environment so Logic.gameOver() (the packet's
        // handleClient target) has Vars.state / Vars.player / Vars.netClient.
        Version.build = 158;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.net = clientNet();
        Vars.netClient = new NetClient();
        Vars.player = Player.create();
        Groups.init();

        byte[] payload;
        if (args.length == 0) {
            payload = new byte[] { 0x02 };
        } else {
            payload = new byte[args.length];
            for (int i = 0; i < args.length; i++) {
                payload[i] = (byte) Integer.parseInt(args[i], 16);
            }
        }

        // Official 158.1 client-side decode path:
        // GameOverCallPacket.read() stores DATA, handled() runs
        // TypeIO.readTeam -> Reads.b() -> Team.get(id).
        GameOverCallPacket packet = new GameOverCallPacket();
        packet.read(Reads.get(new DataInputStream(new ByteArrayInputStream(payload))),
            payload.length);
        packet.handled();

        Team winner = packet.winner;
        if (winner == null) {
            throw new AssertionError("GameOverCallPacket.handled() decoded a null Team");
        }
        System.out.println("decoded winner=" + winner.toString() + " id=" + winner.id);

        // The winner must be the waveTeam in survival: crux, id 2. The client
        // uses it for state.won = player.team() == winner (defeat for team 1).
        if (winner.id != 2 || winner != Team.crux) {
            throw new AssertionError("expected Team.crux (id 2), got " + winner);
        }

        // Round-trip: official write() must reproduce the same single byte.
        ByteArrayOutputStream buffer = new ByteArrayOutputStream();
        packet.write(new Writes(new DataOutputStream(buffer)));
        byte[] written = buffer.toByteArray();
        if (written.length != 1 || (written[0] & 0xff) != 2) {
            throw new AssertionError("write() did not reproduce b teamId=2: "
                + Base64.getEncoder().encodeToString(written));
        }
        System.out.println("write() round-trip ok: "
            + Base64.getEncoder().encodeToString(written));

        // handleClient() must not throw when the packet is dispatched (the
        // net client's gameOver handler sets state.won = player.team() == winner).
        packet.handleClient();
        System.out.println("handleClient() ok: GameOverCallPacket fully processed by 158.1 client");
        System.out.println("ok gameOverDecode=true winner=crux id=2 handleClient=true");
    }
}
