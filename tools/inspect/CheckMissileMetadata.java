import arc.Core;
import arc.Settings;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import java.nio.file.*;

/** Initialized target-JAR oracle for canonical missile movement metadata. */
public final class CheckMissileMetadata {
    public static void main(String[] args) throws Exception {
        Vars.headless=true;
        Core.settings=new Settings();
        Vars.content=new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        int rows=0;
        String[] names={"speed","rotateSpeed","homingDelay","missileAccelTime","lifetime","accel","drag"};
        for(String line:Files.readAllLines(Path.of(args[0]))) {
            if(line.isBlank() || line.startsWith("#")) continue;
            String[] values=line.split("\\s+");
            var unit=Vars.content.unit(Integer.parseInt(values[0]));
            for(int i=0;i<names.length;i++) {
                float actual=unit.getClass().getField(names[i]).getFloat(unit);
                float expected=Float.parseFloat(values[i+1]);
                if(Float.floatToIntBits(actual)!=Float.floatToIntBits(expected))
                    throw new AssertionError(unit.name+" "+names[i]+" "+actual+" != "+expected);
            }
            if(Float.floatToIntBits(unit.range)!=Float.floatToIntBits(Float.parseFloat(values[8])) || Float.floatToIntBits(unit.weapons.first().range())!=Float.floatToIntBits(Float.parseFloat(values[9]))) throw new AssertionError(unit.name+" range");
            if(unit.targetAir!=Boolean.parseBoolean(values[11])) throw new AssertionError(unit.name+" targetAir");
            if(unit.targetGround!=Boolean.parseBoolean(values[10])) throw new AssertionError(unit.name+" targetGround");
            rows++;
        }
        if(rows!=7) throw new AssertionError("rows="+rows);
        int priorities=0;
        for (String line : Files.readAllLines(Path.of(args[0]).resolveSibling("unit_target_priority.tsv"))) {
            if (line.isBlank() || line.startsWith("#")) continue;
            String[] fields=line.split("\\s+");
            var unit=Vars.content.unit(Integer.parseInt(fields[0]));
            if (unit.targetPriority!=Float.parseFloat(fields[1])) throw new AssertionError(unit.name+" targetPriority");
            priorities++;
        }
        if (priorities!=Vars.content.units().size) throw new AssertionError("priority rows="+priorities);
        System.out.println("OK initialized missile metadata rows="+rows);
    }
}
