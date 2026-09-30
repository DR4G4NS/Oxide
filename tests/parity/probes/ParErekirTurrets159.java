import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.ctype.ContentType;
import mindustry.world.Block;
import mindustry.world.blocks.defense.turrets.*;
import mindustry.content.Blocks;
import mindustry.world.consumers.ConsumeLiquid;

public class ParErekirTurrets159 {
    public static void main(String[] args) throws Exception {
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("{");
        System.out.println("\"turrets\": [");
        String[] names = {"afflict", "lustre", "sublimate", "malign", "smite"};
        boolean firstTurret = true;
        for (String n : names) {
            Block b = (Block) Vars.content.getByName(ContentType.block, n);
            StringBuilder sb = new StringBuilder();
            sb.append(firstTurret ? "" : ",\n");
            firstTurret = false;
            sb.append("{\"name\": \"").append(n).append("\"");
            if (b instanceof PowerTurret pt) {
                var bt = pt.shootType;
                sb.append(", \"kind\": \"power\"");
                sb.append(", \"bulletId\": ").append(bt.id);
                sb.append(", \"damage\": ").append(bt.damage);
                sb.append(", \"speed\": ").append(bt.speed);
                sb.append(", \"splashDamage\": ").append(bt.splashDamage);
                sb.append(", \"splashRadius\": ").append(bt.splashDamageRadius);
                sb.append(", \"reload\": ").append(pt.reload);
                sb.append(", \"range\": ").append(pt.range);
                float heat = -1f;
                for (var c : pt.consumers) {
                    if (c instanceof mindustry.world.consumers.ConsumeItemFilter f) {
                        // heat requirement via item filter is not numeric here
                    }
                }
                try {
                    var f = pt.getClass().getField("heatRequirement");
                    heat = f.getFloat(pt);
                } catch (ReflectiveOperationException ignored) {
                    // HeatConsumer via consumers
                }
                sb.append(", \"heatReq\": ").append(heat);
            } else if (b instanceof ContinuousLiquidTurret clt) {
                sb.append(", \"kind\": \"continuous-liquid\", \"liquids\": [");
                boolean first = true;
                var field = ContinuousLiquidTurret.class.getDeclaredField("ammoTypes");
                field.setAccessible(true);
                var ammoTypes = (arc.struct.ObjectMap<?, ?>) field.get(clt);
                for (var e : ammoTypes.entries()) {
                    var liquid = (mindustry.type.Liquid) e.key;
                    var bt = (mindustry.entities.bullet.BulletType) e.value;
                    if (!first) sb.append(", ");
                    first = false;
                    sb.append("{\"liquid\": \"").append(liquid.name).append("\"");
                    sb.append(", \"bulletId\": ").append(bt.id);
                    sb.append(", \"damage\": ").append(bt.damage);
                    sb.append(", \"continuous\": true}");
                }
                sb.append("]");
                sb.append(", \"range\": ").append(clt.range);
                sb.append(", \"reload\": ").append(clt.reload);
            } else if (b instanceof ContinuousTurret ct) {
                sb.append(", \"kind\": \"continuous\"");
                var f = ContinuousTurret.class.getDeclaredField("shootType");
                f.setAccessible(true);
                var bt = (mindustry.entities.bullet.BulletType) f.get(ct);
                sb.append(", \"bulletId\": ").append(bt.id);
                sb.append(", \"damage\": ").append(bt.damage);
                sb.append(", \"range\": ").append(ct.range);
                sb.append(", \"reload\": ").append(ct.reload);
            } else {
                sb.append(", \"kind\": \"unknown\"");
            }
            sb.append("}");
            System.out.println(sb);
        }
        System.out.println("]");
        System.out.println("}");
    }
}
