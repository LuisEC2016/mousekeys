import QtQuick
import Quickshell
import Quickshell.Io
import qs.Common
import qs.Services
import qs.Widgets
import qs.Modules.Plugins
import "Defaults.js" as Defaults

PluginSettings {
    id: root
    pluginId: "niriMouseKeys"

    readonly property string pluginRoot: Defaults.pluginRoot(Qt.resolvedUrl(".."))
    readonly property string home: Quickshell.env("HOME")
    readonly property string daemonState: PluginService.globalVars[pluginId]?.daemonState ?? ""
    property string binary: Defaults.binaryPath(SettingsData.getPluginSettingsForPlugin(pluginId), home)
    property string checkOutput: ""
    property string bindsText: ""
    property string buildOutput: ""

    function refresh() {
        binary = Defaults.binaryPath(SettingsData.getPluginSettingsForPlugin(pluginId), home);
        checkOutput = "";
        checkProc.command = ["sh", "-c", "exec \"$1\" check", "sh", binary];
        checkProc.running = true;
        bindsProc.command = ["sh", "-c", "exec \"$1\" binds", "sh", binary];
        bindsProc.running = true;
    }

    function stateText(state) {
        switch (state) {
        case "running":
            return I18n.trFor("niriMouseKeys", "Daemon running");
        case "missing":
            return I18n.trFor("niriMouseKeys", "Binary not found: press Build");
        case "restarting":
            return I18n.trFor("niriMouseKeys", "Daemon restarting");
        case "failed":
            return I18n.trFor("niriMouseKeys", "Daemon keeps failing: see the diagnostics below");
        default:
            return I18n.trFor("niriMouseKeys", "Daemon stopped");
        }
    }

    Process {
        id: checkProc
        stdout: StdioCollector {
            onStreamFinished: {
                if (text.trim() !== "")
                    root.checkOutput = text.trim();
            }
        }
        onExited: exitCode => {
            if (exitCode === 126 || exitCode === 127)
                root.checkOutput = I18n.trFor("niriMouseKeys", "Cannot run %1").arg(root.binary);
        }
    }

    Process {
        id: bindsProc
        stdout: StdioCollector {
            onStreamFinished: root.bindsText = text.trim()
        }
        onExited: exitCode => {
            if (exitCode !== 0)
                root.bindsText = "";
        }
    }

    Process {
        id: buildProc
        command: ["sh", "-c", Defaults.buildScript, "sh", root.pluginRoot]
        stdout: StdioCollector {
            onStreamFinished: root.buildOutput = text.trim().split("\n").slice(-12).join("\n")
        }
        onExited: exitCode => {
            if (exitCode !== 0) {
                ToastService.showError(I18n.trFor("niriMouseKeys", "Build failed (code %1)").arg(exitCode), root.buildOutput);
                return;
            }
            ToastService.showInfo(I18n.trFor("niriMouseKeys", "niri-mousekeys installed in ~/.local/bin"));
            PluginService.setGlobalVar(root.pluginId, "restartRequest", Date.now());
            restartRefresh.restart();
        }
    }

    Timer {
        id: restartRefresh
        interval: 800
        onTriggered: root.refresh()
    }

    Connections {
        target: root.pluginService
        function onPluginDataChanged(changedId) {
            if (changedId !== root.pluginId)
                return;
            const next = Defaults.binaryPath(SettingsData.getPluginSettingsForPlugin(root.pluginId), root.home);
            if (next !== root.binary)
                root.refresh();
        }
    }

    Component.onCompleted: refresh()

    StyledText {
        width: parent.width
        text: I18n.trFor("niriMouseKeys", "Mouse keys for niri")
        font.pixelSize: Theme.fontSizeLarge
        font.weight: Font.Medium
        color: Theme.surfaceText
    }

    StyledText {
        width: parent.width
        text: I18n.trFor("niriMouseKeys", "Super+Ctrl+arrows move the pointer and accelerate while held. Super+Ctrl+Enter clicks when you release the keys, Super+Ctrl+Space taps right-click, double taps left-click and holds to drag. Changes apply instantly.")
        font.pixelSize: Theme.fontSizeSmall
        color: Theme.surfaceVariantText
        wrapMode: Text.WordWrap
    }

    StyledText {
        width: parent.width
        text: root.stateText(root.daemonState)
        font.pixelSize: Theme.fontSizeMedium
        color: root.daemonState === "running" ? Theme.primary : Theme.warning
    }

    StyledRect {
        width: parent.width
        height: checkText.implicitHeight + Theme.spacingM * 2
        radius: Theme.cornerRadius
        color: Theme.surfaceContainerHigh

        StyledText {
            id: checkText
            anchors.fill: parent
            anchors.margins: Theme.spacingM
            text: root.checkOutput || "…"
            font.family: Theme.monoFontFamily
            font.pixelSize: Theme.fontSizeSmall
            color: Theme.surfaceText
            wrapMode: Text.WrapAnywhere
        }
    }

    Row {
        spacing: Theme.spacingS

        DankButton {
            text: I18n.trFor("niriMouseKeys", "Run diagnostics")
            iconName: "troubleshoot"
            onClicked: root.refresh()
        }

        DankButton {
            text: I18n.trFor("niriMouseKeys", "Restart daemon")
            iconName: "restart_alt"
            onClicked: {
                PluginService.setGlobalVar(root.pluginId, "restartRequest", Date.now());
                restartRefresh.restart();
            }
        }

        DankButton {
            text: buildProc.running ? I18n.trFor("niriMouseKeys", "Building…") : I18n.trFor("niriMouseKeys", "Build and install")
            iconName: "build"
            enabled: !buildProc.running
            onClicked: {
                root.buildOutput = "";
                buildProc.running = true;
            }
        }
    }

    StyledText {
        width: parent.width
        visible: root.buildOutput !== ""
        text: root.buildOutput
        font.family: Theme.monoFontFamily
        font.pixelSize: Theme.fontSizeSmall
        color: Theme.surfaceVariantText
        wrapMode: Text.WrapAnywhere
    }

    StringSetting {
        settingKey: "binaryPath"
        label: I18n.trFor("niriMouseKeys", "Binary path")
        description: I18n.trFor("niriMouseKeys", "Empty uses ~/.local/bin/niri-mousekeys, where Build installs it (needs Rust/cargo). Your niri binds must point to the same file.")
        placeholder: root.home + "/.local/bin/niri-mousekeys"
        defaultValue: ""
    }

    StyledText {
        width: parent.width
        text: I18n.trFor("niriMouseKeys", "niri binds")
        font.pixelSize: Theme.fontSizeLarge
        font.weight: Font.Medium
        color: Theme.surfaceText
    }

    StyledText {
        width: parent.width
        text: I18n.trFor("niriMouseKeys", "Paste them inside binds { } in ~/.config/niri/config.kdl after commenting out niri's default Mod+Ctrl+arrows and Mod+Shift+Ctrl+Left/Right. The binary is spawned directly, without a shell.")
        font.pixelSize: Theme.fontSizeSmall
        color: Theme.surfaceVariantText
        wrapMode: Text.WordWrap
    }

    StyledRect {
        width: parent.width
        height: bindsLabel.implicitHeight + Theme.spacingM * 2
        radius: Theme.cornerRadius
        color: Theme.surfaceContainerHigh
        visible: root.bindsText !== ""

        StyledText {
            id: bindsLabel
            anchors.fill: parent
            anchors.margins: Theme.spacingM
            text: root.bindsText
            font.family: Theme.monoFontFamily
            font.pixelSize: Theme.fontSizeSmall
            color: Theme.surfaceText
            wrapMode: Text.WrapAnywhere
        }
    }

    DankButton {
        text: I18n.trFor("niriMouseKeys", "Copy binds")
        iconName: "content_copy"
        enabled: root.bindsText !== ""
        onClicked: {
            Quickshell.clipboardText = root.bindsText + "\n";
            ToastService.showInfo(I18n.trFor("niriMouseKeys", "Binds copied to the clipboard"));
        }
    }

    StyledText {
        width: parent.width
        text: I18n.trFor("niriMouseKeys", "Movement")
        font.pixelSize: Theme.fontSizeLarge
        font.weight: Font.Medium
        color: Theme.surfaceText
    }

    SliderSetting {
        settingKey: "vMin"
        label: I18n.trFor("niriMouseKeys", "Start speed")
        description: I18n.trFor("niriMouseKeys", "Speed while you start holding an arrow. Lower is more precise.")
        defaultValue: Defaults.tunables.vMin
        minimum: 10
        maximum: 600
        unit: "px/s"
    }

    SliderSetting {
        settingKey: "vMax"
        label: I18n.trFor("niriMouseKeys", "Top speed")
        description: I18n.trFor("niriMouseKeys", "Raise it to cross several monitors faster.")
        defaultValue: Defaults.tunables.vMax
        minimum: 200
        maximum: 6000
        unit: "px/s"
    }

    SliderSetting {
        settingKey: "delayMs"
        label: I18n.trFor("niriMouseKeys", "Acceleration delay")
        description: I18n.trFor("niriMouseKeys", "Time at start speed before accelerating.")
        defaultValue: Defaults.tunables.delayMs
        minimum: 0
        maximum: 1500
        unit: "ms"
    }

    SliderSetting {
        settingKey: "rampMs"
        label: I18n.trFor("niriMouseKeys", "Acceleration time")
        description: I18n.trFor("niriMouseKeys", "Time to go from start to top speed (quadratic curve).")
        defaultValue: Defaults.tunables.rampMs
        minimum: 50
        maximum: 3000
        unit: "ms"
    }

    SliderSetting {
        settingKey: "minStep"
        label: I18n.trFor("niriMouseKeys", "Minimum step")
        description: I18n.trFor("niriMouseKeys", "Pixels moved by a very short tap.")
        defaultValue: Defaults.tunables.minStep
        minimum: 0
        maximum: 20
        unit: "px"
    }

    StyledText {
        width: parent.width
        text: I18n.trFor("niriMouseKeys", "Clicks and Space")
        font.pixelSize: Theme.fontSizeLarge
        font.weight: Font.Medium
        color: Theme.surfaceText
    }

    SliderSetting {
        settingKey: "holdMs"
        label: I18n.trFor("niriMouseKeys", "Hold to drag")
        description: I18n.trFor("niriMouseKeys", "How long Space must be held before the left button goes down. Raise it if slow taps start drags.")
        defaultValue: Defaults.tunables.holdMs
        minimum: 100
        maximum: 1000
        unit: "ms"
    }

    ToggleSetting {
        settingKey: "dragWaitMods"
        label: I18n.trFor("niriMouseKeys", "Drag after releasing Super and Ctrl")
        description: I18n.trFor("niriMouseKeys", "Hold Super+Ctrl+Space, release Super and Ctrl and keep Space down: the button goes down then. Needed on stock niri, which takes Super + left button as \"move window\". Turn it off only with niri patch 0004.")
        defaultValue: Defaults.tunables.dragWaitMods
    }

    SliderSetting {
        settingKey: "doubleMs"
        label: I18n.trFor("niriMouseKeys", "Double tap window")
        description: I18n.trFor("niriMouseKeys", "Maximum gap between two Space taps. It is also the delay of the single-tap right click.")
        defaultValue: Defaults.tunables.doubleMs
        minimum: 100
        maximum: 1000
        unit: "ms"
    }

    SliderSetting {
        settingKey: "clickTimeoutMs"
        label: I18n.trFor("niriMouseKeys", "Click timeout")
        description: I18n.trFor("niriMouseKeys", "A queued click is dropped if keys are still held after this long.")
        defaultValue: Defaults.tunables.clickTimeoutMs
        minimum: 500
        maximum: 10000
        unit: "ms"
    }

    StyledText {
        width: parent.width
        text: I18n.trFor("niriMouseKeys", "Fallbacks")
        font.pixelSize: Theme.fontSizeLarge
        font.weight: Font.Medium
        color: Theme.surfaceText
    }

    SliderSetting {
        settingKey: "fallbackStep"
        label: I18n.trFor("niriMouseKeys", "Step without /dev/input access")
        description: I18n.trFor("niriMouseKeys", "Fixed step per key press when the keyboards cannot be read (user not in the input group).")
        defaultValue: Defaults.tunables.fallbackStep
        minimum: 5
        maximum: 200
        unit: "px"
    }

    SliderSetting {
        settingKey: "jumpFallback"
        label: I18n.trFor("niriMouseKeys", "Monitor jump without niri IPC")
        description: I18n.trFor("niriMouseKeys", "Distance of Super+Ctrl+Shift+Left/Right when niri does not report the focused output width.")
        defaultValue: Defaults.tunables.jumpFallback
        minimum: 640
        maximum: 7680
        unit: "px"
    }
}
