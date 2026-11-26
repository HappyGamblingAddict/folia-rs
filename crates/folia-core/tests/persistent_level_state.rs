use folia_core::game_rules::{self, GameRules};
use folia_core::level::{self, RuntimeChange};
use folia_core::nbt::Tag;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_world(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let root = std::env::temp_dir().join(format!(
        "folia-rs-persistent-{label}-{}-{nanos}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp world directory");
    root
}

fn add_unknown_field(path: &Path) {
    let mut root = level_nbt_root(path);
    if let Tag::Compound(entries) = &mut root
        && let Some((_, Tag::Compound(data))) = entries.iter_mut().find(|(key, _)| key == "Data")
    {
        data.push(("DataPacks".into(), Tag::String("keep me".into())));
    }
    let bytes = folia_core::nbt::write_gzip_root(&root).unwrap();
    fs::write(path, bytes).unwrap();
}

fn level_nbt_root(path: &Path) -> Tag {
    folia_core::nbt::read_gzip_root(&fs::read(path).unwrap()).unwrap()
}

/// A `/gamerule` and a spawn move are written straight to level.dat, with no
/// shutdown hook, and a later process reads them back from disk.
#[test]
fn runtime_rule_and_spawn_changes_survive_a_reload_without_shutdown() {
    let root = temp_world("crash");
    let path = root.join("level.dat");
    level::write_new_world(&path, "CrashWorld", 1234, (0, 64, 0)).unwrap();
    add_unknown_field(&path);

    assert!(game_rules::set_rule_persistent(
        "keep_inventory",
        true,
        &path
    ));
    assert!(game_rules::set_rule_persistent(
        "do_daylight_cycle",
        false,
        &path
    ));
    let moved = level::set_spawn(&path, (-24, 72, 91)).unwrap();
    assert_eq!(moved.spawn, Some((-24, 72, 91)));

    assert_eq!(
        game_rules::bool_rule("keepInventory"),
        Some(true),
        "the in-memory rule changed"
    );
    assert_eq!(game_rules::bool_rule("do_daylight_cycle"), Some(false));

    let reloaded = level::read(&path).expect("level.dat is readable before any shutdown");
    assert_eq!(reloaded.spawn, Some((-24, 72, 91)));
    assert!(reloaded.game_rules.keep_inventory);
    assert!(!reloaded.game_rules.do_daylight_cycle);
    assert_eq!(
        reloaded.game_rules,
        GameRules {
            keep_inventory: true,
            do_daylight_cycle: false,
            ..GameRules::default()
        }
    );

    let root_tag = level_nbt_root(&path);
    let data = root_tag.get("Data").expect("Data compound");
    assert_eq!(
        data.get("DataPacks"),
        Some(&Tag::String("keep me".into())),
        "unknown fields survive every runtime rewrite"
    );
    assert_eq!(
        data.get("GameRules")
            .and_then(|rules| rules.get("keepInventory")),
        Some(&Tag::String("true".into()))
    );
    assert_eq!(
        data.get("GameRules")
            .and_then(|rules| rules.get("doDaylightCycle")),
        Some(&Tag::String("false".into()))
    );
    assert!(!root.join("level.dat.tmp").exists());
    assert!(
        root.join("level.dat_old").exists(),
        "the previous level is retained"
    );

    // Simulate the next process: the persisted rules become the in-memory set.
    assert!(!game_rules::set_rule_persistent(
        "fallingBlocks",
        true,
        &path
    ));
    game_rules::replace_globals(&reloaded.game_rules);
    assert_eq!(game_rules::bool_rule("keepInventory"), Some(true));
    assert_eq!(game_rules::bool_rule("doDaylightCycle"), Some(false));
    assert_eq!(game_rules::bool_rule("fallingBlocks"), None);

    fs::remove_dir_all(root).ok();
}

/// Unknown rules already in the level are not dropped when the port writes its
/// own rules; the whole-root rewrite also keeps a `level.dat_old` backup.
#[test]
fn runtime_write_merges_unmodeled_rules_and_backs_up_the_old_level() {
    let root = temp_world("merge");
    let path = root.join("level.dat");
    level::write_new_world(&path, "Merge", 7, (1, 65, 2)).unwrap();

    let mut with_extra = level_nbt_root(&path);
    if let Tag::Compound(entries) = &mut with_extra
        && let Some((_, Tag::Compound(data))) = entries.iter_mut().find(|(key, _)| key == "Data")
    {
        let rules = data
            .iter_mut()
            .find(|(key, _)| key == "GameRules")
            .and_then(|(_, tag)| match tag {
                Tag::Compound(rules) => Some(rules),
                _ => None,
            })
            .expect("GameRules compound");
        rules.push(("fallingBlocks".into(), Tag::String("true".into())));
        rules.push(("randomTickSpeed".into(), Tag::String("3".into())));
    }
    fs::write(
        &path,
        folia_core::nbt::write_gzip_root(&with_extra).unwrap(),
    )
    .unwrap();

    let rules = GameRules {
        keep_inventory: true,
        ..GameRules::default()
    };
    let changed = level::persist_runtime_state(
        &path,
        RuntimeChange {
            spawn: Some((5, 66, -5)),
            game_rules: Some(rules.clone()),
        },
    )
    .unwrap();
    assert_eq!(changed.game_rules, rules);
    assert_eq!(changed.spawn, Some((5, 66, -5)));

    let on_disk = level_nbt_root(&path);
    let written = on_disk.get("Data").expect("Data compound");
    let written_rules = written.get("GameRules").expect("GameRules compound");
    assert_eq!(
        written_rules.get("fallingBlocks"),
        Some(&Tag::String("true".into()))
    );
    assert_eq!(
        written_rules.get("randomTickSpeed"),
        Some(&Tag::String("3".into()))
    );
    assert_eq!(
        written_rules.get("keepInventory"),
        Some(&Tag::String("true".into()))
    );
    assert_eq!(
        written_rules.get("doTileDrops"),
        Some(&Tag::String("true".into()))
    );

    let old = level::read(root.join("level.dat_old")).unwrap();
    assert_eq!(old.spawn, Some((1, 65, 2)));
    assert_eq!(old.game_rules, GameRules::default());

    fs::remove_dir_all(root).ok();
}

/// An orderly shutdown after the runtime writes must not roll them back: the
/// shutdown hook only advances time and leaves game rules and spawn intact.
#[test]
fn shutdown_record_keeps_runtime_changes() {
    let root = temp_world("shutdown");
    let path = root.join("level.dat");
    level::write_new_world(&path, "Shutdown", 5, (0, 64, 0)).unwrap();

    let rules = GameRules {
        mob_griefing: false,
        ..GameRules::default()
    };
    level::persist_game_rules(&path, &rules).unwrap();
    level::set_spawn(&path, (33, 90, -17)).unwrap();

    let final_level = level::record_session(&path, 600, None).unwrap();
    assert_eq!(final_level.day_time, 600);
    assert_eq!(final_level.spawn, Some((33, 90, -17)));
    assert_eq!(final_level.game_rules, rules);
    assert_eq!(level::read(&path).unwrap().game_rules, rules);

    fs::remove_dir_all(root).ok();
}
