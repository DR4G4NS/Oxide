import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.world.Block;

/** Emits exact fields used by Block.canReplace()/Build.validPlace(). */
public final class InspectBlockPlacement {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("# ID size group replaceable alwaysReplace rotate quickRotate privileged");
        for (Block block : Vars.content.blocks()) {
            System.out.printf("%d\t%d\t%s\t%b\t%b\t%b\t%b\t%b%n",
                block.id, block.size, block.group.name(), block.replaceable,
                block.alwaysReplace, block.rotate, block.quickRotate, block.privileged);
        }
    }
}
