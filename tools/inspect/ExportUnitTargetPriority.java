import arc.Core;
import arc.Settings;
import mindustry.Vars;
import mindustry.core.ContentLoader;
public final class ExportUnitTargetPriority {
    public static void main(String[] args) {
        arc.util.Log.logger = (level, text) -> {};
        Vars.headless = true;
        Core.settings = new Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        System.out.println("# unit_id target_priority (initialized160.5 JAR)");
        for (var unit : Vars.content.units()) System.out.println(unit.id + "\t" + unit.targetPriority);
    }
}
