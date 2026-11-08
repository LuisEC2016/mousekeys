.pragma library

// Same keys and default values as src/config.rs. Booleans travel as 1/0.
var tunables = {
    "vMin": 90,
    "vMax": 1800,
    "delayMs": 200,
    "rampMs": 800,
    "minStep": 2,
    "clickTimeoutMs": 3000,
    "holdMs": 300,
    "doubleMs": 300,
    "fallbackStep": 40,
    "jumpFallback": 1920,
    "dragWaitMods": true
};

function binaryPath(pluginData, home) {
    return (pluginData && pluginData.binaryPath) || (home + "/.local/bin/niri-mousekeys");
}

function wire(v) {
    return typeof v === "boolean" ? (v ? 1 : 0) : v;
}

function configPairs(pluginData) {
    var pairs = [];
    for (var key in tunables) {
        var v = pluginData ? pluginData[key] : undefined;
        var ok = typeof v === typeof tunables[key] && (typeof v !== "number" || isFinite(v));
        pairs.push(key + "=" + wire(ok ? v : tunables[key]));
    }
    return pairs;
}

function pluginRoot(resolvedParentUrl) {
    return resolvedParentUrl.toString().replace(/^file:\/\//, "").replace(/\/$/, "");
}

// sh -c with the plugin path as $1: no paths interpolated into the script.
// --force: without it cargo refuses to replace a binary it did not install
// itself (a manual copy or a symlink at ~/.local/bin/niri-mousekeys), with
// "binary `niri-mousekeys` already exists in destination".
var buildScript = 'export PATH="$HOME/.cargo/bin:$PATH"; '
    + 'cargo install --force --locked --path "$1" --root "$HOME/.local" '
    + '--target-dir "${XDG_CACHE_HOME:-$HOME/.cache}/niri-mousekeys" 2>&1';
