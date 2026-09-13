import arc.Core;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.type.ItemStack;
import mindustry.world.blocks.units.Reconstructor;
import mindustry.world.blocks.units.UnitFactory;
import mindustry.world.consumers.ConsumeItems;
import mindustry.world.consumers.ConsumeLiquid;

/** Exports initialized 159.7 production recipes for src/game/unit_production.tsv. */
public final class InspectUnitProduction {
    public static void main(String[] args) {
        Vars.headless = true;
        Core.settings = new arc.Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        System.out.println("# block\tplan\tunit\tbuild_time\tliquid\tliquid_rate\titems (id:amount)");
        for (var block : Vars.content.blocks()) {
            if (block instanceof UnitFactory factory) {
                for (int i = 0; i < factory.plans.size; i++) {
                    var plan = factory.plans.get(i);
                    row(block.id, i, plan.unit.id, plan.time, -1, 0f, plan.requirements);
                }
            } else if (block instanceof Reconstructor reconstructor) {
                ItemStack[] items = {};
                int liquid = -1;
                float rate = 0f;
                for (var consumer : block.consumers) {
                    if (consumer instanceof ConsumeItems input) items = input.items;
                    if (consumer instanceof ConsumeLiquid input) {
                        liquid = input.liquid.id;
                        rate = input.amount;
                    }
                }
                row(block.id, -1, -1, reconstructor.constructTime, liquid, rate, items);
            }
        }
    }

    private static void row(int block, int plan, int unit, float time, int liquid,
                            float rate, ItemStack[] items) {
        var line = new StringBuilder(block + "\t" + plan + "\t" + unit + "\t" + time
            + "\t" + liquid + "\t" + rate);
        for (var item : items) line.append('\t').append(item.item.id).append(':').append(item.amount);
        System.out.println(line);
    }
}
