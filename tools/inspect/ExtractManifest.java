import mindustry.Vars;
import arc.Core;
import arc.Settings;
import arc.func.Prov;
import arc.struct.ObjectMap;
import arc.struct.Seq;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.game.Rules;
import mindustry.gen.EntityMapping;
import mindustry.io.SaveIO;
import mindustry.io.SaveVersion;
import mindustry.io.TypeIO;
import mindustry.logic.LAccess;
import mindustry.net.Net;
import mindustry.net.Packet;
import mindustry.net.Streamable;
import mindustry.type.UnitType;
import mindustry.world.Block;
import mindustry.type.Item;
import mindustry.type.Liquid;
import mindustry.type.Weather;
import mindustry.type.StatusEffect;
import mindustry.ai.UnitCommand;
import mindustry.ai.UnitStance;

import java.io.File;
import java.io.InputStream;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.file.Files;
import java.security.MessageDigest;
import java.util.*;
import java.util.jar.JarEntry;
import java.util.jar.JarFile;

public final class ExtractManifest {
    private static final StringBuilder OUT = new StringBuilder();

    public static void main(String[] args) throws Exception {
        String jarPath = args.length > 0 ? args[0] : "";
        if (jarPath.isEmpty()) {
            System.err.println("Usage: ExtractManifest <path-to-jar>");
            System.exit(1);
        }

        arc.util.Log.logger = (level, text) -> {};
        Vars.content = new ContentLoader();
        Vars.state = new GameState();
        Core.settings = new Settings();
        Vars.content.createBaseContent();
        Vars.content.init();

        Map<String, Boolean> capabilities = new LinkedHashMap<>();

        objStart();
        // version
        JarFile jarFile = new JarFile(new File(jarPath));
        Properties vProps = new Properties();
        JarEntry versionEntry = jarFile.getJarEntry("version.properties");
        if (versionEntry != null) {
            try (InputStream is = jarFile.getInputStream(versionEntry)) {
                vProps.load(is);
            }
        }

        key("version");
        objStart();
        field("number", vProps.getProperty("number", ""), false);
        field("build", vProps.getProperty("build", ""), false);
        field("type", vProps.getProperty("type", ""), false);
        field("modifier", vProps.getProperty("modifier", ""), false);
        field("buildDate", vProps.getProperty("buildDate", ""), true);
        objEnd(false);

        // saves
        key("save_versions");
        arrStart();
        Field saveVersionsField = SaveIO.class.getDeclaredField("versionArray");
        saveVersionsField.setAccessible(true);
        Seq<?> saveVersions = (Seq<?>) saveVersionsField.get(null);
        capabilities.put("save_versions", true);
        for (int i = 0; i < saveVersions.size; i++) {
            SaveVersion sv = (SaveVersion) saveVersions.get(i);
            objStart();
            fieldNum("version", sv.version, false);
            field("class", sv.getClass().getSimpleName(), true);
            objEnd(i + 1 == saveVersions.size);
        }
        arrEnd(false);

        Field packetClassesField = Net.class.getDeclaredField("packetClasses");
        packetClassesField.setAccessible(true);
        Seq<?> packetClasses = (Seq<?>) packetClassesField.get(null);
        capabilities.put("packet_classes", true);

        Integer worldStreamId = optionalIntField(Net.class, "packetIdWorldStream");
        Integer assetStreamId = optionalIntField(Net.class, "packetIdAssetStream");
        capabilities.put("packetIdWorldStream", worldStreamId != null);
        capabilities.put("packetIdAssetStream", assetStreamId != null);

        key("packets");
        arrStart();
        for (int i = 0; i < packetClasses.size; i++) {
            Class<?> cls = (Class<?>) packetClasses.get(i);
            objStart();
            fieldNum("id", i, false);
            field("name", cls.getSimpleName(), false);
            field("class", cls.getName(), true);
            objEnd(i + 1 == packetClasses.size);
        }
        arrEnd(false);

        key("streams");
        arrStart();
        List<Integer> streamIdx = new ArrayList<>();
        for (int i = 0; i < packetClasses.size; i++) {
            Class<?> cls = (Class<?>) packetClasses.get(i);
            if (Streamable.class.isAssignableFrom(cls) || cls.getSimpleName().contains("Stream")) {
                streamIdx.add(i);
            }
        }
        for (int s = 0; s < streamIdx.size(); s++) {
            int i = streamIdx.get(s);
            Class<?> cls = (Class<?>) packetClasses.get(i);
            boolean incremental = false;
            String lifecycle = "unknown";
            try {
                Packet inst = (Packet) cls.getDeclaredConstructor().newInstance();
                try {
                    Method inc = inst.getClass().getMethod("incremental");
                    incremental = Boolean.TRUE.equals(inc.invoke(inst));
                } catch (NoSuchMethodException ignored) {
                    try {
                        Method inc = Streamable.class.getMethod("incremental");
                        incremental = Boolean.TRUE.equals(inc.invoke(inst));
                    } catch (NoSuchMethodException ignored2) {}
                }
            } catch (Throwable ignored) {}
            String simple = cls.getSimpleName();
            if (simple.equals("StreamBegin") || simple.equals("StreamChunk")) {
                lifecycle = "framing";
            } else if (simple.equals("WorldStream")) {
                lifecycle = "world_join";
            } else if (simple.contains("Asset")) {
                lifecycle = "asset_join";
            }
            objStart();
            fieldNum("packet_id", i, false);
            field("class_name", simple, false);
            field("class", cls.getName(), false);
            fieldNum("registration_order", i, false);
            field("lifecycle", lifecycle, false);
            fieldBool("incremental", incremental, true);
            objEnd(s + 1 == streamIdx.size());
        }
        arrEnd(false);

        key("rpc");
        arrStart();
        List<Integer> rpcIdx = new ArrayList<>();
        for (int i = 0; i < packetClasses.size; i++) {
            Class<?> cls = (Class<?>) packetClasses.get(i);
            if (cls.getSimpleName().endsWith("CallPacket")) {
                rpcIdx.add(i);
            }
        }
        for (int r = 0; r < rpcIdx.size(); r++) {
            int i = rpcIdx.get(r);
            Class<?> cls = (Class<?>) packetClasses.get(i);
            Packet inst = null;
            try {
                inst = (Packet) cls.getDeclaredConstructor().newInstance();
            } catch (Throwable ignored) {}
            int priority = inst == null ? Packet.priorityNormal : inst.getPriority();
            boolean handleServer = hasDeclared(cls, "handleServer");
            boolean handleClient = hasDeclared(cls, "handleClient");
            String direction = handleServer && handleClient ? "both" : handleServer ? "client_to_server" : handleClient ? "server_to_client" : "unknown";
            String remote = cls.getSimpleName().replace("CallPacket", "");
            remote = remote.replaceAll("\\d+$", "");
            objStart();
            fieldNum("packet_id", i, false);
            field("generated_class", cls.getSimpleName(), false);
            field("source_remote", uncapitalize(remote), false);
            field("direction", direction, false);
            fieldNum("priority", priority, false);
            fieldBool("has_handle_server", handleServer, false);
            fieldBool("has_handle_client", handleClient, false);
            key("parameter_types");
            arrStart();
            List<Field> params = publicInstanceFields(cls);
            for (int p = 0; p < params.size(); p++) {
                Field f = params.get(p);
                if (f.getName().equals("DATA")) {
                    continue;
                }
                raw("\"" + esc(f.getType().getTypeName()) + ":" + esc(f.getName()) + "\"");
                if (p + 1 < params.size()) comma();
            }
            arrEnd(true);
            objEnd(r + 1 == rpcIdx.size());
        }
        arrEnd(false);

        key("typeio");
        objStart();
        key("methods");
        arrStart();
        Method[] typeioMethods = TypeIO.class.getDeclaredMethods();
        Arrays.sort(typeioMethods, Comparator.comparing(Method::getName).thenComparing(m -> Arrays.toString(m.getParameterTypes())));
        List<Method> listed = new ArrayList<>();
        for (Method m : typeioMethods) {
            if (Modifier.isPublic(m.getModifiers()) && Modifier.isStatic(m.getModifiers())) {
                listed.add(m);
            }
        }
        for (int i = 0; i < listed.size(); i++) {
            Method m = listed.get(i);
            objStart();
            field("name", m.getName(), false);
            field("return", m.getReturnType().getTypeName(), false);
            key("params");
            arrStart();
            Class<?>[] pts = m.getParameterTypes();
            for (int p = 0; p < pts.length; p++) {
                raw("\"" + esc(pts[p].getTypeName()) + "\"");
                if (p + 1 < pts.length) comma();
            }
            arrEnd(true);
            objEnd(i + 1 == listed.size());
        }
        arrEnd(true);
        objEnd(false);
        capabilities.put("typeio_methods", true);

        emitContent();

        key("entities");
        arrStart();
        capabilities.put("entity_mapping", true);
        Prov[] idMap = EntityMapping.idMap;
        int last = 0;
        for (int i = 0; i < idMap.length; i++) {
            if (idMap[i] != null) last = i;
        }
        boolean firstEnt = true;
        for (int i = 0; i <= last; i++) {
            if (idMap[i] == null) continue;
            String className = "unknown";
            boolean serialize = false;
            try {
                Object inst = idMap[i].get();
                if (inst != null) {
                    className = inst.getClass().getName();
                    serialize = hasDeclared(inst.getClass(), "write") || hasDeclared(inst.getClass(), "writeSync");
                }
            } catch (Throwable t) {
                className = "uninstantiable";
            }
            if (!firstEnt) comma();
            firstEnt = false;
            objStart();
            fieldNum("class_id", i, false);
            field("class_name", className, false);
            fieldBool("serialize", serialize, true);
            objEnd(true);
        }
        arrEnd(false);

        key("entity_sync");
        objStart();
        fieldNum("id_map_length", idMap.length, false);
        fieldNum("mapped_count", last + 1, false);
        String mappingFp = sha256Bytes(Integer.toString(last).getBytes());
        try {
            Field nameMapF = EntityMapping.class.getField("nameMap");
            ObjectMap<?, ?> nameMap = (ObjectMap<?, ?>) nameMapF.get(null);
            fieldNum("alias_count", nameMap.size, false);
            mappingFp = sha256Bytes((last + ":" + nameMap.size).getBytes());
        } catch (Throwable t) {
            fieldNum("alias_count", -1, false);
        }
        field("mapping_fingerprint", mappingFp, true);
        objEnd(false);

        key("rules_fields");
        arrStart();
        Rules rules = new Rules();
        Field[] ruleFields = Rules.class.getFields();
        List<Field> validRuleFields = new ArrayList<>();
        for (Field f : ruleFields) {
            if (!Modifier.isStatic(f.getModifiers()) && Modifier.isPublic(f.getModifiers())) {
                validRuleFields.add(f);
            }
        }
        validRuleFields.sort(Comparator.comparing(Field::getName));
        for (int i = 0; i < validRuleFields.size(); i++) {
            Field f = validRuleFields.get(i);
            Object val = f.get(rules);
            // Object.toString() includes an identity hash for these mutable rule
            // containers, which is JVM/run dependent. Preserve the semantic
            // default while keeping the manifest payload deterministic.
            String valStr = stableRuleDefault(f, val);
            objStart();
            field("name", f.getName(), false);
            field("type", f.getType().getSimpleName(), false);
            field("default", valStr, true);
            objEnd(i + 1 == validRuleFields.size());
        }
        arrEnd(false);

        key("logic");
        objStart();
        emitEnumArray("access", valuesOf("mindustry.logic.LAccess"));
        comma();
        emitEnumArray("unit_control", valuesOf("mindustry.logic.LUnitControl"));
        comma();
        emitEnumArray("locate", valuesOf("mindustry.logic.LLocate"));
        comma();
        emitEnumArray("rules", valuesOf("mindustry.logic.LogicRule"));
        comma();
        key("instructions");
        arrStart();
        try {
            Class<?> exec = Class.forName("mindustry.logic.LExecutor");
            Class<?>[] nested = exec.getDeclaredClasses();
            Arrays.sort(nested, Comparator.comparing(Class::getSimpleName));
            boolean first = true;
            for (Class<?> n : nested) {
                if (!n.getSimpleName().endsWith("I")) continue;
                int arity = 0;
                for (var c : n.getDeclaredConstructors()) {
                    arity = Math.max(arity, c.getParameterCount());
                }
                if (!first) comma();
                first = false;
                objStart();
                field("name", n.getSimpleName(), false);
                fieldNum("constructor_arity", arity, true);
                objEnd(true);
            }
        } catch (Throwable ignored) {}
        arrEnd(true);
        objEnd(false);

        key("capabilities");
        objStart();
        int ci = 0;
        for (Map.Entry<String, Boolean> e : capabilities.entrySet()) {
            ci++;
            fieldBool(e.getKey(), e.getValue(), ci == capabilities.size());
        }
        objEnd(true);
        objEnd(true);

        jarFile.close();
        System.out.print(OUT.toString());
    }

