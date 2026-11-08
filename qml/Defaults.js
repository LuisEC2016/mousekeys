.pragma library

// Same keys and default values as src/config.rs.
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
    "jumpFallback": 1920
};

function binaryPath(pluginData, home) {
    return (pluginData && pluginData.binaryPath) || (home + "/.local/bin/niri-mousekeys");
}

function configPairs(pluginData) {
    var pairs = [];
    for (var key in tunables) {
        var v = pluginData ? pluginData[key] : undefined;
        pairs.push(key + "=" + (typeof v === "number" && isFinite(v) ? v : tunables[key]));
    }
    return pairs;
}

function pluginRoot(resolvedParentUrl) {
    return resolvedParentUrl.toString().replace(/^file:\/\//, "").replace(/\/$/, "");
}

// sh -c with the plugin path as $1: no paths interpolated into the script.
var buildScript = 'export PATH="$HOME/.cargo/bin:$PATH"; '
    + 'cargo install --locked --path "$1" --root "$HOME/.local" '
    + '--target-dir "${XDG_CACHE_HOME:-$HOME/.cache}/niri-mousekeys" 2>&1';
