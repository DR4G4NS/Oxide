import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.world.Block;

/** Emits the exact block flags used by Tile.staticDarkness()/legSolid(). */
public final class InspectBlockPathing {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("# Official Mindustry 8: block ID, synthetic, fillsTile, forceDark.");
        for (Block block : Vars.content.blocks()) {
            System.out.printf("%d\t%b\t%b\t%b%n", block.id, block.synthetic(), block.fillsTile, block.forceDark);
        }
    }
}