    private static String stableRuleDefault(Field field, Object value) {
        if (value == null) return "null";
        String type = field.getType().getName();
        if (type.equals("mindustry.game.MapObjectives") ||
            type.equals("mindustry.game.Rules$TeamRules")) {
            return type;
        }
        return value.toString();
    }

    private static void emitContent() throws Exception {
        key("items");
        arrStart();
        var items = Vars.content.items();
        for (int i = 0; i < items.size; i++) {
            Item it = items.get(i);
            objStart();
            fieldNum("id", it.id, false);
            field("name", it.name, false);
            fieldFloat("explosiveness", it.explosiveness, false);
            fieldFloat("flammability", it.flammability, false);
            fieldFloat("radioactivity", it.radioactivity, false);
            fieldFloat("charge", it.charge, false);
            fieldNum("hardness", it.hardness, false);
            fieldFloat("cost", it.cost, true);
            objEnd(i + 1 == items.size);
        }
        arrEnd(false);

        key("liquids");
        arrStart();
        var liquids = Vars.content.liquids();
        for (int i = 0; i < liquids.size; i++) {
            Liquid lq = liquids.get(i);
            objStart();
            fieldNum("id", lq.id, false);
            field("name", lq.name, false);
            fieldFloat("flammability", lq.flammability, false);
            fieldFloat("temperature", lq.temperature, false);
            fieldFloat("heatCapacity", lq.heatCapacity, false);
            fieldFloat("viscosity", lq.viscosity, false);
            fieldFloat("explosiveness", lq.explosiveness, true);
            objEnd(i + 1 == liquids.size);
        }
        arrEnd(false);

        key("weathers");
        arrStart();
        var weathers = Vars.content.getBy(mindustry.ctype.ContentType.weather);
        for (int i = 0; i < weathers.size; i++) {
            Weather w = (Weather) weathers.get(i);
            objStart();
            fieldNum("id", w.id, false);
            field("name", w.name, true);
            objEnd(i + 1 == weathers.size);
        }
        arrEnd(false);

        key("status_effects");
        arrStart();
        var statuses = Vars.content.statusEffects();
        for (int i = 0; i < statuses.size; i++) {
            StatusEffect st = statuses.get(i);
            objStart();
            fieldNum("id", st.id, false);
            field("name", st.name, false);
            fieldFloat("damage", st.damage, false);
            fieldFloat("damageMultiplier", st.damageMultiplier, false);
            fieldFloat("speedMultiplier", st.speedMultiplier, false);
            fieldFloat("healthMultiplier", st.healthMultiplier, false);
            fieldFloat("reloadMultiplier", st.reloadMultiplier, false);
            fieldFloat("buildSpeedMultiplier", st.buildSpeedMultiplier, false);
            fieldFloat("dragMultiplier", st.dragMultiplier, false);
            fieldBool("disarm", st.disarm, true);
            objEnd(i + 1 == statuses.size);
        }
        arrEnd(false);

        key("units");
        arrStart();
        var units = Vars.content.units();
        for (int i = 0; i < units.size; i++) {
            UnitType u = units.get(i);
            boolean isB = false;
            try {
                Field fb = UnitType.class.getField("isBuilding");
                isB = fb.getBoolean(u);
            } catch (Throwable ignored) {}
            objStart();
            fieldNum("id", u.id, false);
            field("name", u.name, false);
            fieldFloat("health", u.health, false);
            fieldFloat("speed", u.speed, false);
            fieldFloat("hitSize", u.hitSize, false);
            fieldNum("itemCapacity", u.itemCapacity, false);
            fieldBool("flying", u.flying, false);
            fieldBool("isBuilding", isB, false);
            fieldFloat("payloadCapacity", u.payloadCapacity, false);
            fieldFloat("buildSpeed", u.buildSpeed, false);
            fieldFloat("buildRange", u.buildRange, false);
            fieldFloat("rotateSpeed", u.rotateSpeed, false);
            fieldFloat("accel", u.accel, false);
            fieldFloat("drag", u.drag, false);
            fieldFloat("armor", u.armor, false);
            fieldFloat("engineSize", u.engineSize, false);
            fieldFloat("engineOffset", u.engineOffset, false);
            fieldFloat("clipSize", u.clipSize, false);
            fieldNum("defaultCommand", u.defaultCommand == null ? -1 : u.defaultCommand.id, true);
            objEnd(i + 1 == units.size);
        }
        arrEnd(false);

        key("blocks");
        arrStart();
        var blocks = Vars.content.blocks();
        for (int i = 0; i < blocks.size; i++) {
            Block b = blocks.get(i);
            int buildVersion = 0;
            String buildClass = "Building";
            try {
                var build = b.newBuilding();
                if (build != null) {
                    buildClass = build.getClass().getSimpleName();
                    buildVersion = Byte.toUnsignedInt(build.version());
                }
            } catch (Throwable ignored) {}
            objStart();
            fieldNum("id", b.id, false);
            field("name", b.name, false);
            fieldNum("size", b.size, false);
            fieldNum("health", b.health, false);
            fieldBool("hasItems", b.hasItems, false);
            fieldBool("hasLiquids", b.hasLiquids, false);
            fieldBool("hasPower", b.hasPower, false);
            fieldNum("itemCapacity", b.itemCapacity, false);
            fieldFloat("liquidCapacity", b.liquidCapacity, false);
            fieldFloat("buildCostMultiplier", b.buildCostMultiplier, false);
            fieldFloat("buildTime", b.buildTime, false);
            fieldBool("solid", b.solid, false);
            fieldBool("rotate", b.rotate, false);
            fieldBool("update", b.update, false);
            fieldBool("destructible", b.destructible, false);
            fieldBool("synthetic", b.synthetic(), false);
            fieldBool("targetable", b.targetable, false);
            field("buildClass", buildClass, false);
            fieldNum("buildVersion", buildVersion, true);
            objEnd(i + 1 == blocks.size);
        }
        arrEnd(false);

        key("unit_commands");
        arrStart();
        var ucmds = Vars.content.unitCommands();
        for (int i = 0; i < ucmds.size; i++) {
            UnitCommand uc = ucmds.get(i);
            objStart();
            fieldNum("id", uc.id, false);
            field("name", uc.name, true);
            objEnd(i + 1 == ucmds.size);
        }
        arrEnd(false);

        key("unit_stances");
        arrStart();
        var usts = Vars.content.unitStances();
        for (int i = 0; i < usts.size; i++) {
            UnitStance us = usts.get(i);
            objStart();
            fieldNum("id", us.id, false);
            field("name", us.name, false);
            fieldBool("toggle", us.toggle, true);
            objEnd(i + 1 == usts.size);
        }
        arrEnd(false);
    }

