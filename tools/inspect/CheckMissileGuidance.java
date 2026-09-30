import arc.Core; import arc.Settings; import arc.util.Time;
import mindustry.Vars; import mindustry.core.*; import mindustry.game.*; import mindustry.gen.*; import mindustry.ai.types.*;
public class CheckMissileGuidance {
 public static void main(String[] args){
  Vars.headless=true; Core.settings=new Settings(); Vars.content=new ContentLoader(); Vars.content.createBaseContent(); Vars.content.init(); Vars.world=new World();Vars.state=new GameState();Vars.net=new mindustry.net.Net(null);Groups.init();
  Unit shooter=Vars.content.unit(0).create(Team.sharded);shooter.aimX=0;shooter.aimY=100;
  for(int id:new int[]{46,65}) for(float time:new float[]{9,10,25,50}) {
   Unit missile=Vars.content.unit(id).create(Team.sharded);missile.rotation=0;((TimedKillc)missile).time(time);MissileAI ai=(MissileAI)missile.controller();ai.shooter=shooter;Time.delta=1;ai.updateMovement();
   float expectedAngle=time<10?0:(id==46?2.5f:0.25f);
   float expectedSpeed=(id==46?3.35f:4.6f*(float)Math.pow(Math.min(time/50f,1f),2))*0.5f;
   if(Math.abs(missile.rotation-expectedAngle)>0.0001f || Math.abs(missile.vel.len()-expectedSpeed)>0.0001f) throw new AssertionError(id+" age="+time);

  }
  System.out.println("OK MissileAI owner aim delay, turn rate and acceleration");
 }
}
