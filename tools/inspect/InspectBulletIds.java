import mindustry.Vars;
import mindustry.content.Bullets;
import mindustry.core.ContentLoader;

/** Prints runtime IDs for shared bullet types used by Rust protocol fixtures. */
public final class InspectBulletIds {
    public static void main(String[] args) {
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("damageLightning=" + Bullets.damageLightning.id);
        System.out.println("damageLightningGround=" + Bullets.damageLightningGround.id);
        System.out.println("damageLightningAir=" + Bullets.damageLightningAir.id);
    }
}