    private static Object[] valuesOf(String className) {
        try {
            Class<?> cls = Class.forName(className);
            Method values = cls.getMethod("values");
            return (Object[]) values.invoke(null);
        } catch (Throwable t) {
            return new Object[0];
        }
    }

    private static void emitEnumArray(String keyName, Object[] values) throws Exception {
        key(keyName);
        arrStart();
        for (int i = 0; i < values.length; i++) {
            Object acc = values[i];
            boolean isObj = false;
            try {
                Field f = acc.getClass().getField("isObj");
                isObj = f.getBoolean(acc);
            } catch (Throwable ignored) {}
            objStart();
            fieldNum("ordinal", i, false);
            field("name", String.valueOf(acc), false);
            field("key", String.valueOf(acc), false);
            fieldBool("isObj", isObj, true);
            objEnd(i + 1 == values.length);
        }
        arrEnd(true);
    }

    private static Integer optionalIntField(Class<?> cls, String name) {
        try {
            Field f = cls.getField(name);
            return f.getInt(null);
        } catch (Throwable t) {
            return null;
        }
    }

    private static boolean hasDeclared(Class<?> cls, String name) {
        for (Method m : cls.getDeclaredMethods()) {
            if (m.getName().equals(name)) return true;
        }
        return false;
    }

