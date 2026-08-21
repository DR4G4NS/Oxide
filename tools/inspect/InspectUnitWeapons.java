import mindustry.Vars;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.entities.bullet.BulletType;
import mindustry.type.UnitType;

/** Prints runtime weapon/bullet parameters used to audit the Rust combat port. */
public final class InspectUnitWeapons {
    private static void print(UnitType unit) {
        System.out.printf(
            "%s id=%s class=%s health=%s speed=%s range=%s mounts=%s%n",
            unit.name, unit.id, unit.constructor.get().classId(), unit.health,
            unit.speed, unit.range, unit.weapons.size
        );
        unit.weapons.each(weapon -> {
            BulletType bullet = weapon.bullet;
            System.out.printf(
                "%s weapon=%s reload=%s shots=%s inaccuracy=%s velocityRnd=%s " +
                "bullet=%s speed=%s damage=%s life=%s splash=%s radius=%s " +
                "pierce=%s pierceBuilding=%s cap=%s homingRange=%s status=%s(%s) " +
                "statusDuration=%s frags=%s frag=%s%n",
                unit.name, weapon.name, weapon.reload, weapon.shoot.shots,
                weapon.inaccuracy, weapon.velocityRnd, bullet.id, bullet.speed,
                bullet.damage, bullet.lifetime, bullet.splashDamage,
                bullet.splashDamageRadius, bullet.pierce, bullet.pierceBuilding,
                bullet.pierceCap, bullet.homingRange, bullet.status.name, bullet.status.id,
                bullet.statusDuration, bullet.fragBullets,
                bullet.fragBullet == null ? -1 : bullet.fragBullet.id
            );
        });
    }

    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.units().each(InspectUnitWeapons::print);
    }
}
