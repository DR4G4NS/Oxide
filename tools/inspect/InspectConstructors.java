import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.game.Rules;
import mindustry.ai.BlockIndexer;
import mindustry.world.blocks.payloads.Constructor;

/** Prints the exact vanilla recipes accepted by Constructor blocks. */
public final class InspectConstructors {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.state = new GameState();
        Vars.state.rules = new Rules();
        Vars.indexer = new BlockIndexer();
        for (int constructorId : new int[]{406, 407}) {
            var constructor = (Constructor)Vars.content.block(constructorId);
            System.out.printf("constructor\t%d\t%f\t%f%n",
                constructorId, constructor.buildSpeed, constructor.consPower.usage);
            for (var block : Vars.content.blocks()) {
                // canProduce reads state.rules; reproduce its immutable content predicates here.
                boolean allowed = block.isVisible()
                    && block.size >= constructor.minBlockSize
                    && block.size <= constructor.maxBlockSize
                    && !(block instanceof mindustry.world.blocks.storage.CoreBlock)
                    && block.environmentBuildable()
                    && (constructor.filter.isEmpty() || constructor.filter.contains(block));
                if (allowed) {
                    String buildClass;
                    int revision;
                    try {
                        var build = block.buildType.get();
                        buildClass = build.getClass().getSimpleName();
                        revision = build.version();
                    } catch (Throwable failure) {
                        buildClass = "uninitialized";
                        revision = -1;
                    }
                    var line = new StringBuilder();
                    line.append(block.id).append('\t').append(block.size)
                        .append('\t').append(block.buildTime)
                        .append('\t').append(buildClass)
                        .append('\t').append(revision)
                        .append('\t').append(block.hasItems)
                        .append('\t').append(block.hasPower)
                        .append('\t').append(block.hasLiquids)
                        .append('\t').append(block.itemCapacity)
                        .append('\t').append(block.liquidCapacity);
                    for (var stack : block.requirements) {
                        line.append('\t').append(stack.item.id).append(':').append(stack.amount);
                    }
                    System.out.println(line);
                }
            }
        }
    }
}
