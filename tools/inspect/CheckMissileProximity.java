import arc.Core;
import arc.Settings;
import arc.util.Time;
import mindustry.Vars;
import mindustry.core.*;
import mindustry.game.*;
import mindustry.gen.*;
import mindustry.ai.types.MissileAI;
import mindustry.ai.BlockIndexer;

/** Real indexed target acquisition; only the retarget clock is forced ready. */
public final class CheckMissileProximity {
    static final class ReadyMissileAI extends MissileAI {
        @Override public boolean retarget() { return true; }
    }
    public static void main(String[] args) {
        Vars.headless = true;
        Core.settings = new Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Vars.world = new World();
        Vars.world.resize(50, 50).fill();
        Vars.state = new GameState();
        Vars.net = new mindustry.net.Net(null);
        Vars.indexer = new BlockIndexer();
        Groups.init();
        Time.delta = 1;
        int cases = 0;
        for (int id : new int[]{46, 53, 55, 65, 66, 67, 68}) {
            for (boolean air : new boolean[]{false, true}) {
                for (float distance : new float[]{5, 10, 30}) {
                    Groups.unit.clear();
                    Team.crux.data().tree().clear();
                    Vars.state.teams.present.clear();
                    Vars.state.teams.present.add(Team.crux.data());
                    Unit missile = Vars.content.unit(id).create(Team.sharded);
                    missile.set(100, 100);
                    missile.rotation = 0;
                    missile.add();
                    Unit target = Vars.content.unit(air ? 15 : 0).create(Team.crux);
                    target.set(100 + distance, 100);
                    target.elevation = air ? 1 : 0;
                    target.add();
                    Team.crux.data().tree().insert(target);
                    ReadyMissileAI ai = new ReadyMissileAI();
                    ai.unit(missile);
                    ai.updateWeapons();
                    boolean expected = air ? id == 46 && distance <= 10 : distance == 5;
                    if (missile.mounts[0].shoot != expected) {
                        throw new AssertionError(id + " air=" + air + " distance=" + distance);
                    }
                    missile.remove();
                    target.remove();
                    cases++;
                }
            }
        }
        System.out.println("OK MissileAI proximity acquisition cases=" + cases);
        float[][] boundaries={{18,0,1},{18.99f,0,1},{19,0,0},{20,0,0},{18,18,1},{18.99f,18.99f,0},{18,19,0}};
        for (float[] boundary : boundaries) {
            Groups.unit.clear();
            Team.crux.data().tree().clear();
            Vars.state.teams.present.clear();
            Vars.state.teams.present.add(Team.crux.data());
            Unit missile=Vars.content.unit(46).create(Team.sharded);
            missile.set(100,100);
            missile.add();
            Unit target=Vars.content.unit(14).create(Team.crux);
            target.set(100+boundary[0],100+boundary[1]);
            target.add();
            Team.crux.data().tree().insert(target);
            ReadyMissileAI ai=new ReadyMissileAI();
            ai.unit(missile);
            ai.updateWeapons();
            if(missile.mounts[0].shoot!=(boundary[2]==1)) throw new AssertionError("large hitbox boundary "+boundary[0]+","+boundary[1]);
            missile.remove();
            target.remove();
        }
        System.out.println("OK MissileAI strict broadphase boundaries=7");
    }
}
