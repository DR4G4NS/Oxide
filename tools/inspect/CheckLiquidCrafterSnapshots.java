import java.io.*;
import java.nio.file.*;
import arc.Core;
import arc.util.Time;
import arc.util.io.Reads;
import mindustry.Vars;
import mindustry.ai.BlockIndexer;
import mindustry.content.*;
import mindustry.core.*;
import mindustry.game.Team;
import mindustry.gen.*;
import mindustry.net.Net;
import mindustry.world.blocks.production.GenericCrafter;
import mindustry.world.blocks.production.GenericCrafter.GenericCrafterBuild;

/** External 160.5 oracle: complete liquid crafter snapshots and 480 client ticks. */
public final class CheckLiquidCrafterSnapshots {
    static void near(float actual, float expected, String label) {
        if(Math.abs(actual - expected) > 0.0001f) throw new AssertionError(label + " actual=" + actual + " expected=" + expected);
    }
    static GenericCrafterBuild fresh(int x, int y, mindustry.world.Block block) {
        Vars.world.tile(x,y).setBlock(block, Team.sharded, 0);
        return (GenericCrafterBuild)Vars.world.build(x,y);
    }
    static void checkFixture(GenericCrafterBuild build, Path path, boolean sync) throws Exception {
        var in = new DataInputStream(new ByteArrayInputStream(Files.readAllBytes(path)));
        if(sync) build.readSync(Reads.get(in),build.version()); else {
            int length=in.readInt();
            if(length!=in.available())throw new AssertionError("save chunk length");
            byte revision=in.readByte();
            build.readAll(Reads.get(in),revision);
        }
        if(in.available()!=0) throw new AssertionError(path+" unread bytes="+in.available());
        if(build.block == Blocks.electrolyzer) {
            near(build.liquids.get(Liquids.water),12f,"fixture water");
            near(build.liquids.get(Liquids.ozone),8f,"fixture ozone");
            near(build.liquids.get(Liquids.hydrogen),9f,"fixture hydrogen");
        } else near(build.liquids.get(Liquids.nitrogen),15f,"fixture nitrogen");
        System.out.println("COMPLETE_FIXTURE_READ_PASS "+path.getFileName());
    }
    static void readSnapshot(GenericCrafterBuild build, Path path) throws Exception {
        var in=new DataInputStream(new ByteArrayInputStream(Files.readAllBytes(path)));
        build.readSync(Reads.get(in),build.version());
        if(in.available()!=0)throw new AssertionError("snapshot unread bytes "+path);
    }
    static void sameLiquids(GenericCrafterBuild actual, GenericCrafterBuild expected, String stage) {
        for(var liquid:new mindustry.type.Liquid[]{Liquids.water,Liquids.ozone,Liquids.hydrogen})
            near(actual.liquids.get(liquid),expected.liquids.get(liquid),stage+" "+liquid.name);
        near(actual.progress,expected.progress,stage+" progress");
    }
    static void snapshotContinuity(Path dir) throws Exception {
        var client=fresh(20,40,Blocks.electrolyzer);
        client.power.status=1f;
        for(int tick=0;tick<480;tick++) {
            if(tick==360) {
                var expected=fresh(30,40,Blocks.electrolyzer);
                readSnapshot(expected,dir.resolve("electrolyzer-360-sync.bin"));
                sameLiquids(client,expected,"before360 snapshot");
                readSnapshot(client,dir.resolve("electrolyzer-360-sync.bin"));
                sameLiquids(client,expected,"after360 snapshot");
            }
            client.liquids.set(Liquids.water,50f);
            client.power.status=1f;
            client.updateConsumption();client.updateTile();
        }
        var expected=fresh(30,40,Blocks.electrolyzer);
        readSnapshot(expected,dir.resolve("electrolyzer-480-sync.bin"));
        sameLiquids(client,expected,"before480 snapshot");
        readSnapshot(client,dir.resolve("electrolyzer-480-sync.bin"));
        sameLiquids(client,expected,"after480 snapshot");
        System.out.println("CLIENT_480_TICKS_SNAPSHOT_CONTINUITY_PASS water="+client.liquids.get(Liquids.water)+" ozone="+client.liquids.get(Liquids.ozone)+" hydrogen="+client.liquids.get(Liquids.hydrogen));
    }
    public static void main(String[] args) throws Exception {
        if(args.length != 1) throw new IllegalArgumentException("fixture directory required");
        Vars.headless=true;
        Core.settings=new arc.Settings();
        Vars.content=new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Vars.state=new GameState(); Vars.net=new Net(null); Groups.init();
        Vars.world=new World(); Vars.world.resize(60,60).fill();
        Vars.indexer=new BlockIndexer();
        Time.delta=1f;
        GenericCrafter electrolyzer=(GenericCrafter)Blocks.electrolyzer;
        if(electrolyzer.liquidOutputDirections[0]!=1 || electrolyzer.liquidOutputDirections[1]!=3) throw new AssertionError("directions");
        near(electrolyzer.liquidCapacity,50f,"electrolyzer capacity");
        near(electrolyzer.outputLiquids[0].amount,4f/60f,"ozone rate");
        near(electrolyzer.outputLiquids[1].amount,6f/60f,"hydrogen rate");
        var e=fresh(10,10,Blocks.electrolyzer);
        e.liquids.add(Liquids.water,12f); e.efficiency=1f;
        e.updateTile();
        near(e.liquids.get(Liquids.ozone),4f/60f,"stored ozone");
        near(e.liquids.get(Liquids.hydrogen),6f/60f,"stored hydrogen");
        System.out.println("OUTPUT_RATES_AND_STORAGE_PASS ozone="+e.liquids.get(Liquids.ozone)+" hydrogen="+e.liquids.get(Liquids.hydrogen));
        e.liquids.clear();e.liquids.add(Liquids.water,12f);e.liquids.add(Liquids.ozone,50f);e.liquids.add(Liquids.hydrogen,50f);
        if(e.shouldConsume()) throw new AssertionError("all-full consumer running");
        e.liquids.remove(Liquids.hydrogen,1f);
        if(!e.shouldConsume()) throw new AssertionError("free coproduct stopped");
        System.out.println("BACKPRESSURE_PASS all_full_stops=true single_full_continues=true");
        e.liquids.clear();e.liquids.add(Liquids.water,12f);e.liquids.add(Liquids.ozone,49.99f);e.liquids.add(Liquids.hydrogen,49.99f);e.power.status=1f;
        e.updateConsumption();
        float waterUsed=12f-e.liquids.get(Liquids.water);
        float inc=e.getProgressIncrease(1f);
        e.updateTile();
        near(waterUsed,10f/60f,"near-full consumer uses full edelta");
        System.out.println("NEAR_FULL_CONSUMPTION_PASS water_used="+waterUsed+" output_inc="+inc+" output_scaled_water="+(inc*10f/60f));
        // Actual direction-specific dump: north receives ozone, south hydrogen.
        e.liquids.clear();e.liquids.add(Liquids.ozone,10f);e.liquids.add(Liquids.hydrogen,10f);e.efficiency=0f;
        Vars.world.tile(10,12).setBlock(Blocks.liquidRouter,Team.sharded,0);
        Vars.world.tile(10,8).setBlock(Blocks.liquidRouter,Team.sharded,0);
        Vars.world.tile(12,10).setBlock(Blocks.liquidRouter,Team.sharded,0);
        e.updateProximity(); e.dumpOutputs();
        var north=Vars.world.build(10,12);var south=Vars.world.build(10,8);var east=Vars.world.build(12,10);
        near(north.liquids.get(Liquids.ozone),5f,"north pressure split");
        near(south.liquids.get(Liquids.hydrogen),5f,"south pressure split");
        near(north.liquids.get(Liquids.hydrogen),0f,"north no hydrogen");
        near(south.liquids.get(Liquids.ozone),0f,"south no ozone");
        near(east.liquids.get(Liquids.ozone),0f,"east no ozone");
        near(east.liquids.get(Liquids.hydrogen),0f,"east no hydrogen");
        System.out.println("DIRECTION_PRESSURE_DUMP_PASS north_ozone=5 south_hydrogen=5 east=0");
        var concentrator=fresh(40,10,Blocks.atmosphericConcentrator);
        Vars.world.tile(37,10).setBlock(Blocks.electricHeater,Team.sharded,0);
        var heater=Vars.world.build(37,10);
        heater.getClass().getField("heat").setFloat(heater,12f);
        concentrator.updateProximity();
        concentrator.updateTile();
        concentrator.liquids.clear();concentrator.power.status=1f;
        concentrator.updateConsumption();concentrator.updateTile();
        near(concentrator.efficiency,0.5f,"half-heat efficiency");
        near(concentrator.liquids.get(Liquids.nitrogen),8f/60f,"half-heat nitrogen");
        System.out.println("HEAT_GEOMETRY_PARTIAL_RATE_PASS heat=12 efficiency=.5 nitrogen="+concentrator.liquids.get(Liquids.nitrogen));
        if(args.length>0){
            Path dir=Path.of(args[0]);
            snapshotContinuity(dir);
            for(String name:new String[]{"electrolyzer","concentrator"}){
                var b=fresh(30,name.equals("electrolyzer")?30:40,name.equals("electrolyzer")?Blocks.electrolyzer:Blocks.atmosphericConcentrator);
                for(String kind:new String[]{"sync","save"})checkFixture(b,dir.resolve(name+"-"+kind+".bin"),kind.equals("sync"));
            }
        }
        System.out.println("LIQUID_CRAFTER_ORACLE_PASS");
    }
}
