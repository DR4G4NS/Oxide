import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import arc.Core;
import arc.util.io.Reads;
import mindustry.Vars;
import mindustry.ai.BlockIndexer;
import mindustry.content.Blocks;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.game.Team;
import mindustry.gen.Groups;
import mindustry.net.Net;
import mindustry.world.blocks.units.Reconstructor.ReconstructorBuild;

/** Read complete Rust-produced snapshots using the external target client. */
public final class CheckReconstructorSnapshots {
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
        Vars.world.tile(10, 10).setBlock(Blocks.additiveReconstructor, Team.sharded, 0);
        ReconstructorBuild build = (ReconstructorBuild)Vars.world.tile(10, 10).build;
        Path directory = Path.of(args[0]);
        read(build, directory.resolve("reconstructor-running.bin"));
        if (!build.enabled || build.efficiency != 1f || build.payload == null
                || build.payload.unit.type != UnitTypes.dagger
                || build.progress <= 390f || build.progress >= 420f) {
            throw new AssertionError("running snapshot lost construction state");
        }
        float progress = build.progress;
        float health = build.payload.unit.health;
        read(build, directory.resolve("reconstructor-paused.bin"));
        if (build.enabled || build.efficiency != 0f || build.progress != progress
                || build.payload == null || build.payload.unit.type != UnitTypes.dagger
                || build.payload.unit.health != health) {
            // Payload.read allocates a new local entity ID; saved type/health
            // and construction progress, not that ID, must survive readSync.
            throw new AssertionError("paused snapshot reset payload or progress");
        }
        read(build, directory.resolve("reconstructor-completed.bin"));
        if (!build.enabled || build.efficiency != 0f || build.progress != 0f
                || build.payload == null || build.payload.unit.type != UnitTypes.mace) {
            throw new AssertionError("completed snapshot lost upgraded unit");
        }
        Vars.world.tile(5, 5).setBlock(Blocks.groundFactory, Team.sharded, 0);
        var factory = (mindustry.world.blocks.units.UnitFactory.UnitFactoryBuild)Vars.world.tile(5, 5).build;
        read(factory, directory.resolve("factory-deselected.bin"));
        if (factory.currentPlan != -1 || factory.progress != 0f || factory.payload != null
                || factory.efficiency != 0f) {
            throw new AssertionError("factory snapshot reselected or restarted production");
        }
        System.out.println("OK reconstructor complete snapshots running/paused/completed");
        System.out.println("OK factory deselection survives complete snapshot read");
    }

    private static void read(mindustry.gen.Building build, Path path) throws Exception {
        byte[] bytes = Files.readAllBytes(path);
        DataInputStream input = new DataInputStream(new ByteArrayInputStream(bytes));
        build.readSync(Reads.get(input), build.version());
        if (input.available() != 0) throw new AssertionError(path + " trailing bytes=" + input.available());
    }
}
