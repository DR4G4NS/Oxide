import arc.Core; import arc.Settings;
import mindustry.Vars; import mindustry.core.*; import mindustry.game.*; import mindustry.gen.*;
import mindustry.type.*; import mindustry.entities.*; import mindustry.entities.units.*; import mindustry.entities.bullet.*;
import java.lang.reflect.*;
public class CheckDelayedWeaponOrigin {
 static float lastX,lastY,lastAimX,lastAimY;
 public static class Capture extends BulletType {
  @Override public Bullet create(Entityc owner,Entityc shooter,Team team,float x,float y,float angle,float damage,float velocityScl,float lifetimeScl,Object data,Mover mover,float aimX,float aimY,Teamc target) {
   lastX=x;lastY=y;lastAimX=aimX;lastAimY=aimY;return null;
  }
 }
 public static class Exposed extends Weapon {public void execute(Unit u,WeaponMount m){bullet(u,m,0,0,0,null);}}
 public static void main(String[]args)throws Exception{
  Core.app=(arc.Application)Proxy.newProxyInstance(arc.Application.class.getClassLoader(),new Class[]{arc.Application.class},(proxy,m,a)->m.getReturnType()==boolean.class?m.getName().equals("isHeadless"):null);
  Vars.headless=true;Core.settings=new Settings();Vars.content=new ContentLoader();Vars.content.createBaseContent();Vars.content.init();Vars.world=new World();Vars.state=new GameState();Vars.net=new mindustry.net.Net(null);Groups.init();Vars.state.rules.unitCap=1000;Vars.indexer=new mindustry.ai.BlockIndexer();
  var type=Vars.content.unit(15);Unit u=type.create(Team.sharded);u.set(100,100);u.rotation=0;u.add();
  Weapon original=type.weapons.first();Exposed weapon=new Exposed();for(Field f:Weapon.class.getFields()) if(!Modifier.isFinal(f.getModifiers())&&!Modifier.isStatic(f.getModifiers())) f.set(weapon,f.get(original));
  weapon.shootSound=new arc.audio.Sound(){public int at(float x,float y,float pitch,float volume){return 0;}};weapon.bullet=new Capture();WeaponMount mount=new WeaponMount(weapon);mount.aimX=300;mount.aimY=100;
  weapon.execute(u,mount);float x=lastX,y=lastY;
  u.set(200,130);mount.aimX=400;mount.aimY=130;weapon.execute(u,mount);
  if(Math.abs(lastX-x-100)>0.001||Math.abs(lastY-y-30)>0.001||lastAimX!=400||lastAimY!=130)throw new AssertionError("stale delayed origin or aim");
  u.remove();lastX=-999;weapon.execute(u,mount);if(lastX!=-999)throw new AssertionError("removed shooter fired");
  System.out.println("OK Weapon.bullet re-evaluates live muzzle/aim and cancels removed shooter");
 }
}
