import java.io.InputStream;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.util.Time;
import mindustry.Vars;
import mindustry.content.Blocks;
import mindustry.content.StatusEffects;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.gen.Groups;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.type.StatusEffect;
import mindustry.world.Tiles;
import mindustry.world.blocks.environment.Floor;

/**
 * P0-E1 differential probe: floor status reapplication in StatusComp.update
 * on desktop.jar 158.1.
 *
 * Mirrors {@code StatusComp.update} floor reapply + duration decay using the
 * public {@code apply}/{@code unapply}/{@code getDuration} API (no physics/AI).
 */
public final class ParFloorStatus158 {
    static final int WORLD = 16;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParFloorStatus158: refusing to run: classpath version.properties reports '" + version
                + "', expected official 158.1");
            System.exit(2);
        }

        Vars.headless = true;
        Vars.platform = new mindustry.core.Platform(){};
        Vars.net = new mindustry.net.Net(Vars.platform.getNet());
        Core.files = new SdlFiles();
        Core.settings = new arc.Settings();
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Groups.init();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();
        Vars.state = new GameState();
        Vars.state.rules.disableUnitCap = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();

        setFloor(5, 5, Blocks.mud.asFloor());
        setFloor(6, 5, Blocks.water.asFloor());
        setFloor(7, 5, Blocks.slag.asFloor());

        long ticks = 0;

        Unit dagger = UnitTypes.dagger.create(mindustry.game.Team.crux);
        place(dagger, 5, 5);
        dagger.add();
        statusTick(dagger, 1f);
        float aMuddy = duration(dagger, StatusEffects.muddy);
        boolean aHasMuddy = dagger.hasEffect(StatusEffects.muddy);

        Unit atrax = UnitTypes.atrax.create(mindustry.game.Team.crux);
        place(atrax, 5, 5);
        atrax.add();
        statusTick(atrax, 1f);
        float bMuddy = duration(atrax, StatusEffects.muddy);
        boolean bHasMuddy = atrax.hasEffect(StatusEffects.muddy);

        Unit extend = UnitTypes.dagger.create(mindustry.game.Team.crux);
        place(extend, 5, 5);
        extend.add();
        extend.apply(StatusEffects.muddy, 10f);
        statusTick(extend, 1f);
        float cAfterOne = duration(extend, StatusEffects.muddy);
        statusTick(extend, 1f);
        float cAfterTwo = duration(extend, StatusEffects.muddy);

        Unit wet = UnitTypes.dagger.create(mindustry.game.Team.crux);
        place(wet, 6, 5);
        wet.add();
        wet.apply(StatusEffects.burning, 5f);
        statusTick(wet, 1f);
        float dWet = duration(wet, StatusEffects.wet);
        boolean dHasBurning = duration(wet, StatusEffects.burning) > 0f;

        Unit roam = UnitTypes.dagger.create(mindustry.game.Team.crux);
        place(roam, 5, 5);
        roam.add();
        statusTick(roam, 1f);
        place(roam, 10, 5);
        for(int i = 0; i < 5; i++) statusTick(roam, 1f);
        float eMuddy = duration(roam, StatusEffects.muddy);
        place(roam, 5, 5);
        statusTick(roam, 1f);
        float fMuddy = duration(roam, StatusEffects.muddy);

        Unit precept = UnitTypes.precept.create(mindustry.game.Team.crux);
        place(precept, 7, 5);
        precept.add();
        statusTick(precept, 1f);
        float gMelt = duration(precept, StatusEffects.melting);
        boolean gHasMelt = precept.hasEffect(StatusEffects.melting);

        Unit half = UnitTypes.dagger.create(mindustry.game.Team.crux);
        place(half, 5, 5);
        half.add();
        statusTick(half, 0.5f);
        float hMuddy = duration(half, StatusEffects.muddy);

        System.out.printf(
            "{\n"
                + "  \"probe_version\": \"158.1\",\n"
                + "  \"probe_name\": \"ParFloorStatus158\",\n"
                + "  \"tick\": %d,\n"
                + "  \"a_muddy_duration\": %.6f,\n"
                + "  \"a_has_muddy\": %b,\n"
                + "  \"b_muddy_duration\": %.6f,\n"
                + "  \"b_has_muddy\": %b,\n"
                + "  \"c_after_one\": %.6f,\n"
                + "  \"c_after_two\": %.6f,\n"
                + "  \"d_wet_duration\": %.6f,\n"
                + "  \"d_has_burning\": %b,\n"
                + "  \"e_muddy_duration\": %.6f,\n"
                + "  \"f_muddy_duration\": %.6f,\n"
                + "  \"g_melt_duration\": %.6f,\n"
                + "  \"g_has_melt\": %b,\n"
                + "  \"h_muddy_duration\": %.6f\n"
                + "}\n",
            ticks,
            aMuddy, aHasMuddy,
            bMuddy, bHasMuddy,
            cAfterOne, cAfterTwo,
            dWet, dHasBurning,
            eMuddy,
            fMuddy,
            gMelt, gHasMelt,
            hMuddy
        );
    }

    static void statusTick(Unit unit, float delta) {
        Time.delta = delta;
        Floor floor = unit.floorOn();
        if (unit.isGrounded() && !unit.type.hovering) {
            unit.apply(floor.status, floor.statusDuration);
        }
        decayTracked(unit, delta);
    }

    static void decayTracked(Unit unit, float delta) {
        decay(unit, StatusEffects.muddy, delta);
        decay(unit, StatusEffects.wet, delta);
        decay(unit, StatusEffects.burning, delta);
        decay(unit, StatusEffects.melting, delta);
        decay(unit, StatusEffects.tarred, delta);
        decay(unit, StatusEffects.freezing, delta);
    }

    static void decay(Unit unit, StatusEffect effect, float delta) {
        float time = unit.getDuration(effect);
        if (time <= 0f) return;
        if (effect.permanent) return;
        float next = Math.max(time - delta, 0f);
        unit.unapply(effect);
        if (next > 0f) {
            unit.apply(effect, next);
        }
    }

    static void setFloor(int x, int y, Floor floor) {
        Vars.world.tiles.get(x, y).setFloor(floor);
    }

    static void place(Unit unit, int tileX, int tileY) {
        unit.set(tileX * 8f, tileY * 8f);
        unit.elevation = 0f;
    }

    static float duration(Unit unit, StatusEffect effect) {
        return unit.getDuration(effect);
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParFloorStatus158.class.getResourceAsStream("/version.properties")) {
            if (in == null) {
                return "unknown";
            }
            Properties props = new Properties();
            props.load(in);
            String build = props.getProperty("build", "?");
            String type = props.getProperty("type", "?");
            return type + " " + build;
        }
    }
}
