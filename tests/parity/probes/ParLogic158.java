import java.io.InputStream;
import java.util.Properties;
import java.util.TreeMap;

import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.logic.GlobalVars;
import mindustry.logic.LAssembler;
import mindustry.logic.LExecutor;
import mindustry.logic.LVar;

/**
 * P0-00 differential probe: deterministic LExecutor run on desktop.jar 158.1.
 *
 * Standalone probe (no game loop, no world, no client): assembles a small
 * integer-only logic program and drives {@code LExecutor.runOnce()} exactly
 * once per game tick for {@value #TICKS} ticks, then emits the normalized
 * executor state as a single JSON object on stdout.
 *
 * The program is chosen so every instruction is deterministic and
 * world-independent (set/op/print/jump/stop), the printed text never reaches
 * the 400-char text buffer limit, and only `op` is used for arithmetic:
 * desktop 158.1 compiles `set a b + c` into a copy of a *variable named
 * "b_+_c"* (verified empirically with the 158.1 jar), so expression-style
 * `set` is deliberately avoided. The 0-based jump targets are verified
 * against the same jar:
 *
 *   op add n n 1          idx 0
 *   print "n="            idx 1
 *   print n               idx 2
 *   op add m m 2          idx 3
 *   jump 0 lessThan n 50  idx 4   (back to idx 0 until n == 50)
 *   stop                  idx 5   (halts forever afterwards)
 *
 * After 601 runOnce calls the program is parked on `stop`: n=50, m=100,
 * text="n=1n=2...n=50" (191 chars), counter parked on the stop instruction.
 * The Rust side replays the identical source and must reproduce the same
 * normalized output field by field.
 *
 * Version gate: the probe refuses to run unless the classpath
 * version.properties reports the official 158.1 build, and emits
 * "probe_version" so the Rust fixture loader can double check.
 */
public final class ParLogic158 {
    /** Number of game ticks simulated; one runOnce per tick. */
    static final int TICKS = 601;

    /** The exact logic source replayed by the Rust side. */
    static final String PROGRAM =
        "op add n n 1\n"
        + "print \"n=\"\n"
        + "print n\n"
        + "op add m m 2\n"
        + "jump 0 lessThan n 50\n"
        + "stop";

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParLogic158: refusing to run: classpath version.properties reports '" + version
                + "', expected official 158.1");
            System.exit(2);
        }

        // LAssembler.var resolves builtin variables through Vars.logicVars
        // (GlobalVars), which the game creates during Vars.init(). That path
        // needs the full Arc app, so the probe constructs the two pieces it
        // actually uses: base content + an (empty) global var table — enough
        // for LAssembler.var() and LExecutor.load(). GlobalVars.init() is
        // deliberately skipped: it needs Core.files (logicids.dat) and only
        // populates content/lookup entries this probe's program never uses.
        // No game loop, world or client is started.
        Vars.headless = true;
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.logicVars = new GlobalVars();

        LExecutor exec = new LExecutor();
        exec.load(LAssembler.assemble(PROGRAM, true));
        if (!exec.initialized()) {
            System.err.println("ParLogic158: executor did not initialize");
            System.exit(3);
        }

        for (int tick = 0; tick < TICKS; tick++) {
            exec.runOnce();
        }

        // Normalized dump: user variables only (skip builtin '@' vars), sorted
        // by name so the JSON is stable regardless of var allocation order.
        TreeMap<String, LVar> userVars = new TreeMap<>();
        if (exec.vars != null) {
            for (LVar v : exec.vars) {
                if (v != null && !v.name.startsWith("@")) {
                    userVars.put(v.name, v);
                }
            }
        }

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("logic-601")).append(",\n");
        json.append("  \"tick\": ").append(TICKS).append(",\n");
        json.append("  \"executions\": ").append(TICKS).append(",\n");
        json.append("  \"program\": ").append(jsonString(PROGRAM)).append(",\n");
        json.append("  \"counter\": ").append((long) exec.counter.numval).append(",\n");
        json.append("  \"text\": ").append(jsonString(exec.textBuffer.toString())).append(",\n");
        json.append("  \"vars\": {");
        boolean first = true;
        for (var entry : userVars.entrySet()) {
            if (!first) json.append(",");
            first = false;
            LVar v = entry.getValue();
            json.append("\n    ").append(jsonString(entry.getKey())).append(": {\"isobj\": ")
                .append(v.isobj).append(", \"num\": ").append(v.numval).append("}");
        }
        json.append("\n  }\n}\n");
        System.out.print(json);
    }

    /** Reads version.properties from the classpath (the desktop.jar root). */
    static String classpathVersion() throws Exception {
        try (InputStream in = ParLogic158.class.getResourceAsStream("/version.properties")) {
            if (in == null) return "missing";
            Properties p = new Properties();
            p.load(in);
            String build = p.getProperty("build", "missing");
            String type = p.getProperty("type", "missing");
            return type + " " + build;
        }
    }

    static String jsonString(String value) {
        StringBuilder out = new StringBuilder("\"");
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            switch (c) {
                case '"' -> out.append("\\\"");
                case '\\' -> out.append("\\\\");
                case '\n' -> out.append("\\n");
                case '\r' -> out.append("\\r");
                case '\t' -> out.append("\\t");
                default -> {
                    if (c < 0x20) {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
                }
            }
        }
        return out.append("\"").toString();
    }
}
