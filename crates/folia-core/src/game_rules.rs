//! Game-rule storage and the shared global rule set.
//!
//! Mirrors the subset of Folia 1.21.11 game rules the server currently acts
//! on. Rule names are canonicalized to vanilla's camelCase spelling; both the
//! camelCase name and a snake_case alias are accepted on input so console and
//! command callers can use either form.

use std::sync::{Mutex, OnceLock};

/// The game rules the Rust server currently reads. Defaults match a vanilla
/// 1.21.11 world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRules {
    pub do_daylight_cycle: bool,
    pub keep_inventory: bool,
    pub mob_griefing: bool,
    pub do_tile_drops: bool,
}

impl GameRules {
    /// Map an input rule name to its canonical vanilla (camelCase) name.
    /// Returns `None` for rules this store does not model.
    pub fn parse_name(name: &str) -> Option<&'static str> {
        match name {
            "doDaylightCycle" | "do_daylight_cycle" => Some("doDaylightCycle"),
            "keepInventory" | "keep_inventory" => Some("keepInventory"),
            "mobGriefing" | "mob_griefing" => Some("mobGriefing"),
            "doTileDrops" | "do_tile_drops" => Some("doTileDrops"),
            _ => None,
        }
    }

    fn bool_value(&self, canonical: &str) -> Option<bool> {
        match canonical {
            "doDaylightCycle" => Some(self.do_daylight_cycle),
            "keepInventory" => Some(self.keep_inventory),
            "mobGriefing" => Some(self.mob_griefing),
            "doTileDrops" => Some(self.do_tile_drops),
            _ => None,
        }
    }

    /// Read a canonical rule as the vanilla "true"/"false" string, or `None`
    /// when the name is not a rule this store models.
    pub fn get(&self, canonical: &str) -> Option<String> {
        self.bool_value(canonical)
            .map(|value| if value { "true" } else { "false" }.to_owned())
    }

    /// Set a canonical rule. Unknown names are ignored.
    pub fn set(&mut self, canonical: &str, value: bool) {
        match canonical {
            "doDaylightCycle" => self.do_daylight_cycle = value,
            "keepInventory" => self.keep_inventory = value,
            "mobGriefing" => self.mob_griefing = value,
            "doTileDrops" => self.do_tile_drops = value,
            _ => {}
        }
    }
}

impl Default for GameRules {
    fn default() -> Self {
        Self {
            do_daylight_cycle: true,
            keep_inventory: false,
            mob_griefing: true,
            do_tile_drops: true,
        }
    }
}

static GLOBAL: OnceLock<Mutex<GameRules>> = OnceLock::new();

/// The process-wide rule set, initialized with vanilla defaults on first use.
pub fn global() -> &'static Mutex<GameRules> {
    GLOBAL.get_or_init(|| Mutex::new(GameRules::default()))
}

/// Read a boolean rule by either its camelCase or snake_case name.
pub fn bool_rule(name: &str) -> Option<bool> {
    let canonical = GameRules::parse_name(name)?;
    let rules = global()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    rules.bool_value(canonical)
}

/// Set a boolean rule by either its camelCase or snake_case name. Returns
/// `false` when `name` is not a modeled rule.
pub fn set_rule(name: &str, value: bool) -> bool {
    let Some(canonical) = GameRules::parse_name(name) else {
        return false;
    };
    let mut rules = global()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    rules.set(canonical, value);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_vanilla() {
        let rules = GameRules::default();
        assert!(rules.do_daylight_cycle);
        assert!(!rules.keep_inventory);
        assert!(rules.mob_griefing);
        assert!(rules.do_tile_drops);
    }

    #[test]
    fn aliases_resolve_to_canonical_camel_case() {
        assert_eq!(
            GameRules::parse_name("doDaylightCycle"),
            Some("doDaylightCycle")
        );
        assert_eq!(
            GameRules::parse_name("do_daylight_cycle"),
            Some("doDaylightCycle")
        );
        assert_eq!(
            GameRules::parse_name("keepInventory"),
            Some("keepInventory")
        );
        assert_eq!(
            GameRules::parse_name("keep_inventory"),
            Some("keepInventory")
        );
        assert_eq!(GameRules::parse_name("mobGriefing"), Some("mobGriefing"));
        assert_eq!(GameRules::parse_name("mob_griefing"), Some("mobGriefing"));
        assert_eq!(GameRules::parse_name("doTileDrops"), Some("doTileDrops"));
        assert_eq!(GameRules::parse_name("do_tile_drops"), Some("doTileDrops"));
    }

    #[test]
    fn set_and_get_round_trip() {
        let mut rules = GameRules::default();
        assert_eq!(rules.get("doDaylightCycle").as_deref(), Some("true"));
        rules.set("doDaylightCycle", false);
        assert_eq!(rules.get("doDaylightCycle").as_deref(), Some("false"));
        assert_eq!(rules.get("keepInventory").as_deref(), Some("false"));
        rules.set("keepInventory", true);
        assert_eq!(rules.get("keepInventory").as_deref(), Some("true"));
        rules.set("mobGriefing", false);
        assert_eq!(rules.get("mobGriefing").as_deref(), Some("false"));
        rules.set("doTileDrops", false);
        assert_eq!(rules.get("doTileDrops").as_deref(), Some("false"));
    }

    #[test]
    fn unknown_names_are_rejected() {
        assert_eq!(GameRules::parse_name("fallingBlocks"), None);
        assert_eq!(GameRules::parse_name("DO_DAYLIGHT_CYCLE"), None);
        assert_eq!(GameRules::parse_name(""), None);
        let rules = GameRules::default();
        assert_eq!(rules.get("fallingBlocks"), None);
        let mut rules = GameRules::default();
        rules.set("fallingBlocks", true);
        assert_eq!(rules, GameRules::default());
    }

    #[test]
    fn global_accessors_accept_both_aliases() {
        assert_eq!(bool_rule("do_daylight_cycle"), Some(true));
        assert!(set_rule("doDaylightCycle", false));
        assert_eq!(bool_rule("doDaylightCycle"), Some(false));
        assert_eq!(bool_rule("do_daylight_cycle"), Some(false));
        assert!(!set_rule("fallingBlocks", true));
        assert_eq!(bool_rule("fallingBlocks"), None);
        // Restore the default so other unit tests see the vanilla value.
        assert!(set_rule("doDaylightCycle", true));
        assert_eq!(bool_rule("doDaylightCycle"), Some(true));
    }
}
