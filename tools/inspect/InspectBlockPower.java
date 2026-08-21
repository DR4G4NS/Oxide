import arc.Core;
import arc.Settings;
import mindustry.Vars;
import mindustry.core.GameState;
import mindustry.core.ContentLoader;
import mindustry.gen.Building;
import mindustry.world.Block;

/** Emits the exact per-block module flags used by Building.writeBase/readBase. */
public final class InspectBlockPower {
    public static void main(String[] args) {
        Core.settings = new Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.content.init();
        System.out.println("# Official Mindustry 8: block ID, buildClass, version, hasItems, hasPower, hasLiquids, connectedPower, insulated, consumesPower, outputsPower, conductivePower.");
        for (Block block : Vars.content.blocks()) {
            Building sample = block.newBuilding();
            System.out.printf("%d\t%s\t%d\t%b\t%b\t%b\t%b\t%b\t%b\t%b\t%b%n",
                block.id,
                sample.getClass().getSimpleName(),
                sample.version(),
                block.hasItems,
                block.hasPower,
                block.hasLiquids,
                block.connectedPower,
                block.insulated,
                block.consumesPower,
                block.outputsPower,
                block.conductivePower);
        }
    }
}
