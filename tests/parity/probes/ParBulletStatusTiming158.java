import java.io.InputStream;
import java.lang.reflect.Field;
import java.util.Locale;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.Mathf;
import arc.math.geom.Vec2;
import arc.struct.Seq;
import arc.util.Time;
import mindustry.Vars;
import mindustry.ai.types.CommandAI;
import mindustry.content.StatusEffects;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.entities.EntityCollisions;
import mindustry.entities.units.StatusEntry;
import mindustry.entities.units.WeaponMount;
import mindustry.game.Team;
import mindustry.gen.Groups;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.type.StatusEffect;
import mindustry.world.Tiles;
import mindustry.world.blocks.environment.Floor;

/**
 * P1-B2 differential probe: tick ordering for bullet → status → movement/weapons.
 *
 * Replays official 158.1 MechUnit component order each tick:
 *   VelComp (-1) → StatusComp → WeaponsComp → controller movement (sets vel)
 * then, after the unit pass, {@code BulletType.hitEntity} → {@code unit.apply(status)}.
 *
 * Traces N-1 / N / N+1 / N+2:
 *   A. bullet → speed (sapped)
 *   B. bullet → disarmed
 *   C. status expires on the firing tick (disarmed time → 0)
 *   D. reload multiplier applied and after expiry (electrified)
 */
