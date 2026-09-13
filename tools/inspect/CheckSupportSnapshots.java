import java.io.*;
import java.nio.file.*;
import arc.Core;
import arc.graphics.Texture;
import arc.graphics.g2d.*;
import arc.util.Time;
import arc.util.io.Reads;
import mindustry.Vars;
import mindustry.ai.BlockIndexer;
import mindustry.content.*;
import mindustry.core.*;
import mindustry.game.Team;
import mindustry.gen.*;
import mindustry.net.Net;
import mindustry.world.blocks.distribution.StackConveyor;
import mindustry.world.blocks.distribution.StackConveyor.StackConveyorBuild;

/** External 159.7 oracle: complete snapshots and actual belt draw calls. */
public final class CheckSupportSnapshots {
    static final class RecordingBatch extends Batch {
        TextureRegion item;
        int itemDraws;
        float itemX, itemY;
        protected void draw(Texture texture, float[] vertices, int offset, int count) {}
        protected void draw(TextureRegion region, float x, float y, float originX, float originY,
                            float width, float height, float rotation) {
            if (region == item) { itemDraws++; itemX = x + originX; itemY = y + originY; }
        }
        protected void flush() {}
    }

    public static void main(String[] args) throws Exception {
        Vars.headless = true;
        Core.settings = new arc.Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Vars.state = new GameState();
        Vars.net = new Net(null);
        Groups.init();
        Vars.world = new World();
        Vars.world.resize(40, 40).fill();
        Vars.indexer = new BlockIndexer();
        Vars.controlPath = new mindustry.ai.ControlPathfinder();
        Vars.pathfinder = new mindustry.ai.Pathfinder();
        Path dir = Path.of(args[0]);
        for (int id : new int[]{21, 22}) {
            var type = Vars.content.unit(id);
            Unit unit = type.create(Team.sharded);
            for (String state : new String[]{"idle", "repair", "full", "hold", "enemy"}) {
                var input = input(dir.resolve("support-" + id + "-" + state + ".bin"));
                unit.readSync(Reads.get(input));
                if (input.available() != 0) throw new AssertionError("trailing unit bytes");
                boolean shooting = state.equals("repair") || state.equals("enemy");
                if (unit.isShooting != shooting) throw new AssertionError(type + " " + state + " trigger");
                for (var mount : unit.mounts) {
                    if (mount.shoot != shooting || mount.rotate != shooting) {
                        throw new AssertionError(type + " " + state + " mount shoot/rotate");
                    }
                    if (state.equals("repair") && (mount.aimX != 160f || mount.aimY != 64f)) {
                        throw new AssertionError("repair mount aims away from damaged building");
                    }
                }
            }
        }
        if (UnitTypes.mega.weapons.size != 4
                || UnitTypes.mega.weapons.get(0).reload != 48f
                || UnitTypes.mega.weapons.get(2).reload != 30f
                || UnitTypes.mega.weapons.get(0).bullet.healPercent != 5.5f
                || UnitTypes.mega.weapons.get(2).bullet.healPercent != 3f) {
            throw new AssertionError("Mega weapon contract changed");
        }
        RecordingBatch batch = new RecordingBatch();
        Core.batch = batch;
        batch.item = Items.titanium.fullIcon = new TextureRegion();
        StackConveyor conveyor = (StackConveyor)Blocks.plastaniumConveyor;
        conveyor.stackRegion = new TextureRegion();
        conveyor.glowRegion = new TextureRegion() { public boolean found() { return false; } };
        Time.delta = 1f;
        for (int block : new int[]{327, 328}) {
            int headY = block == 327 ? 9 : 10;
            Vars.world.tile(10, 7).setBlock(Vars.content.block(block), Team.sharded, 0);
            for (int y = headY; y <= headY + 10; y++) {
                Vars.world.tile(10, y).setBlock(Blocks.plastaniumConveyor, Team.sharded, 1);
            }
            var head = (StackConveyorBuild)Vars.world.build(10, headY);
            if (!head.acceptItem(Vars.world.build(10, 7), Items.titanium)) {
                throw new AssertionError("JAR rejects multiblock drill at loading dock");
            }
            var tip = (StackConveyorBuild)Vars.world.build(10, headY + 10);
            for (int pass = 0; pass < 2; pass++) {
                var input = input(dir.resolve("plastanium-" + block + ".bin"));
                tip.readSync(Reads.get(input), tip.version());
                if (input.available() != 0 || tip.items.get(Items.titanium) != 10) {
                    throw new AssertionError("complete belt snapshot lost items");
                }
                for (int tick = 0; tick < 400; tick++) {
                    tip.updateTile();
                    batch.itemDraws = 0;
                    tip.draw();
                    if (batch.itemDraws != 1 || batch.itemX != tip.x || batch.itemY != tip.y) {
                        throw new AssertionError("inspection hid/displaced stack: " + batch.itemDraws);
                    }
                }
            }
        }
        System.out.println("OK support snapshots idle/repair/full/hold/enemy, rotating mounts, Mega weapon values");
        System.out.println("OK plastanium drill input and actual draw survive inspection across 800 client ticks");
    }

    static DataInputStream input(Path file) throws IOException {
        return new DataInputStream(new ByteArrayInputStream(Files.readAllBytes(file)));
    }
}
