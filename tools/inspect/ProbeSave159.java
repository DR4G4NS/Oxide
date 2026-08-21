import arc.Core;
import arc.Settings;
import mindustry.Vars;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.io.SaveIO;
import mindustry.io.SaveMeta;
import mindustry.io.SaveReadState;
import mindustry.io.SaveVersion;
import mindustry.io.versions.Save12;
import mindustry.world.Tile;
import mindustry.world.WorldContext;

import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.zip.InflaterInputStream;

/**
 * Build 159.7 JAR-backed behavioral probes for Save12/Save13 patch regions and
 * MSAV header/version/meta readability (not a full world deserialize).
 * Usage:
 *   ProbeSave159 read-save12-patches <12-byte-patch-file>
 *   ProbeSave159 reject-save13-patches-on-save12 <8-byte-save13-patch-file>
 *   ProbeSave159 read-msav-meta <path-to.msav>
 */
public final class ProbeSave159 {
    private static final class EmptyWorldContext implements WorldContext {
        @Override
        public Tile tile(int index) {
            throw new UnsupportedOperationException("tile");
        }

        @Override
        public void resize(int width, int height) {}

        @Override
        public Tile create(int x, int y, int floorID, int overlayID, int wallID) {
            throw new UnsupportedOperationException("create");
        }

        @Override
        public boolean isGenerating() {
            return true;
        }

        @Override
        public void begin() {}

        @Override
        public void end() {}
    }

    private static WorldContext emptyContext() {
        return new EmptyWorldContext();
    }

    private static void initHeadless() {
        arc.util.Log.logger = (level, text) -> {};
        Vars.content = new ContentLoader();
        Vars.state = new GameState();
        Core.settings = new Settings();
        Vars.content.createBaseContent();
        Vars.content.init();
    }

    private static Save12 save12() {
        return (Save12) SaveIO.versionArray.get(11);
    }

    private static void readSave12Patches(byte[] patchBytes) throws IOException {
        Save12 s12 = save12();
        DataInputStream in = new DataInputStream(new ByteArrayInputStream(patchBytes));
        s12.readDataPatches(in, new SaveReadState(emptyContext()));
    }

    public static void main(String[] args) throws Exception {
        if (args.length < 1) {
            System.err.println(
                    "Usage: ProbeSave159 <read-save12-patches|reject-save13-patches-on-save12|read-msav-meta> ...");
            System.exit(2);
        }
        initHeadless();
        switch (args[0]) {
            case "read-save12-patches" -> {
                byte[] patch = Files.readAllBytes(Path.of(args[1]));
                if (patch.length != 12) {
                    throw new IOException("expected 12-byte Save12 empty patch region, got " + patch.length);
                }
                readSave12Patches(patch);
                System.out.println("OK read-save12-patches");
            }
            case "reject-save13-patches-on-save12" -> {
                byte[] patch = Files.readAllBytes(Path.of(args[1]));
                if (patch.length != 8) {
                    throw new IOException("expected 8-byte Save13 empty patch region, got " + patch.length);
                }
                try {
                    readSave12Patches(patch);
                    System.err.println("Save12 reader accepted Save13 patch format");
                    System.exit(1);
                } catch (IOException | RuntimeException expected) {
                    System.out.println("OK reject-save13-patches-on-save12");
                }
            }
            case "read-msav-meta" -> {
                Path path = Path.of(args[1]);
                try (InputStream raw = Files.newInputStream(path);
                        InflaterInputStream inflated = new InflaterInputStream(raw);
                        DataInputStream stream = new DataInputStream(inflated)) {
                    SaveIO.readHeader(stream);
                    int version = stream.readInt();
                    SaveVersion ver = SaveIO.versions.get(version);
                    if (ver == null) {
                        throw new IOException("unknown save version " + version);
                    }
                    SaveMeta meta = ver.getMeta(stream);
                    System.out.println(
                            "OK read-msav-meta " + path.getFileName() + " version=" + meta.version);
                }
            }
            default -> {
                System.err.println("unknown command: " + args[0]);
                System.exit(2);
            }
        }
    }
}