    private static List<Field> publicInstanceFields(Class<?> cls) {
        List<Field> out = new ArrayList<>();
        for (Field f : cls.getFields()) {
            if (!Modifier.isStatic(f.getModifiers())) out.add(f);
        }
        return out;
    }

    private static String uncapitalize(String s) {
        if (s.isEmpty()) return s;
        return Character.toLowerCase(s.charAt(0)) + s.substring(1);
    }

    private static String sha256Bytes(byte[] data) throws Exception {
        MessageDigest md = MessageDigest.getInstance("SHA-256");
        byte[] d = md.digest(data);
        StringBuilder sb = new StringBuilder();
        for (byte b : d) sb.append(String.format("%02x", b));
        return sb.toString();
    }

    private static void objStart() { OUT.append("{"); }
    private static void objEnd(boolean last) { OUT.append("}"); if (!last) comma(); }
    private static void arrStart() { OUT.append("["); }
    private static void arrEnd(boolean last) { OUT.append("]"); if (!last) comma(); }
    private static void comma() { OUT.append(","); }
    private static void key(String k) { OUT.append("\"").append(esc(k)).append("\":"); }
    private static void raw(String s) { OUT.append(s); }
    private static void field(String k, String v, boolean last) {
        key(k); OUT.append("\"").append(esc(v)).append("\""); if (!last) comma();
    }
    private static void fieldNum(String k, int v, boolean last) {
        key(k); OUT.append(v); if (!last) comma();
    }
    private static void fieldFloat(String k, float v, boolean last) {
        key(k); OUT.append(v); if (!last) comma();
    }
    private static void fieldBool(String k, boolean v, boolean last) {
        key(k); OUT.append(v ? "true" : "false"); if (!last) comma();
    }
    private static String esc(String s) {
        if (s == null) return "";
        return s.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n").replace("\r", "");
    }
}
