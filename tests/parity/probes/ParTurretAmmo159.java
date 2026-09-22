import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.ctype.ContentType;
import mindustry.world.Block;
import mindustry.world.blocks.defense.turrets.ItemTurret;
import mindustry.content.Blocks;

public class ParTurretAmmo159 {
    public static void main(String[] args) {
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("{");
        System.out.println("\"turrets\": [");
        String[] names = {"breach", "diffuse", "titan", "disperse", "scathe", "smite"};
        boolean firstTurret = true;
        for (String n : names) {
            Block b = (Block) Vars.content.getByName(ContentType.block, n);
            if (!(b instanceof ItemTurret t)) {
                System.out.println((firstTurret ? "" : ",") + "{\"name\": \"" + n + "\", \"missing\": true}");
                firstTurret = false;
                continue;
            }
            StringBuilder sb = new StringBuilder();
            sb.append(firstTurret ? "" : ",\n");
            firstTurret = false;
            sb.append("{\"name\": \"").append(n).append("\",\n");
            sb.append(" \"reload\": ").append(t.reload).append(",\n");
            sb.append(" \"range\": ").append(t.range).append(",\n");
            sb.append(" \"ammoPerShot\": ").append(t.ammoPerShot).append(",\n");
            sb.append(" \"maxAmmo\": ").append(t.maxAmmo).append(",\n");
            sb.append(" \"targetAir\": ").append(t.targetAir).append(",\n");
            sb.append(" \"targetGround\": ").append(t.targetGround).append(",\n");
            sb.append(" \"consumeAmmoOnce\": ").append(t.consumeAmmoOnce).append(",\n");
            sb.append(" \"shootClass\": \"").append(t.shoot.getClass().getSimpleName()).append("\",\n");
            int shots = 1;
            try {
                var f = t.shoot.getClass().getField("shots");
                shots = f.getInt(t.shoot);
            } catch (ReflectiveOperationException ignored) {
            }
            sb.append(" \"shootShots\": ").append(shots).append(",\n");
            sb.append(" \"shots\": [");
            boolean first = true;
            for (var e : t.ammoTypes.entries()) {
                var item = e.key;
                var bullet = e.value;
                if (!first) sb.append(", ");
                first = false;
                sb.append("{\"item\": \"").append(item.name).append("\"");
                sb.append(", \"bulletId\": ").append(bullet.id);
                sb.append(", \"damage\": ").append(bullet.damage);
                sb.append(", \"speed\": ").append(bullet.speed);
                sb.append(", \"splashDamage\": ").append(bullet.splashDamage);
                sb.append(", \"splashRadius\": ").append(bullet.splashDamageRadius);
                sb.append(", \"ammoMultiplier\": ").append(bullet.ammoMultiplier);
                sb.append(", \"reloadMultiplier\": ").append(bullet.reloadMultiplier);
                float rangeChange = 0f;
                try {
                    var f = bullet.getClass().getField("rangeChange");
                    rangeChange = f.getFloat(bullet);
                } catch (ReflectiveOperationException ignored) {
                }
                sb.append(", \"rangeChange\": ").append(rangeChange);
                sb.append("}");
            }
            sb.append("]}");
            System.out.println(sb);
        }
        System.out.println("]");
        System.out.println("}");
    }
}
