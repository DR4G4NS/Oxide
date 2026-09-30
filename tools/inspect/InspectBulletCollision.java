import java.util.Locale;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.entities.bullet.BulletType;

/** Export collision contracts from the externally supplied target JAR. */
public final class InspectBulletCollision {
    public static void main(String[] args) {
        Locale.setDefault(Locale.ROOT);
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        for (BulletType bullet : Vars.content.bullets()) bullet.init();
        System.out.println("# bullet_id\tcollides\tcollides_air\tcollides_ground\tcollides_tiles\tcollides_team\thit_size\tscale_life\tdespawn_hit");
        for (BulletType bullet : Vars.content.bullets()) {
            System.out.printf("%d\t%b\t%b\t%b\t%b\t%b\t%s\t%b\t%b%n",
                bullet.id, bullet.collides, bullet.collidesAir, bullet.collidesGround,
                bullet.collidesTiles, bullet.collidesTeam, bullet.hitSize, bullet.scaleLife, bullet.despawnHit);
        }
    }
}
