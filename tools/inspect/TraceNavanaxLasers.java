import mindustry.Vars;
import mindustry.core.*;
import mindustry.type.*;
import mindustry.gen.*;
import mindustry.game.*;
import mindustry.entities.units.*;
import arc.util.Time;
import java.lang.reflect.*;
public class TraceNavanaxLasers {
 static java.util.List<String> trace=new java.util.ArrayList<>();
 static int tick; static Unit target;
 public static class TraceWeapon extends Weapon {
  int index;
 protected Teamc findTarget(Unit u,float x,float y,float r,boolean a,boolean g){return target;}
 protected boolean checkTarget(Unit u,Teamc t,float x,float y,float r){return false;}
  protected void shoot(Unit u, WeaponMount m,float x,float y,float a){trace.add(tick+":"+index); Bullet b=Bullet.create(); b.type=bullet; b.lifetime=bullet.lifetime; b.team=u.team; b.add(); m.bullet=b;}
 }
 public static void main(String[] args) throws Exception {
  Vars.headless=true; Vars.content=new ContentLoader(); Vars.content.createBaseContent();
  Vars.state=new GameState(); Vars.world=new World(); Vars.net=new mindustry.net.Net(null); Groups.init();
  for(var b:Vars.content.bullets()) b.init(); UnitType t=Vars.content.unit(34); t.init();
  for(int i=2;i<6;i++) { Weapon old=t.weapons.get(i); TraceWeapon w=new TraceWeapon();
   for(Field f:Weapon.class.getFields()) if(!Modifier.isFinal(f.getModifiers())&&!Modifier.isStatic(f.getModifiers())) f.set(w,f.get(old));
   w.index=i; t.weapons.set(i,w); }
  Unit u=t.create(Team.sharded); u.rotation=90f; target=t.create(Team.crux);target.x=0;target.y=50;
  for(tick=1;tick<=700;tick++) {Time.delta=1f; for(int i=2;i<6;i++) {
   WeaponMount m=u.mounts[i]; m.shoot=true; m.rotate=true;m.aimX=u.x; m.aimY=u.y+200f; m.warmup=1f;
   m.weapon.update(u,m);
   if(m.bullet!=null) m.bullet.time+=1f;
  }}
  String actual=String.join(",",trace);
  if(!actual.equals("3:2,3:3,7:4,7:5,327:2,327:3,331:4,331:5,651:2,651:3,655:4,655:5")) throw new AssertionError(actual);
  System.out.println("OK TraceNavanaxLasers "+actual);
 }
}
