import mindustry.Vars;
import mindustry.core.ContentLoader;

/** Prints official total build requirements and build time for Serpulo unit IDs 0..34. */
public final class InspectUnitRequirements {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        for (int id = 0; id < 35; id++) {
            var unit = Vars.content.unit(id);
            var line = new StringBuilder();
            line.append(id).append('\t').append(unit.getBuildTime());
            for (var stack : unit.getTotalRequirements()) {
                line.append('\t').append(stack.item.id).append(':').append(stack.amount);
            }
            System.out.println(line);
        }
    }
}
