import java.io.InputStream;
import java.util.Properties;

import arc.Core;
import arc.backend.sdl.SdlFiles;
import arc.math.geom.Vec2;
import mindustry.Vars;
import mindustry.ai.UnitCommand;
import mindustry.ai.UnitStance;
import mindustry.ai.types.CommandAI;
import mindustry.content.UnitTypes;
import mindustry.core.ContentLoader;
import mindustry.core.GameState;
import mindustry.core.World;
import mindustry.game.EventType;
import mindustry.game.Team;
import mindustry.gen.Groups;
import mindustry.gen.Player;
import mindustry.gen.Unit;
import mindustry.logic.GlobalVars;
import mindustry.world.Tiles;

/**
 * P0-05 differential probe: player <-> unit possession lifecycle on
 * desktop.jar 158.1, driving the exact API InputHandler.unitControl uses
 * after its gate — `player.unit(unit)` / `player.clearUnit()`
 * (PlayerComp.java:281-319).
 *
 * Minimal headless world (no game loop, no client, no physics) following
 * ParCommand158. `state.rules.waves = true` makes every team except the
 * default (sharded) an AI team (Team.isAI, Team.java:112-114), so:
 *   - sharded mega/poly  -> CommandAI controllers (UnitType.java:281)
 *   - crux flare         -> plain wave AI controller
 *
 * Scenarios (state read directly off the units and the player):
 *   A setup     — mega's CommandAI gets an active target, one queued entry
 *                 and a pursueTarget stance; ids read via UnitCommand.id
 *                 (content order = move 0, repair 1, rebuild 2, assist 3,
 *                 mine 4, payload family 5-9, UnitCommand.java:74-113).
 *   B possess   — player.unit(mega): controller becomes the Player and
 *                 player.lastCommand saves the pre-possession command
 *                 (PlayerComp.java:290-292).
 *   C release   — player.clearUnit(): resetController + command restore
 *                 (PlayerComp.java:294-300); the fresh CommandAI's queue,
 *                 targets and stances are read to prove what round-trips.
 *   D switch    — re-possess mega, then player.unit(poly): records what
 *                 command mega's fresh CommandAI ends up with (the switch
 *                 ordering truth) and poly's saved command.
 *   E release B — poly's own restore.
 *   F team      — player.team(blue) mid-possession: does the possessed
 *                 unit's team follow (PlayerComp.java:266-271)?
 *   G wave AI   — possessing a non-CommandAI unit: no lastCommand save,
 *                 and the released controller follows the adopted team.
 *   H same type — two polys with distinct supported commands expose the
 *                 incoming-save-before-old-restore ordering, then B -> A.
 *   I team      — release without restoring the player's old team.
 *   J teardown  — Player.remove() and possessed-unit death/update.
 *
 * Version gate: refuses to run unless the classpath version.properties
 * reports the official 158.1 build.
 */
public final class ParPossess158 {

