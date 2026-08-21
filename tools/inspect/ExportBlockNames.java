import mindustry.Vars;
import mindustry.core.ContentLoader;

/** Prints the authoritative numeric block ID/name registry from the local server JAR. */
public final class ExportBlockNames {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Vars.content.createBaseContent();
        Vars.content.blocks().each(block -> System.out.println(block.id + "\t" + block.name));
    }
}