public final class ParBulletStatusTiming158 {
    static final int WORLD = 16;
    static final float DELTA = 1f;
    static final float TARGET_X = 200f;
    static final float TARGET_Y = 80f;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParBulletStatusTiming158: refusing to run: classpath version.properties reports '"
                + version + "', expected official 158.1");
            System.exit(2);
        }

        boot();

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": \"158.1\",\n");
        json.append("  \"probe_name\": \"bullet-status-timing\",\n");
        json.append("  \"tick\": 0,\n");

        appendTrace(json, "speed_status", traceSpeedStatus(), false);
        appendTrace(json, "disarmed", traceDisarmed(), false);
        appendTrace(json, "expiry_on_fire", traceExpiryOnFire(), false);
        appendTrace(json, "reload_multiplier", traceReloadMultiplier(), true);
        json.append("\n}\n");
        System.out.print(json);
    }

    static void boot() {
        Vars.headless = true;
        Vars.platform = new mindustry.core.Platform(){};
        Core.files = new SdlFiles();
        Core.app = new arc.Application() {
            public arc.struct.Seq<arc.ApplicationListener> getListeners() {
                return new arc.struct.Seq<>();
            }
            public arc.Application.ApplicationType getType() {
                return arc.Application.ApplicationType.headless;
            }
            public boolean isHeadless() {
                return true;
            }
            public String getClipboardText() {
                return "";
            }
            public void setClipboardText(String text) {}
            public void post(Runnable r) {
                r.run();
            }
            public void exit() {}
        };
        Core.settings = new arc.Settings();
        Core.audio = new arc.audio.Audio(true);
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.init();
        Groups.init();
        Vars.logicVars = new GlobalVars();
        Vars.logicVars.init();
        Vars.net = new mindustry.net.Net(Vars.platform.getNet());
        Vars.state = new GameState();
        Vars.state.rules.disableUnitCap = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(WORLD, WORLD);
        Vars.world.tiles.fill();
        Vars.collisions = new EntityCollisions();
    }

    /** A: sapped from a bullet hit must not slow movement until N+1. */
    static Trace traceSpeedStatus() throws Exception {
        Unit unit = dagger(80f, 80f);
        setMoveIntent(unit);
        for (int i = 0; i < 3; i++) unitTick(unit, false, null, 0f);

        Trace t = new Trace();
        unitTick(unit, false, null, 0f);
        t.nMinus1 = snapshot(unit);

        unitTick(unit, true, StatusEffects.sapped, 180f);
        t.endN = snapshot(unit);

        unitTick(unit, false, null, 0f);
        t.endNPlus1 = snapshot(unit);

        unitTick(unit, false, null, 0f);
        t.endNPlus2 = snapshot(unit);
        return t;
    }

    /** B: disarmed from a bullet hit must not suppress firing until N+1. */
    static Trace traceDisarmed() throws Exception {
        Unit unit = dagger(80f, 80f);
        aimAt(unit, 95f, 80f);
        unit.mounts[0].reload = 0f;
        unit.mounts[0].warmup = 1f;
        unit.mounts[0].shoot = true;
        for (int i = 0; i < 2; i++) unitTick(unit, false, null, 0f);

        Trace t = new Trace();
        unitTick(unit, false, null, 0f);
        t.nMinus1 = snapshot(unit);

        int shotsBefore = unit.mounts[0].totalShots;
        unitTick(unit, true, StatusEffects.disarmed, 60f);
        t.endN = snapshot(unit);
        t.endN.shotsFired = unit.mounts[0].totalShots - shotsBefore;

        int shotsAtN = unit.mounts[0].totalShots;
        unitTick(unit, false, null, 0f);
        t.endNPlus1 = snapshot(unit);
        t.endNPlus1.shotsFired = unit.mounts[0].totalShots - shotsAtN;

        int shotsAtN1 = unit.mounts[0].totalShots;
        unitTick(unit, false, null, 0f);
        t.endNPlus2 = snapshot(unit);
        t.endNPlus2.shotsFired = unit.mounts[0].totalShots - shotsAtN1;
        return t;
    }

    /** C: disarmed expiring during status tick on N allows firing that same tick. */
    static Trace traceExpiryOnFire() throws Exception {
        Unit unit = dagger(80f, 80f);
        aimAt(unit, 95f, 80f);
        unit.apply(StatusEffects.disarmed, 2f);
        unitTick(unit, false, null, 0f); // 2 → 1, aggregate disarmed=true
        unit.mounts[0].reload = 0f;
        unit.mounts[0].warmup = 1f;
        unit.mounts[0].shoot = true;

        Trace t = new Trace();
        t.nMinus1 = snapshot(unit); // time=1, disarmed, reload ready — no tick yet

        int shotsBefore = unit.mounts[0].totalShots;
        unitTick(unit, false, null, 0f); // 1 → 0, removed before weapons → fire
        t.endN = snapshot(unit);
        t.endN.shotsFired = unit.mounts[0].totalShots - shotsBefore;

        int shotsAtN = unit.mounts[0].totalShots;
        unitTick(unit, false, null, 0f);
        t.endNPlus1 = snapshot(unit);
        t.endNPlus1.shotsFired = unit.mounts[0].totalShots - shotsAtN;

        int shotsAtN1 = unit.mounts[0].totalShots;
        unitTick(unit, false, null, 0f);
        t.endNPlus2 = snapshot(unit);
        t.endNPlus2.shotsFired = unit.mounts[0].totalShots - shotsAtN1;
        return t;
    }

    /** D: electrified reload multiplier during life and after expiry. */
    static Trace traceReloadMultiplier() throws Exception {
        Unit unit = dagger(80f, 80f);
        aimAt(unit, 95f, 80f);
        unit.apply(StatusEffects.electrified, 100f);
        unit.mounts[0].reload = 10f;
        unit.mounts[0].warmup = 1f;
        unit.mounts[0].shoot = true;

        Trace t = new Trace();
        unitTick(unit, false, null, 0f);
        t.nMinus1 = snapshot(unit);

        unitTick(unit, false, null, 0f);
        t.endN = snapshot(unit);

        unitTick(unit, false, null, 0f);
        t.endNPlus1 = snapshot(unit);

        unit.apply(StatusEffects.electrified, 1f);
        unitTick(unit, false, null, 0f); // expires
        t.endNPlus2 = snapshot(unit);
        return t;
    }

    static Unit dagger(float x, float y) {
        Unit unit = UnitTypes.dagger.create(Team.sharded);
        unit.set(x, y);
        unit.add();
        Vars.state.teams.updateTeamStats();
        return unit;
    }

    static void aimAt(Unit unit, float x, float y) {
        unit.mounts[0].aimX = x;
        unit.mounts[0].aimY = y;
        unit.mounts[0].shoot = true;
        unit.mounts[0].rotate = true;
    }

    static void setMoveIntent(Unit unit) {
        float speed = unit.type().speed * 60f / 8f * unit.speedMultiplier();
        float dx = TARGET_X - unit.x(), dy = TARGET_Y - unit.y();
        unit.vel.set(dx, dy).setLength(speed / 60f);
    }

    /**
     * One tick in official component order, optionally applying a bullet status
     * after the unit pass (Logic.updateEntities: unit update then bullet collide).
     */
    static void unitTick(Unit unit, boolean bulletHit, StatusEffect effect, float duration) throws Exception {
        Time.delta = DELTA;
        Time.time += DELTA;

        // VelComp (-1): integrate velocity set by the previous tick's controller.
        float px = unit.x(), py = unit.y();
        unit.move(unit.vel.x * DELTA, unit.vel.y * DELTA);
        if (Mathf.equal(px, unit.x())) unit.vel.x = 0f;
        if (Mathf.equal(py, unit.y())) unit.vel.y = 0f;
        unit.vel.scl(Math.max(1f - unit.drag() * DELTA, 0f));

        // StatusComp.update
        statusTick(unit);

        // WeaponsComp.update
        for (WeaponMount mount : unit.mounts) {
            mount.weapon.update(unit, mount);
        }

        // Controller movement intent for the next tick (CommandAI would pathfind;
        // here we set velocity directly using the post-status speed multiplier).
        setMoveIntent(unit);

        if (bulletHit && effect != null) {
            unit.apply(effect, duration);
        }
    }

    static void statusTick(Unit unit) throws Exception {
        Floor floor = unit.floorOn();
        if (unit.isGrounded() && !unit.type().hovering) {
            unit.apply(floor.status, floor.statusDuration);
        }
        Seq<StatusEntry> statuses = statusesOf(unit);
        int index = 0;
        while (index < statuses.size) {
            StatusEntry entry = statuses.get(index++);
            entry.time = Math.max(entry.time - Time.delta, 0f);
            if (entry.effect == null || (entry.time <= 0f && !entry.effect.permanent)) {
                if (entry.effect != null) entry.effect.onRemoved(unit);
                index--;
                statuses.remove(index);
            } else {
                entry.effect.update(unit, entry);
            }
        }
        // Recompute aggregate multipliers like StatusComp.update tail.
        unit.speedMultiplier = 1f;
        unit.damageMultiplier = 1f;
        unit.healthMultiplier = 1f;
        unit.reloadMultiplier = 1f;
        unit.buildSpeedMultiplier = 1f;
        unit.dragMultiplier = 1f;
        unit.armorOverride = -1f;
        unit.disarmed = false;
        for (StatusEntry entry : statuses) {
            if (entry.effect.dynamic) {
                unit.speedMultiplier *= entry.speedMultiplier;
                unit.healthMultiplier *= entry.healthMultiplier;
                unit.damageMultiplier *= entry.damageMultiplier;
                unit.reloadMultiplier *= entry.reloadMultiplier;
                unit.buildSpeedMultiplier *= entry.buildSpeedMultiplier;
                unit.dragMultiplier *= entry.dragMultiplier;
                if (entry.armorOverride >= 0f) unit.armorOverride = entry.armorOverride;
            } else {
                unit.speedMultiplier *= entry.effect.speedMultiplier;
                unit.healthMultiplier *= entry.effect.healthMultiplier;
                unit.damageMultiplier *= entry.effect.damageMultiplier;
                unit.reloadMultiplier *= entry.effect.reloadMultiplier;
                unit.buildSpeedMultiplier *= entry.effect.buildSpeedMultiplier;
                unit.dragMultiplier *= entry.effect.dragMultiplier;
            }
            unit.disarmed |= entry.effect.disarm;
        }
    }

    static Snapshot snapshot(Unit unit) throws Exception {
        Snapshot s = new Snapshot();
        s.health = unit.health();
        Seq<StatusEntry> statuses = statusesOf(unit);
        s.statusIds = new int[statuses.size];
        s.statusTimes = new float[statuses.size];
        for (int i = 0; i < statuses.size; i++) {
            s.statusIds[i] = statuses.get(i).effect.id;
            s.statusTimes[i] = statuses.get(i).time;
        }
        s.speed = unit.speedMultiplier();
        s.x = unit.x();
        s.y = unit.y();
        s.reload = unit.mounts[0].reload;
        s.reloadMult = unit.reloadMultiplier();
        s.shotsFired = 0;
        s.disarmed = unit.disarmed();
        s.canShoot = unit.canShoot();
        return s;
    }

    @SuppressWarnings("unchecked")
    static Seq<StatusEntry> statusesOf(Unit unit) throws Exception {
        Class<?> c = unit.getClass();
        Field field = null;
        while (c != null) {
            try {
                field = c.getDeclaredField("statuses");
                break;
            } catch (NoSuchFieldException ignored) {
                c = c.getSuperclass();
            }
        }
        if (field == null) throw new IllegalStateException("statuses field missing");
        field.setAccessible(true);
        return (Seq<StatusEntry>) field.get(unit);
    }

    static void appendTrace(StringBuilder json, String name, Trace trace, boolean last) {
        json.append("  \"").append(name).append("\": {\n");
        appendPhase(json, "n_minus_1", trace.nMinus1);
        appendPhase(json, "end_n", trace.endN);
        appendPhase(json, "end_n_plus_1", trace.endNPlus1);
        appendPhase(json, "end_n_plus_2", trace.endNPlus2, true);
        json.append("  }").append(last ? "" : ",");
        json.append("\n");
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s) {
        appendPhase(json, phase, s, false);
    }

    static void appendPhase(StringBuilder json, String phase, Snapshot s, boolean lastPhase) {
        json.append("    \"").append(phase).append("\": {");
        json.append("\"health\": ").append(num(s.health)).append(", ");
        json.append("\"status_ids\": ").append(ints(s.statusIds)).append(", ");
        json.append("\"status_times\": ").append(floats(s.statusTimes)).append(", ");
        json.append("\"speed\": ").append(num(s.speed)).append(", ");
        json.append("\"x\": ").append(num(s.x)).append(", ");
        json.append("\"y\": ").append(num(s.y)).append(", ");
        json.append("\"shots_fired\": ").append(s.shotsFired).append(", ");
        json.append("\"reload\": ").append(num(s.reload)).append(", ");
        json.append("\"reload_mult\": ").append(num(s.reloadMult)).append(", ");
        json.append("\"disarmed\": ").append(s.disarmed).append(", ");
        json.append("\"can_shoot\": ").append(s.canShoot);
        json.append("}").append(lastPhase ? "" : ",");
        json.append("\n");
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParBulletStatusTiming158.class.getResourceAsStream("/version.properties")) {
            if (in == null) return "unknown";
            Properties props = new Properties();
            props.load(in);
            return props.getProperty("type", "?") + " " + props.getProperty("build", "?");
        }
    }

    static final class Trace {
        Snapshot nMinus1, endN, endNPlus1, endNPlus2;
    }

    static final class Snapshot {
        float health;
        int[] statusIds = new int[0];
        float[] statusTimes = new float[0];
        float speed;
        float x, y;
        int shotsFired;
        float reload;
        float reloadMult;
        boolean disarmed;
        boolean canShoot;
    }

    static String ints(int[] values) {
        StringBuilder out = new StringBuilder("[");
        for (int i = 0; i < values.length; i++) {
            if (i > 0) out.append(", ");
            out.append(values[i]);
        }
        return out.append("]").toString();
    }

    static String floats(float[] values) {
        StringBuilder out = new StringBuilder("[");
        for (int i = 0; i < values.length; i++) {
            if (i > 0) out.append(", ");
            out.append(num(values[i]));
        }
        return out.append("]").toString();
    }

    static String num(float value) {
        if (Float.isInfinite(value)) return value > 0 ? "1e38" : "-1e38";
        if (Float.isNaN(value)) return "null";
        return String.format(Locale.US, "%.6f", value);
    }
}
