//! Daemon settings.
//!
//! The keys are the same ones the plugin settings page stores (`pluginData`)
//! and they travel as `key=value`. Times are in milliseconds because the DMS
//! `SliderSetting` only handles integers.

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// s between movement steps (125 Hz by default).
    pub tick: f64,
    /// px/s at the start: fine positioning.
    pub v_min: f64,
    /// px/s at full speed.
    pub v_max: f64,
    /// s at `v_min` before accelerating.
    pub t_delay: f64,
    /// s to go from `v_min` to `v_max` (quadratic curve).
    pub t_ramp: f64,
    /// px guaranteed even for an instantaneous press.
    pub min_step: f64,
    /// px per press when /dev/input cannot be read.
    pub fallback_step: f64,
    /// s waiting for the keys to be released before the click is dropped.
    pub click_timeout: f64,
    /// s between queued clicks (double click = Enter twice).
    pub click_gap: f64,
    /// px of the monitor jump when niri does not answer.
    pub jump_fallback: f64,
    /// s Space must be held to start a drag.
    pub hold_t: f64,
    /// Maximum s between releasing the first tap and pressing the second.
    pub double_t: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tick: 0.008,
            v_min: 90.0,
            v_max: 1800.0,
            t_delay: 0.20,
            t_ramp: 0.80,
            min_step: 2.0,
            fallback_step: 40.0,
            click_timeout: 3.0,
            click_gap: 0.04,
            jump_fallback: 1920.0,
            hold_t: 0.30,
            double_t: 0.30,
        }
    }
}

/// Key, factor from the wire unit to the internal one, and minimum accepted (in wire units).
const KEYS: [(&str, f64, f64); 12] = [
    ("tickMs", 1e-3, 1.0),
    ("vMin", 1.0, 0.0),
    ("vMax", 1.0, 0.0),
    ("delayMs", 1e-3, 0.0),
    ("rampMs", 1e-3, 1.0),
    ("minStep", 1.0, 0.0),
    ("fallbackStep", 1.0, 0.0),
    ("clickTimeoutMs", 1e-3, 0.0),
    ("clickGapMs", 1e-3, 0.0),
    ("jumpFallback", 1.0, 0.0),
    ("holdMs", 1e-3, 0.0),
    ("doubleMs", 1e-3, 0.0),
];

impl Config {
    fn slot(&mut self, i: usize) -> &mut f64 {
        match i {
            0 => &mut self.tick,
            1 => &mut self.v_min,
            2 => &mut self.v_max,
            3 => &mut self.t_delay,
            4 => &mut self.t_ramp,
            5 => &mut self.min_step,
            6 => &mut self.fallback_step,
            7 => &mut self.click_timeout,
            8 => &mut self.click_gap,
            9 => &mut self.jump_fallback,
            10 => &mut self.hold_t,
            _ => &mut self.double_t,
        }
    }

    /// Applies one `key=value`. Changes nothing if the key or the value is invalid.
    pub fn set(&mut self, pair: &str) -> Result<(), String> {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| tr!("expected key=value: {pair:?}", "se esperaba clave=valor: {pair:?}"))?;
        let i = KEYS
            .iter()
            .position(|(k, _, _)| *k == key.trim())
            .ok_or_else(|| tr!("unknown key: {key:?}", "clave desconocida: {key:?}"))?;
        let (name, scale, min) = KEYS[i];
        let v: f64 = raw
            .trim()
            .parse()
            .map_err(|_| tr!("{name}: not a number {raw:?}", "{name}: valor no numérico {raw:?}"))?;
        if !v.is_finite() || v < min {
            return Err(tr!(
                "{name}: {raw:?} out of range (minimum {min})",
                "{name}: {raw:?} fuera de rango (mínimo {min})"
            ));
        }
        *self.slot(i) = v * scale;
        Ok(())
    }

    /// Applies each `key=value`; returns the errors for the log.
    pub fn apply<'a>(&mut self, pairs: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        pairs.into_iter().filter_map(|p| self.set(p).err()).collect()
    }

    /// Speed in px/s after holding the key for `t` seconds.
    pub fn speed(&self, t: f64) -> f64 {
        if t <= self.t_delay {
            return self.v_min;
        }
        let x = ((t - self.t_delay) / self.t_ramp).min(1.0);
        self.v_min + (self.v_max - self.v_min) * x * x
    }

    pub fn summary(&self) -> String {
        let mut c = self.clone();
        let mut out = tr!("settings:", "ajustes:");
        for (i, (name, scale, _)) in KEYS.iter().enumerate() {
            let v = *c.slot(i) / scale;
            let _ = write!(out, " {name}={}", (v * 1000.0).round() / 1000.0);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Distance covered holding the key for `secs` seconds, in `tick` steps.
    fn travel(cfg: &Config, secs: f64) -> f64 {
        let mut t = 0.0;
        let mut d = 0.0;
        while t < secs {
            t += cfg.tick;
            d += cfg.speed(t) * cfg.tick;
        }
        d
    }

    #[test]
    fn default_curve_matches_the_readme() {
        let c = Config::default();
        assert_eq!(c.speed(0.0), 90.0);
        assert_eq!(c.speed(0.2), 90.0);
        assert_eq!(c.speed(5.0), 1800.0);
        // "about 70 px in 0.5 s, about 540 px in 1 s and 1920 px in about 1.8 s"
        let half = travel(&c, 0.5);
        let one = travel(&c, 1.0);
        assert!((60.0..80.0).contains(&half), "{half}");
        assert!((520.0..570.0).contains(&one), "{one}");
        let mut t = 0.0;
        while travel(&c, t) < 1920.0 {
            t += 0.01;
        }
        assert!((1.7..1.9).contains(&t), "{t}");
    }

    #[test]
    fn keys_in_milliseconds() {
        let mut c = Config::default();
        assert!(c.apply(["vMax=2600", "rampMs=600", "holdMs=450"]).is_empty());
        assert_eq!(c.v_max, 2600.0);
        assert!((c.t_ramp - 0.6).abs() < 1e-12);
        assert!((c.hold_t - 0.45).abs() < 1e-12);
    }

    #[test]
    fn rejects_without_touching_the_value() {
        let mut c = Config::default();
        let errs = c.apply(["tickMs=0", "rampMs=0", "vMin=-1", "foo=1", "vMax", "vMax=abc", "vMax=inf"]);
        assert_eq!(errs.len(), 7, "{errs:?}");
        assert_eq!(c, Config::default());
    }

    #[test]
    fn summary_in_wire_units() {
        let s = Config::default().summary();
        assert!(s.contains("tickMs=8 "), "{s}");
        assert!(s.contains("holdMs=300 "), "{s}");
        assert!(s.contains("vMax=1800 "), "{s}");
    }
}
