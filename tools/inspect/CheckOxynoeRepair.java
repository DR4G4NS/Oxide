import arc.Core;
import arc.util.Time;
import mindustry.Vars;
import mindustry.ai.BlockIndexer;
import mindustry.ai.types.CommandAI;
import mindustry.content.*;
import mindustry.core.*;
import mindustry.game.Team;
import mindustry.gen.*;
import mindustry.net.Net;
import mindustry.ai.UnitStance;

/** External 160.5 oracle for Oxynoe plasma weapon controls and ownership. */
public final class CheckOxynoeRepair {
  public static void main(String[] args) {
    Vars.headless = true;
    Core.app = new arc.mock.MockApplication();
    Core.audio = new arc.mock.MockAudio();
    Core.settings = new arc.Settings();
    Vars.content = new ContentLoader();
    Vars.content.createBaseContent();
    Vars.content.init();
    Vars.state = new GameState();
    Vars.net = new Net(null);
    Groups.init();
    Vars.world = new World();
    Vars.world.resize(40,40).fill();
    Vars.indexer = new BlockIndexer();
    Time.delta=1f;Vars.state.rules.unitCap=1000;
    var u=UnitTypes.oxynoe.create(Team.get(5));u.set(80,80);u.rotation=0;u.add();
    var ai=new CommandAI();ai.unit(u);
    ai.setStance(UnitStance.holdFire);
    if(ai.shouldFire())throw new AssertionError("hold fire allowed");
    ai.disableStance(UnitStance.holdFire);
    if(!ai.shouldFire())throw new AssertionError("normal fire disabled");
    u.disarmed=true;
    var mount=u.mounts[0];mount.shoot=true;mount.rotate=true;mount.aimX=120;mount.aimY=80;mount.rotation=90;mount.targetRotation=90;mount.reload=0;
    mount.weapon.update(u,mount);
    if(mount.totalShots!=0)throw new AssertionError("disarmed fired");
    u.disarmed=false;mount.reload=0;mount.side=mount.weapon.flipSprite;
    for(int i=0;i<60 && mount.totalShots==0;i++) mount.weapon.update(u,mount);
    if(mount.totalShots!=1 || mount.bullet==null)throw new AssertionError("expected repair bullet");
    var b=mount.bullet;
    if(b.team.id!=5 || b.owner!=u)throw new AssertionError("team/owner changed");
    if(b.type.healPercent!=1.5f || !b.type.collidesTeam)throw new AssertionError("not repair plasma");
    System.out.println("OXYNOE_REPAIR_WEAPON_ORACLE_PASS hold_fire=true disarm=true owner=unit team=5 heal=1.5%");
  }
}
