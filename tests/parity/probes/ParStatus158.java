import java.io.InputStream;
import java.lang.reflect.Field;
import java.util.Locale;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.struct.Seq;
import arc.util.Time;
import mindustry.Vars;
import mindustry.content.Blocks;
import mindustry.content.StatusEffects;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.entities.units.StatusEntry;
import mindustry.gen.Groups;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.type.StatusEffect;
import mindustry.world.Tiles;
import mindustry.world.blocks.environment.Floor;

/**
 * P0-E2 differential probe: StatusComp apply/update on desktop.jar 158.1.
 *
 * Drives the official {@code apply} transition table and the StatusComp.update
 * collection loop (floor reapply, duration decay, StatusEffect.update, first-match
 * insertion order). Dumps ordered entries plus the aggregate multipliers that
 * gameplay consumes.
 */
public final class ParStatus158 {
    static final int WORLD = 16;

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParStatus158: refusing to run: classpath version.properties reports '" + version
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

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": \"158.1\",\n");
        json.append("  \"probe_name\": \"ParStatus158\",\n");
        json.append("  \"tick\": 0,\n");

        boolean first = true;
        first = caseOut(json, first, "burning_tarred", burningTarred());
        first = caseOut(json, first, "tarred_burning", tarredBurning());
        first = caseOut(json, first, "melting_tarred", meltingTarred());
        first = caseOut(json, first, "tarred_melting", tarredMelting());
        first = caseOut(json, first, "wet_shocked", wetShocked());
        first = caseOut(json, first, "freezing_blasted", freezingBlasted());
        first = caseOut(json, first, "opposites", opposites());
        first = caseOut(json, first, "first_match", firstMatch());
        first = caseOut(json, first, "corroded_timer", corrodedTimer());
        first = caseOut(json, first, "corroded_phases", corrodedPhases());
        first = caseOut(json, first, "corroded_delta_gt_interval", corrodedDeltaGtInterval());
        first = caseOut(json, first, "corroded_expiry_on_fire", corrodedExpiryOnFire());
        first = caseOut(json, first, "disarmed", disarmed());
        first = caseOut(json, first, "overdrive", overdrive());
        first = caseOut(json, first, "boss", boss());
        first = caseOut(json, first, "infinity", infinity());
        first = caseOut(json, first, "dynamic", dynamic());
        first = caseOut(json, first, "floor", floor());
        first = caseOut(json, first, "hovering", hovering());
        json.append("\n}\n");
        System.out.print(json);
    }

    static Dump burningTarred() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.burning, 200f);
        u.apply(StatusEffects.tarred, 200f);
        return dump(u);
    }

    static Dump tarredBurning() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.tarred, 200f);
        u.apply(StatusEffects.burning, 200f);
        return dump(u);
    }

    static Dump meltingTarred() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.melting, 150f);
        u.apply(StatusEffects.tarred, 100f);
        return dump(u);
    }

    static Dump tarredMelting() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.tarred, 150f);
        u.apply(StatusEffects.melting, 100f);
        return dump(u);
    }

    static Dump wetShocked() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.wet, 60f);
        u.apply(StatusEffects.shocked, 1f);
        return dump(u);
    }

    static Dump freezingBlasted() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.freezing, 60f);
        u.apply(StatusEffects.blasted, 1f);
        return dump(u);
    }

    static Dump opposites() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.burning, 10f);
        u.apply(StatusEffects.wet, 20f);
        return dump(u);
    }

    static Dump firstMatch() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.sapped, 50f);
        u.apply(StatusEffects.burning, 10f);
        u.apply(StatusEffects.wet, 20f);
        return dump(u);
    }

    static Dump corrodedTimer() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.corroded, 100f);
        statusTick(u, 1f);
        return dump(u);
    }

    static Dump corrodedPhases() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.corroded, 100f);
        statusTick(u, 14f);
        float hp14 = u.health;
        float dt14 = damageTime(u, StatusEffects.corroded);
        statusTick(u, 1f);
        float hp15 = u.health;
        float dt15 = damageTime(u, StatusEffects.corroded);
        statusTick(u, 15f);
        Dump d = dump(u);
        d.extra = String.format(Locale.US,
            ", \"hp_after_14\": %.6f, \"dt_after_14\": %.6f, \"hp_after_15\": %.6f, \"dt_after_15\": %.6f, \"fired_at_15\": %b, \"fired_at_30\": %b",
            hp14, dt14, hp15, dt15, hp15 < hp14 - 0.01f, u.health < hp15 - 0.01f);
        return d;
    }

    static Dump corrodedDeltaGtInterval() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.corroded, 100f);
        statusTick(u, 40f);
        return dump(u);
    }

    static Dump corrodedExpiryOnFire() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.corroded, 5f);
        float before = u.health;
        statusTick(u, 10f);
        Dump d = dump(u);
        d.extra = String.format(Locale.US, ", \"fired_on_expiry\": %b", u.health < before - 0.01f);
        return d;
    }

    static Dump disarmed() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.disarmed, 60f);
        statusTick(u, 1f);
        return dump(u);
    }

    static Dump overdrive() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.overdrive, 1f);
        statusTick(u, 1f);
        return dump(u);
    }

    static Dump boss() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.boss, 1f);
        statusTick(u, 1f);
        return dump(u);
    }

    static Dump infinity() throws Exception {
        Unit u = dagger();
        u.apply(StatusEffects.overdrive, Float.POSITIVE_INFINITY);
        u.apply(StatusEffects.boss, 0f);
        statusTick(u, 1_000_000f);
        return dump(u);
    }

    static Dump dynamic() throws Exception {
        Unit u = dagger();
        StatusEntry entry = u.applyDynamicStatus();
        entry.speedMultiplier = 2f;
        entry.healthMultiplier = 0.5f;
        entry.damageMultiplier = 3f;
        entry.reloadMultiplier = 0.25f;
        entry.buildSpeedMultiplier = 4f;
        entry.dragMultiplier = 1.5f;
        entry.armorOverride = 10f;
        statusTick(u, 1f);
        return dump(u);
    }

    static Dump floor() throws Exception {
        Unit u = dagger();
        place(u, 5, 5);
        statusTick(u, 1f);
        return dump(u);
    }

    static Dump hovering() throws Exception {
        Unit u = UnitTypes.atrax.create(mindustry.game.Team.crux);
        u.health = u.maxHealth;
        place(u, 5, 5);
        u.add();
        statusTick(u, 1f);
        return dump(u);
    }

    static Unit dagger() {
        Unit u = UnitTypes.dagger.create(mindustry.game.Team.crux);
        u.health = u.maxHealth;
        place(u, 10, 10);
        u.add();
        return u;
    }

    static void statusTick(Unit unit, float delta) throws Exception {
        Time.delta = delta;
        Floor floor = unit.floorOn();
        if (unit.isGrounded() && !unit.type.hovering) {
            unit.apply(floor.status, floor.statusDuration);
        }
        Seq<StatusEntry> statuses = statusesOf(unit);
        int index = 0;
        while (index < statuses.size) {
            StatusEntry entry = statuses.get(index++);
            entry.time = Math.max(entry.time - Time.delta, 0f);
            if (entry.effect == null || (entry.time <= 0f && !entry.effect.permanent)) {
                index--;
                statuses.remove(index);
            } else {
                entry.effect.update(unit, entry);
            }
        }
    }

    static Dump dump(Unit unit) throws Exception {
        Seq<StatusEntry> statuses = statusesOf(unit);
        Dump d = new Dump();
        d.ids = new int[statuses.size];
        d.times = new float[statuses.size];
        d.damageTimes = new float[statuses.size];
        float speed = 1f, damage = 1f, health = 1f, reload = 1f, build = 1f, drag = 1f;
        float armor = -1f;
        boolean disarmed = false;
        for (int i = 0; i < statuses.size; i++) {
            StatusEntry entry = statuses.get(i);
            d.ids[i] = entry.effect.id;
            d.times[i] = entry.time;
            d.damageTimes[i] = entry.damageTime;
            if (entry.effect.dynamic) {
                speed *= entry.speedMultiplier;
                health *= entry.healthMultiplier;
                damage *= entry.damageMultiplier;
                reload *= entry.reloadMultiplier;
                build *= entry.buildSpeedMultiplier;
                drag *= entry.dragMultiplier;
                if (entry.armorOverride >= 0f) armor = entry.armorOverride;
            } else {
                speed *= entry.effect.speedMultiplier;
                health *= entry.effect.healthMultiplier;
                damage *= entry.effect.damageMultiplier;
                reload *= entry.effect.reloadMultiplier;
                build *= entry.effect.buildSpeedMultiplier;
                drag *= entry.effect.dragMultiplier;
            }
            disarmed |= entry.effect.disarm;
        }
        d.health = unit.health;
        d.healthMult = health;
        d.speed = speed;
        d.damage = damage;
        d.reload = reload;
        d.buildSpeed = build;
        d.drag = drag;
        d.armorOverride = armor;
        d.disarmed = disarmed;
        d.canShoot = !disarmed && !(unit.type.canBoost && unit.isFlying());
        d.healthInfinite = Float.isInfinite(health);
        return d;
    }

    static float damageTime(Unit unit, StatusEffect effect) throws Exception {
        for (StatusEntry entry : statusesOf(unit)) {
            if (entry.effect == effect) return entry.damageTime;
        }
        return 0f;
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
        if (field == null) {
            throw new IllegalStateException("statuses field missing on " + unit.getClass());
        }
        field.setAccessible(true);
        return (Seq<StatusEntry>) field.get(unit);
    }

    static void setFloor(int x, int y, Floor floor) {
        Vars.world.tiles.get(x, y).setFloor(floor);
    }

    static void place(Unit unit, int tileX, int tileY) {
        unit.set(tileX * 8f, tileY * 8f);
        unit.elevation = 0f;
    }

    static boolean caseOut(StringBuilder json, boolean first, String name, Dump dump) {
        if (!first) json.append(",\n");
        json.append("  \"").append(name).append("\": ");
        dump.append(json);
        return false;
    }

    static String classpathVersion() throws Exception {
        try (InputStream in = ParStatus158.class.getResourceAsStream("/version.properties")) {
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

    static final class Dump {
        int[] ids = new int[0];
        float[] times = new float[0];
        float[] damageTimes = new float[0];
        float health;
        float healthMult;
        float speed;
        float damage;
        float reload;
        float buildSpeed;
        float drag;
        float armorOverride;
        boolean disarmed;
        boolean canShoot;
        boolean healthInfinite;
        String extra = "";

        void append(StringBuilder json) {
            json.append("{");
            json.append("\"ids\": ").append(ints(ids));
            json.append(", \"times\": ").append(floats(times));
            json.append(", \"damage_times\": ").append(floats(damageTimes));
            json.append(", \"health\": ").append(num(health));
            json.append(", \"health_mult\": ").append(healthInfinite ? "null" : num(healthMult));
            json.append(", \"health_infinite\": ").append(healthInfinite);
            json.append(", \"speed\": ").append(num(speed));
            json.append(", \"damage\": ").append(num(damage));
            json.append(", \"reload\": ").append(num(reload));
            json.append(", \"build_speed\": ").append(num(buildSpeed));
            json.append(", \"drag\": ").append(num(drag));
            if (armorOverride < 0f) {
                json.append(", \"armor_override\": null");
            } else {
                json.append(", \"armor_override\": ").append(num(armorOverride));
            }
            json.append(", \"disarmed\": ").append(disarmed);
            json.append(", \"can_shoot\": ").append(canShoot);
            json.append(extra);
            json.append("}");
        }
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
        if (Float.isInfinite(value)) {
            return value > 0 ? "1e38" : "-1e38";
        }
        if (Float.isNaN(value)) {
            return "null";
        }
        return String.format(Locale.US, "%.6f", value);
    }
}
