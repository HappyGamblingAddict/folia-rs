//! Shared-shape weather state with ticking game-event transitions.
//!
//! Mirrors the overworld weather clock used by Folia 1.21.11: rain and thunder
//! flags, their countdown timers, and the interpolated levels the client
//! renders. Timers are set by `/weather` and advanced once per session tick;
//! only level/flag transitions are emitted as `ClientboundGameEvent` packets.

use folia_random::RandomSource;

/// Overworld weather state shared by the join session's clock and the
/// `/weather` command.
#[derive(Debug, Clone, PartialEq)]
pub struct Weather {
    pub raining: bool,
    pub thundering: bool,
    pub rain_ticks: i32,
    pub thunder_ticks: i32,
    pub rain_level: f32,
    pub thunder_level: f32,
}

impl Weather {
    /// A newly joined world starts clear with no pending timers.
    pub fn new() -> Self {
        Self {
            raining: false,
            thundering: false,
            rain_ticks: 0,
            thunder_ticks: 0,
            rain_level: 0.0,
            thunder_level: 0.0,
        }
    }

    /// Apply a `/weather` preset. `duration_ticks <= 0` selects vanilla's
    /// random duration. Returns the `ClientboundGameEvent` transitions, or
    /// `None` for an unknown preset (leaving the state untouched).
    pub fn set(
        &mut self,
        kind: &str,
        duration_ticks: i32,
        random: &mut impl RandomSource,
    ) -> Option<Vec<(u8, f32)>> {
        let events = weather_events(kind)?;
        match kind {
            "clear" => {
                self.raining = false;
                self.thundering = false;
                self.rain_ticks = 0;
                self.thunder_ticks = 0;
                self.rain_level = 0.0;
                self.thunder_level = 0.0;
            }
            "rain" => {
                self.rain_ticks = if duration_ticks > 0 {
                    duration_ticks
                } else {
                    12_000 + random.next_int_bound(168_001)
                };
                self.thunder_ticks = 0;
                self.raining = true;
                self.thundering = false;
                self.rain_level = 1.0;
                self.thunder_level = 0.0;
            }
            "thunder" => {
                // Vanilla uses the same duration for both the rain and the
                // thunder countdown when thunder is requested.
                let ticks = if duration_ticks > 0 {
                    duration_ticks
                } else {
                    3_600 + random.next_int_bound(12_001)
                };
                self.rain_ticks = ticks;
                self.thunder_ticks = ticks;
                self.raining = true;
                self.thundering = true;
                self.rain_level = 1.0;
                self.thunder_level = 1.0;
            }
            _ => unreachable!("weather_events rejects unknown kinds"),
        }
        Some(events)
    }

    /// Advance the clock by one tick and return only the level/flag
    /// transitions that must be sent to clients.
    ///
    /// This is an explicit simplification: rain never starts thunder on its
    /// own. Vanilla can begin thunder while rain is already active; here
    /// thunder only comes from an explicit `/weather thunder`.
    pub fn tick(&mut self, _random: &mut impl RandomSource) -> Vec<(u8, f32)> {
        if self.rain_ticks > 0 {
            self.rain_ticks -= 1;
        }
        if self.thunder_ticks > 0 {
            self.thunder_ticks -= 1;
        }

        if self.raining && self.rain_ticks == 0 {
            self.raining = false;
            self.thundering = false;
            self.rain_ticks = 0;
            self.thunder_ticks = 0;
            self.rain_level = 0.0;
            self.thunder_level = 0.0;
            return vec![(2, 0.0), (7, 0.0), (8, 0.0)];
        }
        if self.thundering && self.thunder_ticks == 0 {
            self.thundering = false;
            self.thunder_level = 0.0;
            return vec![(8, 0.0)];
        }
        Vec::new()
    }
}

