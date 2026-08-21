import mindustry.Vars;
import arc.Core;
import arc.Settings;
import mindustry.core.ContentLoader;
import mindustry.gen.UnitSpawnCallPacket;
import mindustry.gen.CommandBuildingCallPacket;
import mindustry.gen.CommandUnitsCallPacket;
import mindustry.gen.SetUnitCommandCallPacket;
import mindustry.gen.SetUnitStanceCallPacket;
import mindustry.gen.PickedUnitPayloadCallPacket;
import mindustry.gen.PickedBuildPayloadCallPacket;
import mindustry.gen.PayloadDroppedCallPacket;
import mindustry.gen.UnitEnteredPayloadCallPacket;
import mindustry.net.Net;
import mindustry.content.Blocks;
import mindustry.game.Rules;
import mindustry.ai.UnitCommand;
import mindustry.ai.UnitStance;
import mindustry.type.UnitType;
import mindustry.world.Block;

/** Prints generated packet IDs from the supplied server/desktop JAR. */
public final class InspectPacketIds {
    public static void main(String[] args) {
        Vars.content = new ContentLoader();
        Core.settings = new Settings();
        Vars.content.createBaseContent();
        Vars.content.init();
        System.out.printf(
            "UnitSpawnCallPacket=%d%n",
            Byte.toUnsignedInt(Net.getPacketId(new UnitSpawnCallPacket()))
        );
        System.out.printf(
            "CommandBuildingCallPacket=%d%n",
            Byte.toUnsignedInt(Net.getPacketId(new CommandBuildingCallPacket()))
        );
        System.out.printf(
            "CommandUnitsCallPacket=%d%n",
            Byte.toUnsignedInt(Net.getPacketId(new CommandUnitsCallPacket()))
        );
        System.out.printf(
            "SetUnitCommandCallPacket=%d%n",
            Byte.toUnsignedInt(Net.getPacketId(new SetUnitCommandCallPacket()))
        );
        System.out.printf(
            "SetUnitStanceCallPacket=%d%n",
            Byte.toUnsignedInt(Net.getPacketId(new SetUnitStanceCallPacket()))
        );
        System.out.printf("PickedUnitPayloadCallPacket=%d%n", Byte.toUnsignedInt(Net.getPacketId(new PickedUnitPayloadCallPacket())));
        System.out.printf("PickedBuildPayloadCallPacket=%d%n", Byte.toUnsignedInt(Net.getPacketId(new PickedBuildPayloadCallPacket())));
        System.out.printf("PayloadDroppedCallPacket=%d%n", Byte.toUnsignedInt(Net.getPacketId(new PayloadDroppedCallPacket())));
        System.out.printf("UnitEnteredPayloadCallPacket=%d%n", Byte.toUnsignedInt(Net.getPacketId(new UnitEnteredPayloadCallPacket())));
        System.out.printf(
            "reconstructors liquid=%s/%s item=%s/%s/%s/%s%n",
            Blocks.exponentialReconstructor.liquidCapacity,
            Blocks.tetrativeReconstructor.liquidCapacity,
            Blocks.additiveReconstructor.itemCapacity,
            Blocks.multiplicativeReconstructor.itemCapacity,
            Blocks.exponentialReconstructor.itemCapacity,
            Blocks.tetrativeReconstructor.itemCapacity
        );
        Rules rules = new Rules();
        System.out.printf(
            "unitCap default=%d variable=%s disabled=%s cores=%d/%d/%d commands=%d%n",
            rules.unitCap,
            rules.unitCapVariable,
            rules.disableUnitCap,
            Blocks.coreShard.unitCapModifier,
            Blocks.coreFoundation.unitCapModifier,
            Blocks.coreNucleus.unitCapModifier,
            Vars.content.unitCommands().size
        );
        for (UnitCommand command : Vars.content.unitCommands()) {
            System.out.printf("command %d=%s%n", command.id, command.name);
        }
        for (UnitStance stance : Vars.content.unitStances()) {
            System.out.printf(
                "stance %d=%s toggle=%s%n", stance.id, stance.name, stance.toggle);
        }
        for (UnitType unit : Vars.content.units()) {
            if (unit.id < 35) {
                System.out.printf(
                    "unit %d=%s defaultCommand=%d commands=%s stances=%s build=%s/%s hit=%s payload=%s pickupUnits=%s allowedPayload=%s%n",
                    unit.id,
                    unit.name,
                    unit.defaultCommand == null ? -1 : unit.defaultCommand.id,
                    unit.commands.toString(", ", command -> Integer.toString(command.id)),
                    unit.stances.toString(", ", stance -> Integer.toString(stance.id)),
                    unit.buildSpeed,
                    unit.buildRange,
                    unit.hitSize,
                    unit.payloadCapacity,
                    unit.pickupUnits,
                    unit.allowedInPayloads
                );
            }
        }
        for (Block block : Vars.content.blocks()) {
            if (block.canPickup) {
                var build = block.newBuilding();
                System.out.printf("block %d=%s size=%d build=%s version=%d items=%s power=%s liquids=%s%n",
                    block.id, block.name, block.size, build.getClass().getSimpleName(),
                    Byte.toUnsignedInt(build.version()), block.hasItems, block.hasPower,
                    block.hasLiquids);
            }
        }
    }
}
