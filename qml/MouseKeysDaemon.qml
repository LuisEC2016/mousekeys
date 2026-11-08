import QtQuick
import Quickshell
import Quickshell.Io
import qs.Common
import qs.Services
import qs.Modules.Plugins
import "Defaults.js" as Defaults

PluginComponent {
    id: root

    readonly property string binary: Defaults.binaryPath(pluginData, Quickshell.env("HOME"))
    readonly property string pluginRoot: Defaults.pluginRoot(Qt.resolvedUrl(".."))
    readonly property string manifestVersion: PluginService.availablePlugins[pluginId]?.version ?? ""
    property string runningBinary: ""
    property string probedBinary: ""
    property bool stopping: false
    property bool pendingStart: false
    property int failures: 0
    property real startedAt: 0
    property string lastLine: ""
    property string sentConfig: ""

    function setState(state) {
        PluginService.setGlobalVar(pluginId, "daemonState", state);
    }

    // Checks the binary and its version before starting it.
    function probe() {
        restartTimer.stop();
        probedBinary = binary;
        Proc.runCommand("niriMouseKeys.version", ["sh", "-c", "exec \"$1\" version", "sh", binary], (stdout, exitCode) => {
            if (root.stopping)
                return;
            if (exitCode !== 0) {
                root.setState("missing");
                ToastService.showWarning(I18n.trFor("niriMouseKeys", "niri-mousekeys binary not found"), I18n.trFor("niriMouseKeys", "Open Settings → Plugins → Niri Mouse Keys and press Build, or install it at %1.").arg(root.binary));
                return;
            }
            const version = stdout.trim();
            if (root.manifestVersion && version !== root.manifestVersion)
                ToastService.showWarning(I18n.trFor("niriMouseKeys", "niri-mousekeys %1 does not match the plugin (%2)").arg(version).arg(root.manifestVersion), I18n.trFor("niriMouseKeys", "Rebuild it from the plugin settings."));
            root.start();
        });
    }

    function start() {
        const pairs = Defaults.configPairs(pluginData);
        runningBinary = binary;
        sentConfig = pairs.join(" ");
        daemon.command = [binary, "daemon", "--supervised"].concat(pairs);
        daemon.running = true;
    }

    // A slider emits a change per step and DMS saves each one: send only the
    // settled value, and only if it differs from what the daemon already has.
    function sendConfig() {
        const line = Defaults.configPairs(pluginData).join(" ");
        if (!daemon.running || line === sentConfig)
            return;
        sentConfig = line;
        daemon.write("config " + line + "\n");
    }

    function restart() {
        if (daemon.running) {
            pendingStart = true;
            daemon.running = false;
        } else {
            probe();
        }
    }

    Process {
        id: daemon

        stdinEnabled: true

        stderr: SplitParser {
            onRead: line => {
                root.lastLine = line;
                console.info("niriMouseKeys:", line);
            }
        }

        onStarted: {
            root.startedAt = Date.now();
            root.setState("running");
        }

        onExited: exitCode => {
            if (root.stopping)
                return;
            if (root.pendingStart) {
                root.pendingStart = false;
                root.probe();
                return;
            }
            root.failures = Date.now() - root.startedAt > 30000 ? 1 : root.failures + 1;
            if (root.failures > 5) {
                root.setState("failed");
                ToastService.showError(I18n.trFor("niriMouseKeys", "Mouse keys daemon keeps exiting (code %1)").arg(exitCode), root.lastLine);
                return;
            }
            root.setState("restarting");
            restartTimer.interval = Math.min(30000, 1000 * Math.pow(2, root.failures - 1));
            restartTimer.restart();
        }
    }

    Timer {
        id: restartTimer
        repeat: false
        onTriggered: root.start()
    }

    Timer {
        id: configTimer
        interval: 150
        repeat: false
        onTriggered: root.sendConfig()
    }

    onPluginDataChanged: {
        if (stopping || pendingStart)
            return;
        if (daemon.running) {
            if (binary !== runningBinary)
                restart();
            else
                configTimer.restart();
            return;
        }
        // No daemon (binary missing or failing): a new path deserves another try.
        if (probedBinary !== "" && binary !== probedBinary) {
            failures = 0;
            probe();
        }
    }

    Connections {
        target: PluginService
        function onGlobalVarChanged(changedId, varName) {
            if (changedId === root.pluginId && varName === "restartRequest") {
                root.failures = 0;
                root.restart();
            }
        }
    }

    Component.onCompleted: probe()

    Component.onDestruction: {
        stopping = true;
        restartTimer.stop();
        configTimer.stop();
        daemon.running = false;
        setState("stopped");
    }
}