impl Default for Weather {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the `ClientboundGameEvent` sequence for a `/weather` preset.
///
/// Event ids: 1 = START_RAINING, 2 = STOP_RAINING, 7 = RAIN_LEVEL_CHANGE,
/// 8 = THUNDER_LEVEL_CHANGE. Unknown kinds return `None`.
pub fn weather_events(kind: &str) -> Option<Vec<(u8, f32)>> {
    match kind {
        "clear" => Some(vec![(2, 0.0), (7, 0.0), (8, 0.0)]),
        "rain" => Some(vec![(1, 0.0), (7, 1.0), (8, 0.0)]),
        "thunder" => Some(vec![(1, 0.0), (7, 1.0), (8, 1.0)]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use folia_random::LegacyRandomSource;

    fn rng() -> LegacyRandomSource {
        LegacyRandomSource::new(0x9E37)
    }

    #[test]
    fn clear_set_events_and_state() {
        let mut weather = Weather::new();
        let events = weather.set("clear", 0, &mut rng()).unwrap();
        assert_eq!(events, vec![(2, 0.0), (7, 0.0), (8, 0.0)]);
        assert!(!weather.raining);
        assert!(!weather.thundering);
        assert_eq!((weather.rain_ticks, weather.thunder_ticks), (0, 0));
        assert_eq!((weather.rain_level, weather.thunder_level), (0.0, 0.0));
    }

    #[test]
    fn rain_set_events_and_random_duration() {
        let mut weather = Weather::new();
        let events = weather.set("rain", 0, &mut rng()).unwrap();
        assert_eq!(events, vec![(1, 0.0), (7, 1.0), (8, 0.0)]);
        assert!(weather.raining);
        assert!(!weather.thundering);
        assert!((12_000..=180_000).contains(&weather.rain_ticks));
        assert_eq!(weather.thunder_ticks, 0);
        assert_eq!((weather.rain_level, weather.thunder_level), (1.0, 0.0));
    }

    #[test]
    fn thunder_set_events_and_random_duration() {
        let mut weather = Weather::new();
        let events = weather.set("thunder", 0, &mut rng()).unwrap();
        assert_eq!(events, vec![(1, 0.0), (7, 1.0), (8, 1.0)]);
        assert!(weather.raining && weather.thundering);
        assert!((3_600..=15_600).contains(&weather.rain_ticks));
        assert!((3_600..=15_600).contains(&weather.thunder_ticks));
        assert_eq!(weather.rain_ticks, weather.thunder_ticks);
        assert_eq!((weather.rain_level, weather.thunder_level), (1.0, 1.0));
    }

    #[test]
    fn explicit_duration_is_used_for_both_thunder_timers() {
        let mut weather = Weather::new();
        weather.set("thunder", 250, &mut rng()).unwrap();
        assert_eq!((weather.rain_ticks, weather.thunder_ticks), (250, 250));
        weather.set("rain", 500, &mut rng()).unwrap();
        assert_eq!((weather.rain_ticks, weather.thunder_ticks), (500, 0));
    }

    #[test]
    fn seeded_durations_are_deterministic() {
        let mut a = Weather::new();
        let mut b = Weather::new();
        a.set("rain", 0, &mut LegacyRandomSource::new(12_345))
            .unwrap();
        b.set("rain", 0, &mut LegacyRandomSource::new(12_345))
            .unwrap();
        assert_eq!(a.rain_ticks, b.rain_ticks);
        a.set("thunder", 0, &mut LegacyRandomSource::new(999))
            .unwrap();
        b.set("thunder", 0, &mut LegacyRandomSource::new(999))
            .unwrap();
        assert_eq!(a.thunder_ticks, b.thunder_ticks);
    }

    #[test]
    fn rain_expiry_clears_thunder() {
        let mut weather = Weather::new();
        weather.set("thunder", 100, &mut rng()).unwrap();
        weather.rain_ticks = 3;
        weather.thunder_ticks = 5;
        let mut random = rng();
        assert!(weather.tick(&mut random).is_empty());
        assert!(weather.tick(&mut random).is_empty());
        assert_eq!(
            weather.tick(&mut random),
            vec![(2, 0.0), (7, 0.0), (8, 0.0)]
        );
        assert!(!weather.raining && !weather.thundering);
        assert_eq!((weather.rain_ticks, weather.thunder_ticks), (0, 0));
        assert_eq!((weather.rain_level, weather.thunder_level), (0.0, 0.0));
    }

    #[test]
    fn thunder_expiry_leaves_rain() {
        let mut weather = Weather::new();
        weather.set("thunder", 100, &mut rng()).unwrap();
        weather.rain_ticks = 5;
        weather.thunder_ticks = 2;
        let mut random = rng();
        assert!(weather.tick(&mut random).is_empty());
        assert_eq!(weather.tick(&mut random), vec![(8, 0.0)]);
        assert!(weather.raining);
        assert!(!weather.thundering);
        assert_eq!(weather.rain_ticks, 3);
        assert_eq!((weather.rain_level, weather.thunder_level), (1.0, 0.0));
    }

    #[test]
    fn no_automatic_thunder_start() {
        let mut weather = Weather::new();
        weather.set("rain", 3, &mut rng()).unwrap();
        let mut random = rng();
        assert!(weather.tick(&mut random).is_empty());
        assert!(weather.tick(&mut random).is_empty());
        assert_eq!(
            weather.tick(&mut random),
            vec![(2, 0.0), (7, 0.0), (8, 0.0)]
        );
        assert!(!weather.thundering);
    }

    #[test]
    fn unknown_kind_returns_none_without_changing_state() {
        let mut weather = Weather::new();
        weather.set("rain", 50, &mut rng()).unwrap();
        let before = weather.clone();
        assert_eq!(weather.set("sunny", 0, &mut rng()), None);
        assert_eq!(weather, before);
    }

    #[test]
    fn weather_events_match_vanilla_sequence() {
        assert_eq!(
            weather_events("clear"),
            Some(vec![(2, 0.0), (7, 0.0), (8, 0.0)])
        );
        assert_eq!(
            weather_events("rain"),
            Some(vec![(1, 0.0), (7, 1.0), (8, 0.0)])
        );
        assert_eq!(
            weather_events("thunder"),
            Some(vec![(1, 0.0), (7, 1.0), (8, 1.0)])
        );
    }

    #[test]
    fn weather_events_reject_unknown_kind() {
        assert_eq!(weather_events("sunny"), None);
        assert_eq!(weather_events(""), None);
        assert_eq!(weather_events("RAIN"), None);
    }
}