    public static void main(String[] args) throws Exception {
        String version = classpathVersion();
        String build = version.substring(version.indexOf(' ') + 1);
        if (!version.startsWith("official ") || !"158.1".equals(build)) {
            System.err.println("ParPossess158: refusing to run: classpath version.properties reports '" + version
                + "', expected official 158.1");
            System.exit(2);
        }

        // Shared headless setup.
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
        Vars.state.rules.waves = true; // only sharded is player-commandable
        Vars.state.rules.disableUnitCap = true;
        Vars.world = new World();
        Vars.world.tiles = new Tiles(16, 16);
        Vars.world.tiles.fill();
        Vars.state.teams.updateTeamStats();

        // No update loop is ever run: the probe only drives controller
        // transitions, so grounded units are safe.
        Unit mega = UnitTypes.mega.create(Team.sharded);
        mega.set(80f, 80f);
        mega.add();
        Unit poly = UnitTypes.poly.create(Team.sharded);
        poly.set(120f, 80f);
        poly.add();
        Unit flare = UnitTypes.flare.create(Team.crux);
        flare.set(160f, 80f);
        flare.add();
        Unit polyA = UnitTypes.poly.create(Team.sharded);
        polyA.set(80f, 120f);
        polyA.add();
        Unit polyB = UnitTypes.poly.create(Team.sharded);
        polyB.set(120f, 120f);
        polyB.add();
        Vars.state.teams.updateTeamStats();

        if (!(mega.controller() instanceof CommandAI) || !(poly.controller() instanceof CommandAI)) {
            System.err.println("ParPossess158: sharded mega/poly did not receive CommandAI controllers");
            System.exit(3);
        }
        if (flare.controller() instanceof CommandAI) {
            System.err.println("ParPossess158: crux flare unexpectedly received a CommandAI controller");
            System.exit(4);
        }

        Player p = Player.create();
        p.team(Team.sharded);

        // --- A: baseline + CommandAI state on mega ------------------------
        CommandAI ai = (CommandAI) mega.controller();
        int aInitial = ai.command.id;
        ai.commandQueue(new Vec2(100f, 100f)); // promoted to active
        ai.commandQueue(new Vec2(150f, 150f)); // queued
        ai.setStance(UnitStance.pursueTarget);
        boolean aHasCommand = ai.hasCommand();
        int aQueue = ai.commandQueue.size;
        boolean aStance = ai.hasStance(UnitStance.pursueTarget);

        // --- B: possess mega ------------------------------------------------
        p.unit(mega);
        boolean possessCtrlPlayer = mega.isPlayer();
        int possessLastCommand = cmdId(p.lastCommand);

        // --- C: release mega --------------------------------------------------
        p.clearUnit();
        boolean releaseCtrlAi = mega.controller() instanceof CommandAI;
        CommandAI ai2 = (CommandAI) mega.controller();
        int releaseCommand = cmdId(ai2.command);
        int releaseQueue = ai2.commandQueue.size;
        boolean releaseHasCommand = ai2.hasCommand();
        int releaseTarget = targetKind(ai2);
        int releaseStances = stanceMask(ai2);
        boolean releaseStancesEmpty = ai2.stances.isEmpty();
        boolean releaseStanceGone = !ai2.hasStance(UnitStance.pursueTarget);
        boolean releaseLogic = ai2.isLogicControllable();

        // --- D: switch mega -> poly -------------------------------------------
        p.unit(mega);
        int bInitial = ((CommandAI) poly.controller()).command.id;
        p.unit(poly);
        boolean switchCtrlPlayer = poly.isPlayer();
        boolean switchMegaIsAi = mega.controller() instanceof CommandAI;
        int switchMegaCommand = switchMegaIsAi ? ((CommandAI) mega.controller()).command.id : -2;
        int switchPlayerLast = cmdId(p.lastCommand);

        // --- E: release poly ---------------------------------------------------
        p.clearUnit();
        boolean bReleaseCtrlAi = poly.controller() instanceof CommandAI;
        int bReleaseCommand = bReleaseCtrlAi ? ((CommandAI) poly.controller()).command.id : -2;
        int bReleaseQueue = bReleaseCtrlAi ? ((CommandAI) poly.controller()).commandQueue.size : -1;

        // --- F: team change mid-possession --------------------------------------
        p.unit(mega);
        p.team(Team.blue);
        boolean teamPropagated = mega.team() == Team.blue;
        boolean teamSurvives = mega.isPlayer();
        p.team(Team.sharded); // restore before release
        p.clearUnit();

        // --- G: possessing a non-CommandAI unit --------------------------------
        boolean waveCtrlAiBefore = flare.controller() instanceof CommandAI;
        int lastBefore = cmdId(p.lastCommand);
        p.unit(flare);
        boolean wavePossessCtrlPlayer = flare.isPlayer();
        // PlayerComp.unit sets unit.team(team) (line 304) — observable when
        // the same-team InputHandler gate is bypassed (this probe drives
        // PlayerComp directly): the crux flare becomes sharded.
        int wavePossessTeam = flare.team().id;
        boolean waveLastCommandUnchanged = cmdId(p.lastCommand) == lastBefore;
        p.clearUnit();
        boolean waveReleaseCtrlAi = flare.controller() instanceof CommandAI;
        int waveReleaseCommand = commandId(flare);
        int waveReleaseQueue = queueSize(flare);
        int waveReleaseTarget = targetKind(flare);
        int waveReleaseStances = stanceMask(flare);
        int waveReleaseTeam = flare.team().id;

        // --- H: same-type A -> B -> A, distinct supported commands ------------
        CommandAI polyAAi = (CommandAI) polyA.controller();
        CommandAI polyBAi = (CommandAI) polyB.controller();
        polyAAi.command(UnitCommand.moveCommand);
        polyBAi.command(UnitCommand.rebuildCommand);
        int sameInitialACommand = polyAAi.command.id;
        int sameInitialBCommand = polyBAi.command.id;
        boolean sameMoveSupported = polyA.type().commands.contains(UnitCommand.moveCommand);
        boolean sameRebuildSupported = polyA.type().commands.contains(UnitCommand.rebuildCommand);
        polyAAi.commandQueue(new Vec2(88f, 128f));
        polyAAi.commandQueue(new Vec2(96f, 136f));
        polyAAi.setStance(UnitStance.pursueTarget);
        polyBAi.commandQueue(new Vec2(128f, 128f));
        polyBAi.commandQueue(new Vec2(136f, 136f));
        polyBAi.setStance(UnitStance.holdFire);

        Player same = Player.create();
        same.team(Team.sharded);
        same.unit(polyA);
        same.unit(polyB);
        int sameSwitchAController = controllerKind(polyA);
        int sameSwitchACommand = commandId(polyA);
        int sameSwitchAQueue = queueSize(polyA);
        int sameSwitchATarget = targetKind(polyA);
        int sameSwitchAStances = stanceMask(polyA);
        int sameSwitchATeam = polyA.team().id;
        int sameSwitchBController = controllerKind(polyB);
        int sameSwitchBTeam = polyB.team().id;
        int sameSwitchLastCommand = cmdId(same.lastCommand);

        same.unit(polyA);
        int sameBackAController = controllerKind(polyA);
        int sameBackATeam = polyA.team().id;
        int sameBackBController = controllerKind(polyB);
        int sameBackBCommand = commandId(polyB);
        int sameBackBQueue = queueSize(polyB);
        int sameBackBTarget = targetKind(polyB);
        int sameBackBStances = stanceMask(polyB);
        int sameBackBTeam = polyB.team().id;
        int sameBackLastCommand = cmdId(same.lastCommand);
        same.clearUnit();

        // --- I: team change survives release and changes the default AI --------
        Unit teamUnit = UnitTypes.poly.create(Team.sharded);
        teamUnit.set(160f, 120f);
        teamUnit.add();
        ((CommandAI) teamUnit.controller()).command(UnitCommand.moveCommand);
        Player teamPlayer = Player.create();
        teamPlayer.team(Team.sharded);
        teamPlayer.unit(teamUnit);
        teamPlayer.team(Team.blue);
        teamPlayer.clearUnit();
        int teamReleaseController = controllerKind(teamUnit);
        int teamReleaseCommand = commandId(teamUnit);
        int teamReleaseQueue = queueSize(teamUnit);
        int teamReleaseTarget = targetKind(teamUnit);
        int teamReleaseStances = stanceMask(teamUnit);
        int teamReleaseTeam = teamUnit.team().id;
        int teamReleaseLastCommand = cmdId(teamPlayer.lastCommand);

        // --- J1: disconnect (Player.remove -> clearUnit) -----------------------
        Unit disconnectUnit = UnitTypes.poly.create(Team.sharded);
        disconnectUnit.set(80f, 160f);
        disconnectUnit.add();
        ((CommandAI) disconnectUnit.controller()).command(UnitCommand.moveCommand);
        Player disconnectPlayer = Player.create();
        disconnectPlayer.team(Team.sharded);
        disconnectPlayer.add();
        disconnectPlayer.unit(disconnectUnit);
        disconnectPlayer.remove();
        boolean disconnectPlayerUnitNull = disconnectPlayer.unit() == null;
        int disconnectController = controllerKind(disconnectUnit);
        int disconnectCommand = commandId(disconnectUnit);
        int disconnectQueue = queueSize(disconnectUnit);
        int disconnectTarget = targetKind(disconnectUnit);
        int disconnectStances = stanceMask(disconnectUnit);
        int disconnectTeam = disconnectUnit.team().id;
        int disconnectLastCommand = cmdId(disconnectPlayer.lastCommand);

        // --- J2: death, then Player.update notices invalid unit ----------------
        Unit deathUnit = UnitTypes.poly.create(Team.sharded);
        deathUnit.set(120f, 160f);
        deathUnit.add();
        ((CommandAI) deathUnit.controller()).command(UnitCommand.rebuildCommand);
        Player deathPlayer = Player.create();
        deathPlayer.team(Team.sharded);
        deathPlayer.add();
        deathPlayer.unit(deathUnit);
        // Unit.kill() reaches audio/Fx singletons that are intentionally not
        // booted in this minimal probe. The observable PlayerComp path starts
        // at the same post-death state: health <= 0 and no longer added.
        deathUnit.health(0f);
        deathUnit.remove();
        deathPlayer.update();
        boolean deathPlayerUnitNull = deathPlayer.unit() == null;
        int deathController = controllerKind(deathUnit);
        int deathCommand = commandId(deathUnit);
        int deathQueue = queueSize(deathUnit);
        int deathTarget = targetKind(deathUnit);
        int deathStances = stanceMask(deathUnit);
        int deathTeam = deathUnit.team().id;
        int deathLastCommand = cmdId(deathPlayer.lastCommand);
        deathPlayer.remove();

        // Silence the unused-import style warnings for EventType (documented
        // dependency: player.unit fires UnitChangeEvent through static
        // Events, which needs no listeners headless).
        Class<?> fired = EventType.UnitChangeEvent.class;

        StringBuilder json = new StringBuilder();
        json.append("{\n");
        json.append("  \"probe_version\": ").append(jsonString("158.1")).append(",\n");
        json.append("  \"probe_name\": ").append(jsonString("possession")).append(",\n");
        json.append("  \"tick\": 0,\n");
        json.append("  \"event_fired\": ").append(jsonString(fired.getSimpleName())).append(",\n");
        json.append("  \"a_initial_command\": ").append(aInitial).append(",\n");
        json.append("  \"a_setup_has_command\": ").append(aHasCommand).append(",\n");
        json.append("  \"a_setup_queue\": ").append(aQueue).append(",\n");
        json.append("  \"a_setup_stance\": ").append(aStance).append(",\n");
        json.append("  \"possess_ctrl_player\": ").append(possessCtrlPlayer).append(",\n");
        json.append("  \"possess_last_command\": ").append(possessLastCommand).append(",\n");
        json.append("  \"release_ctrl_command_ai\": ").append(releaseCtrlAi).append(",\n");
        json.append("  \"release_command\": ").append(releaseCommand).append(",\n");
        json.append("  \"release_queue\": ").append(releaseQueue).append(",\n");
        json.append("  \"release_has_command\": ").append(releaseHasCommand).append(",\n");
        json.append("  \"release_target\": ").append(releaseTarget).append(",\n");
        json.append("  \"release_stances\": ").append(releaseStances).append(",\n");
        json.append("  \"release_stances_empty\": ").append(releaseStancesEmpty).append(",\n");
        json.append("  \"release_stance_gone\": ").append(releaseStanceGone).append(",\n");
        json.append("  \"release_logic_controllable\": ").append(releaseLogic).append(",\n");
        json.append("  \"b_initial_command\": ").append(bInitial).append(",\n");
        json.append("  \"switch_ctrl_player\": ").append(switchCtrlPlayer).append(",\n");
        json.append("  \"switch_mega_is_command_ai\": ").append(switchMegaIsAi).append(",\n");
        json.append("  \"switch_mega_command\": ").append(switchMegaCommand).append(",\n");
        json.append("  \"switch_player_last_command\": ").append(switchPlayerLast).append(",\n");
        json.append("  \"b_release_ctrl_command_ai\": ").append(bReleaseCtrlAi).append(",\n");
        json.append("  \"b_release_command\": ").append(bReleaseCommand).append(",\n");
        json.append("  \"b_release_queue\": ").append(bReleaseQueue).append(",\n");
        json.append("  \"team_propagated\": ").append(teamPropagated).append(",\n");
        json.append("  \"team_possession_survives\": ").append(teamSurvives).append(",\n");
        json.append("  \"wave_ctrl_command_ai_before\": ").append(waveCtrlAiBefore).append(",\n");
        json.append("  \"wave_possess_ctrl_player\": ").append(wavePossessCtrlPlayer).append(",\n");
        json.append("  \"wave_possess_team\": ").append(wavePossessTeam).append(",\n");
        json.append("  \"wave_last_command_unchanged\": ").append(waveLastCommandUnchanged).append(",\n");
        json.append("  \"wave_release_ctrl_command_ai\": ").append(waveReleaseCtrlAi).append(",\n");
        json.append("  \"wave_release_command\": ").append(waveReleaseCommand).append(",\n");
        json.append("  \"wave_release_queue\": ").append(waveReleaseQueue).append(",\n");
        json.append("  \"wave_release_target\": ").append(waveReleaseTarget).append(",\n");
        json.append("  \"wave_release_stances\": ").append(waveReleaseStances).append(",\n");
        json.append("  \"wave_release_team\": ").append(waveReleaseTeam).append(",\n");
        json.append("  \"same_initial_a_command\": ").append(sameInitialACommand).append(",\n");
        json.append("  \"same_initial_b_command\": ").append(sameInitialBCommand).append(",\n");
        json.append("  \"same_move_supported\": ").append(sameMoveSupported).append(",\n");
        json.append("  \"same_rebuild_supported\": ").append(sameRebuildSupported).append(",\n");
        json.append("  \"same_switch_a_controller\": ").append(sameSwitchAController).append(",\n");
        json.append("  \"same_switch_a_command\": ").append(sameSwitchACommand).append(",\n");
        json.append("  \"same_switch_a_queue\": ").append(sameSwitchAQueue).append(",\n");
        json.append("  \"same_switch_a_target\": ").append(sameSwitchATarget).append(",\n");
        json.append("  \"same_switch_a_stances\": ").append(sameSwitchAStances).append(",\n");
        json.append("  \"same_switch_a_team\": ").append(sameSwitchATeam).append(",\n");
        json.append("  \"same_switch_b_controller\": ").append(sameSwitchBController).append(",\n");
        json.append("  \"same_switch_b_team\": ").append(sameSwitchBTeam).append(",\n");
        json.append("  \"same_switch_last_command\": ").append(sameSwitchLastCommand).append(",\n");
        json.append("  \"same_back_a_controller\": ").append(sameBackAController).append(",\n");
        json.append("  \"same_back_a_team\": ").append(sameBackATeam).append(",\n");
        json.append("  \"same_back_b_controller\": ").append(sameBackBController).append(",\n");
        json.append("  \"same_back_b_command\": ").append(sameBackBCommand).append(",\n");
        json.append("  \"same_back_b_queue\": ").append(sameBackBQueue).append(",\n");
        json.append("  \"same_back_b_target\": ").append(sameBackBTarget).append(",\n");
        json.append("  \"same_back_b_stances\": ").append(sameBackBStances).append(",\n");
        json.append("  \"same_back_b_team\": ").append(sameBackBTeam).append(",\n");
        json.append("  \"same_back_last_command\": ").append(sameBackLastCommand).append(",\n");
        json.append("  \"team_release_controller\": ").append(teamReleaseController).append(",\n");
        json.append("  \"team_release_command\": ").append(teamReleaseCommand).append(",\n");
        json.append("  \"team_release_queue\": ").append(teamReleaseQueue).append(",\n");
        json.append("  \"team_release_target\": ").append(teamReleaseTarget).append(",\n");
        json.append("  \"team_release_stances\": ").append(teamReleaseStances).append(",\n");
        json.append("  \"team_release_team\": ").append(teamReleaseTeam).append(",\n");
        json.append("  \"team_release_last_command\": ").append(teamReleaseLastCommand).append(",\n");
        json.append("  \"disconnect_player_unit_null\": ").append(disconnectPlayerUnitNull).append(",\n");
        json.append("  \"disconnect_controller\": ").append(disconnectController).append(",\n");
        json.append("  \"disconnect_command\": ").append(disconnectCommand).append(",\n");
        json.append("  \"disconnect_queue\": ").append(disconnectQueue).append(",\n");
        json.append("  \"disconnect_target\": ").append(disconnectTarget).append(",\n");
        json.append("  \"disconnect_stances\": ").append(disconnectStances).append(",\n");
        json.append("  \"disconnect_team\": ").append(disconnectTeam).append(",\n");
        json.append("  \"disconnect_last_command\": ").append(disconnectLastCommand).append(",\n");
        json.append("  \"death_player_unit_null\": ").append(deathPlayerUnitNull).append(",\n");
        json.append("  \"death_controller\": ").append(deathController).append(",\n");
        json.append("  \"death_command\": ").append(deathCommand).append(",\n");
        json.append("  \"death_queue\": ").append(deathQueue).append(",\n");
        json.append("  \"death_target\": ").append(deathTarget).append(",\n");
        json.append("  \"death_stances\": ").append(deathStances).append(",\n");
        json.append("  \"death_team\": ").append(deathTeam).append(",\n");
        json.append("  \"death_last_command\": ").append(deathLastCommand).append("\n");
        json.append("}\n");
        System.out.print(json);
    }

