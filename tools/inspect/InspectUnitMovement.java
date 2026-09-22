import arc.Core;
import arc.Settings;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.type.UnitType;

/** Emits initialized unit physics fields for Serpulo units in the target JAR. */
public final class InspectUnitMovement {
    public static void main(String[] args) {
        arc.util.Log.logger = (level, text) -> {};
        Vars.headless = true;
        Core.settings = new Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        System.out.println("# ID hitSize physics allowLegStep legPhysicsLayer flying naval");
        for (UnitType unit : Vars.content.units()) {
            if (unit.id >= 35) break;
            System.out.printf("%d\t%f\t%b\t%b\t%b\t%b\t%b%n",
                unit.id, unit.hitSize, unit.physics, unit.allowLegStep,
                unit.legPhysicsLayer, unit.flying, unit.naval);
        }
    }
}
