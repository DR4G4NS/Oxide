import mindustry.Vars;
import mindustry.core.*;
import mindustry.type.*;
import mindustry.gen.*;
import mindustry.game.*;
import mindustry.entities.units.*;
import arc.util.Time;
import java.lang.reflect.*;
public class TraceNavanax {
 static java.util.List<String> trace=new java.util.ArrayList<>();
 static int tick;
 public static class TraceWeapon extends Weapon {
  int index;
  protected void shoot(Unit u, WeaponMount m,float x,float y,float a){trace.add(tick+":"+index);}
 }
 public static void main(String[] args) throws Exception {
  Vars.headless=true; Vars.content=new ContentLoader(); Vars.content.createBaseContent();
  Vars.state=new GameState(); Vars.world=new World(); Vars.net=new mindustry.net.Net(null); Groups.init();
  UnitType t=Vars.content.unit(34); t.init();
  for(int i=0;i<2;i++) { Weapon old=t.weapons.get(i); TraceWeapon w=new TraceWeapon();
   for(Field f:Weapon.class.getFields()) if(!Modifier.isFinal(f.getModifiers())&&!Modifier.isStatic(f.getModifiers())) f.set(w,f.get(old));
   w.index=i; t.weapons.set(i,w); }
  Unit u=t.create(Team.sharded); u.rotation=90f;
  for(tick=1;tick<=400;tick++) {Time.delta=1f; for(int i=0;i<2;i++) {
   WeaponMount m=u.mounts[i]; m.shoot=true; m.rotate=true;m.aimX=u.x; m.aimY=u.y+200f; m.rotation=0f; m.warmup=1f;
   m.weapon.update(u,m);
  }}
  String actual=String.join(",",trace);
  if(!actual.equals("1:0,66:1,132:0,197:1,263:0,328:1,394:0")) throw new AssertionError(actual);
  System.out.println("OK TraceNavanax "+actual);
 }
}
