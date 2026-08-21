import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.type.StatusEffect;
import mindustry.world.Block;
import mindustry.world.blocks.environment.Floor;

/** Emits floor status fields for desktop 158.1. */
public final class InspectFloorStatus {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        System.out.println("# block_id\tname\tstatus_id\tstatus_duration");
        for (Block block : Vars.content.blocks()) {
            if (!(block instanceof Floor floor)) {
                continue;
            }
            StatusEffect status = floor.status;
            if (status == null || status.id <= 0) {
                continue;
            }
            System.out.printf(
                "%d\t%s\t%d\t%f%n",
                block.id,
                block.name,
                status.id,
                floor.statusDuration
            );
        }
    }
}
