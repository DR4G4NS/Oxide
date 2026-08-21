import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.type.UnitType;

/** Emits unit hovering flag for desktop 158.1. */
public final class InspectUnitHovering {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("# unit_id\tname\thovering");
        for (UnitType unit : Vars.content.units()) {
            if (unit.hovering) {
                System.out.printf("%d\t%s\ttrue%n", unit.id, unit.name);
            }
        }
    }
}
