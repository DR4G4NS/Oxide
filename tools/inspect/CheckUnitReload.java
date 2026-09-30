import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.type.UnitType;
import mindustry.type.Weapon;

/** Check the external JAR's pre/post-init reload contract, without game assets. */
public final class CheckUnitReload {
    public static void main(String[] args) {
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        int checked = 0;
        for (int id : new int[]{0, 3, 5, 11, 18, 21}) {
            UnitType unit = Vars.content.unit(id);
            float[] reload = new float[unit.weapons.size];
            boolean[] mirror = new boolean[reload.length];
            for (int i = 0; i < reload.length; i++) {
                reload[i] = unit.weapons.get(i).reload;
                mirror[i] = unit.weapons.get(i).mirror;
            }
            unit.init();
            int mount = 0;
            for (int i = 0; i < reload.length; i++) {
                float expected = reload[i] * (mirror[i] ? 2f : 1f);
                for (int side = 0; side < (mirror[i] ? 2 : 1); side++) {
                    Weapon weapon = unit.weapons.get(mount++);
                    if (weapon.reload != expected) {
                        throw new AssertionError(unit.name + " reload " + weapon.reload + " != " + expected);
                    }
                    checked++;
                }
            }
            if (mount != unit.weapons.size) throw new AssertionError(unit.name + " mount count");
        }
        System.out.println("OK initialized unit reloads mounts=" + checked);
    }
}