    static int cmdId(UnitCommand command) {
        return command == null ? -1 : command.id;
    }

    /** Controller kind: 0 Player, 1 CommandAI, 2 any other controller. */
    static int controllerKind(Unit unit) {
        return unit.isPlayer() ? 0 : unit.controller() instanceof CommandAI ? 1 : 2;
    }

    static int commandId(Unit unit) {
        return unit.controller() instanceof CommandAI ai ? cmdId(ai.command) : -1;
    }

    static int queueSize(Unit unit) {
        return unit.controller() instanceof CommandAI ai ? ai.commandQueue.size : -1;
    }

    /** Target kind: 0 none, 1 position, 2 Teamc target. */
    static int targetKind(Unit unit) {
        return unit.controller() instanceof CommandAI ai ? targetKind(ai) : -1;
    }

    static int targetKind(CommandAI ai) {
        return ai.attackTarget != null ? 2 : ai.targetPos != null ? 1 : 0;
    }

    static int stanceMask(Unit unit) {
        return unit.controller() instanceof CommandAI ai ? stanceMask(ai) : -1;
    }

    static int stanceMask(CommandAI ai) {
        int mask = 0;
        for (int i = 0; i < Vars.content.unitStances().size && i < 31; i++) {
            if (ai.stances.get(i)) mask |= 1 << i;
        }
        return mask;
    }

    /** Reads version.properties from the classpath (the desktop.jar root). */
    static String classpathVersion() throws Exception {
        try (InputStream in = ParPossess158.class.getResourceAsStream("/version.properties")) {
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
