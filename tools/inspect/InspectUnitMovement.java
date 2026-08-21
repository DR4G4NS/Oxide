import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.type.UnitType;

/** Emits exact unit physics fields for Serpulo units in desktop 158.1. */
public final class InspectUnitMovement {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("# ID hitSize physics allowLegStep legPhysicsLayer flying naval");
        for (UnitType unit : Vars.content.units()) {
            if (unit.id >= 35) break;
            System.out.printf("%d\t%f\t%b\t%b\t%b\t%b\t%b%n",
                unit.id, unit.hitSize, unit.physics, unit.allowLegStep,
                unit.legPhysicsLayer, unit.flying, unit.naval);
        }
    }
}
