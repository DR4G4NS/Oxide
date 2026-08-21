import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.type.StatusEffect;

/** Prints runtime StatusEffect parameters used to audit the Rust combat port. */
public final class InspectStatusEffects {
    private static void print(StatusEffect status) {
        System.out.printf(
            "%d\t%s\t%f\t%f\t%f\t%f\t%f\t%f\t%f\t%b\t%b\t%f\t%f\t%s%n",
            status.id, status.name, status.speedMultiplier,
            status.damageMultiplier, status.reloadMultiplier,
            status.healthMultiplier, status.damage,
            status.intervalDamage, status.intervalDamageTime,
            status.permanent, status.reactive, status.effectChance,
            status.transitionDamage, status.color
        );
    }

    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("# id\tname\tspeed\tdamageMult\treloadMult\thealthMult\tdamagePerTick\tintervalDamage\tintervalDamageTime\tpermanent\treactive\teffectChance\ttransitionDamage\tcolor");
        Vars.content.statusEffects().each(InspectStatusEffects::print);
    }
}
