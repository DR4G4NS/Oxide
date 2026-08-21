import mindustry.Vars;
import arc.Core;
import arc.Settings;
import mindustry.core.ContentLoader;
import mindustry.gen.WorldDataBeginCallPacket;
import mindustry.net.Net;

/** Prints WorldDataBeginCallPacket ID from the supplied desktop JAR (build 158.1). */
public final class InspectWorldDataBegin {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Core.settings = new Settings();
        Vars.content.createBaseContent();
        Vars.content.init();
        System.out.printf("WorldDataBeginCallPacket=%d%n",
            Byte.toUnsignedInt(Net.getPacketId(new WorldDataBeginCallPacket())));
    }
}
