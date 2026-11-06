//! Focused tests for the goal selector, navigation, and AI tick driver.

use folia_core::entity_ai::{
    ActorKind, ActorView, AiAgent, AiDriver, AiEvent, AiMob, FloatGoal, Goal, GoalContext,
    GoalFlags, GoalSelector, LookAtPlayerGoal, LookControl, MemoryWorld, PathFailure,
    RandomStrollGoal, WorldView, find_path,
};
use folia_core::math::{BlockPos, Vec3};
use std::cell::RefCell;
use std::rc::Rc;

struct ProbeGoal {
    label: &'static str,
    flags: GoalFlags,
    usable: bool,
    continue_ok: bool,
    log: Rc<RefCell<Vec<String>>>,
}

impl ProbeGoal {
    fn new(
        label: &'static str,
        flags: GoalFlags,
        usable: bool,
        continue_ok: bool,
        log: Rc<RefCell<Vec<String>>>,
    ) -> Self {
        Self {
            label,
            flags,
            usable,
            continue_ok,
            log,
        }
    }
}

impl Goal for ProbeGoal {
    fn name(&self) -> &'static str {
        self.label
    }

    fn flags(&self) -> GoalFlags {
        self.flags
    }

    fn can_use(&mut self, _ctx: &mut GoalContext<'_>) -> bool {
        self.usable
    }

    fn can_continue_to_use(&mut self, _ctx: &mut GoalContext<'_>) -> bool {
        self.continue_ok
    }

    fn start(&mut self, _ctx: &mut GoalContext<'_>) {
        self.log.borrow_mut().push(format!("start:{}", self.label));
    }

    fn stop(&mut self, _ctx: &mut GoalContext<'_>) {
        self.log.borrow_mut().push(format!("stop:{}", self.label));
    }
}

struct OneShotGoal {
    started: bool,
    log: Rc<RefCell<Vec<String>>>,
}

impl Goal for OneShotGoal {
    fn name(&self) -> &'static str {
        "OneShotGoal"
    }

    fn flags(&self) -> GoalFlags {
        GoalFlags::MOVE
    }

    fn can_use(&mut self, _ctx: &mut GoalContext<'_>) -> bool {
        !self.started
    }

    fn can_continue_to_use(&mut self, _ctx: &mut GoalContext<'_>) -> bool {
        false
    }

    fn start(&mut self, _ctx: &mut GoalContext<'_>) {
        self.started = true;
        self.log.borrow_mut().push("start:stopper".to_owned());
    }

    fn stop(&mut self, _ctx: &mut GoalContext<'_>) {
        self.log.borrow_mut().push("stop:stopper".to_owned());
    }
}

fn test_world() -> MemoryWorld {
    let mut world = MemoryWorld::flat(-64, 320, 63, 32);
    world.set_default_biome("minecraft:plains");
    world.set_default_sky_light(0);
    world.set_default_block_light(0);
    world
}

fn test_mob() -> AiMob {
    AiMob::new(1, ActorKind::Zombie, Vec3::new(0.5, 64.0, 0.5), 12345)
}

fn tick_selector(
    selector: &mut GoalSelector,
    mob: &mut AiMob,
    world: &dyn WorldView,
    look: &mut LookControl,
    game_time: i64,
) {
    let actors: Vec<ActorView> = Vec::new();
    let mut events = Vec::new();
    let mut ctx = GoalContext {
        mob,
        world,
        actors: &actors,
        look,
        events: &mut events,
        game_time,
    };
    selector.tick(&mut ctx);
}

#[test]
fn priority_replacement_and_flag_exclusion_follow_vanilla() {
    let world = test_world();
    let mut mob = test_mob();
    let mut look = LookControl::new();
    let log = Rc::new(RefCell::new(Vec::new()));

    let mut selector = GoalSelector::new();
    selector.add_goal(
        5,
        Box::new(ProbeGoal::new(
            "low",
            GoalFlags::MOVE,
            true,
            true,
            log.clone(),
        )),
    );
    selector.add_goal(
        1,
        Box::new(ProbeGoal::new(
            "high",
            GoalFlags::MOVE,
            true,
            true,
            log.clone(),
        )),
    );
    selector.add_goal(
        4,
        Box::new(ProbeGoal::new(
            "look",
            GoalFlags::LOOK,
            true,
            true,
            log.clone(),
        )),
    );

    tick_selector(&mut selector, &mut mob, &world, &mut look, 1);

    assert_eq!(selector.running_names(), vec!["high", "look"]);
    let log_entries = log.borrow().clone();
    assert!(log_entries.contains(&"start:low".to_owned()));
    assert!(log_entries.contains(&"stop:low".to_owned()));
    assert!(log_entries.contains(&"start:high".to_owned()));
    assert!(log_entries.contains(&"start:look".to_owned()));

    selector.disable_control_flag(GoalFlags::MOVE);
    tick_selector(&mut selector, &mut mob, &world, &mut look, 2);

    assert_eq!(selector.running_names(), vec!["look"]);
    assert!(log.borrow().contains(&"stop:high".to_owned()));
}

#[test]
fn goals_start_and_stop_on_can_use_and_can_continue_transitions() {
    let world = test_world();
    let mut mob = test_mob();
    let mut look = LookControl::new();
    let log = Rc::new(RefCell::new(Vec::new()));

    let mut selector = GoalSelector::new();
    selector.add_goal(
        1,
        Box::new(OneShotGoal {
            started: false,
            log: log.clone(),
        }),
    );
    selector.add_goal(
        2,
        Box::new(ProbeGoal::new(
            "blocked",
            GoalFlags::LOOK,
            false,
            true,
            log.clone(),
        )),
    );

    tick_selector(&mut selector, &mut mob, &world, &mut look, 1);
    assert_eq!(selector.running_names(), vec!["OneShotGoal"]);
    assert!(log.borrow().contains(&"start:stopper".to_owned()));
    assert!(!log.borrow().contains(&"start:blocked".to_owned()));

    tick_selector(&mut selector, &mut mob, &world, &mut look, 2);
    assert!(selector.running_names().is_empty());
    assert!(log.borrow().contains(&"stop:stopper".to_owned()));
}

#[test]
fn path_detours_around_a_wall() {
    let mut world = test_world();
    for z in -2..=2 {
        for y in 64..=65 {
            world.set_block_name(2, y, z, "minecraft:stone");
        }
    }

    let path = find_path(
        &world,
        BlockPos::new(0, 64, 0),
        BlockPos::new(5, 64, 0),
        &Default::default(),
    )
    .expect("wall should be bypassed");

    assert_eq!(path.points().first(), Some(&BlockPos::new(0, 64, 0)));
    assert_eq!(path.end(), Some(BlockPos::new(5, 64, 0)));
    assert!(
        path.points()
            .iter()
            .all(|point| !(point.x() == 2 && point.z().abs() <= 2))
    );
    assert!(path.points().iter().any(|point| point.z().abs() >= 3));
}

#[test]
fn path_steps_up_one_block_at_a_time() {
    let mut world = test_world();
    for x in 1..=6 {
        world.set_block_name(x, 64, 0, "minecraft:stone");
    }

    let path = find_path(
        &world,
        BlockPos::new(0, 64, 0),
        BlockPos::new(6, 65, 0),
        &Default::default(),
    )
    .expect("raised platform is reachable");

    assert_eq!(path.end(), Some(BlockPos::new(6, 65, 0)));
    assert!(path.points().iter().any(|point| point.y() == 65));
    for pair in path.points().windows(2) {
        assert!(
            (pair[1].y() - pair[0].y()).abs() <= 1,
            "path must not jump more than one block: {pair:?}"
        );
    }
}

#[test]
fn unreachable_and_too_large_paths_fail_explicitly() {
    let mut world = test_world();
    for x in -2_i32..=2 {
        for z in -2_i32..=2 {
            if x.abs() == 2 || z.abs() == 2 {
                for y in 64..=65 {
                    world.set_block_name(x, y, z, "minecraft:stone");
                }
            }
        }
    }

    let unreachable = find_path(
        &world,
        BlockPos::new(0, 64, 0),
        BlockPos::new(6, 64, 0),
        &Default::default(),
    );
    assert_eq!(unreachable, Err(PathFailure::Unreachable));

    let too_large = find_path(
        &world,
        BlockPos::new(0, 64, 0),
        BlockPos::new(10, 64, 0),
        &folia_core::entity_ai::PathOptions {
            max_range: 4.0,
            ..Default::default()
        },
    );
    assert_eq!(too_large, Err(PathFailure::TooLarge));
}

#[test]
fn navigation_follows_a_path_to_its_target() {
    let world = test_world();
    let mut mob = test_mob();
    let AiMob {
        navigation, pos, ..
    } = &mut mob;

    assert!(navigation.move_to(&world, BlockPos::new(0, 64, 0), BlockPos::new(4, 64, 0)));
    for _ in 0..200 {
        if navigation.is_done() {
            break;
        }
        navigation.tick(pos);
    }

    assert!(navigation.is_done());
    assert!((pos.x() - 4.5).abs() < 1.0e-6);
    assert!((pos.z() - 0.5).abs() < 1.0e-6);
    assert!((pos.y() - 64.0).abs() < 1.0e-6);
}

#[test]
fn random_stroll_is_deterministic_for_a_fixed_seed() {
    let world = test_world();
    let mut first = AiAgent::new(AiMob::new(
        1,
        ActorKind::Zombie,
        Vec3::new(0.5, 64.0, 0.5),
        99,
    ));
    first
        .goal_selector
        .add_goal(1, Box::new(RandomStrollGoal::with_interval(1.0, 1)));
    let mut second = AiAgent::new(AiMob::new(
        1,
        ActorKind::Zombie,
        Vec3::new(0.5, 64.0, 0.5),
        99,
    ));
    second
        .goal_selector
        .add_goal(1, Box::new(RandomStrollGoal::with_interval(1.0, 1)));

    let mut first_events = Vec::new();
    let mut second_events = Vec::new();
    for time in 1..=80 {
        first.tick(&world, &[], time, &mut first_events);
        second.tick(&world, &[], time, &mut second_events);
    }

    assert_eq!(first.mob.pos, second.mob.pos);
    assert_eq!(
        first.mob.navigation.path().map(|path| path.points()),
        second.mob.navigation.path().map(|path| path.points())
    );
}

#[test]
fn hurt_by_target_then_melee_attack_damages_the_player() {
    let world = test_world();
    let mut driver = AiDriver::new();
    let zombie = driver.add_zombie(1, Vec3::new(0.5, 64.0, 0.5), 7);
    driver.add_player(ActorView::player(99, Vec3::new(3.5, 64.0, 0.5)));
    {
        let agent = driver.agent_mut(zombie).expect("zombie agent");
        agent.mob.last_hurt_by = Some(99);
        agent.mob.last_hurt_by_timestamp = 1;
    }

    let mut attacked = false;
    for _ in 0..80 {
        let events = driver.tick(&world);
        if events.iter().any(|event| {
            matches!(
                event,
                AiEvent::Attack {
                    attacker: 1,
                    target: 99,
                    ..
                }
            )
        }) {
            attacked = true;
            break;
        }
    }

    assert!(attacked, "melee goal should attack the hurt-by target");
    assert_eq!(driver.agent(zombie).unwrap().mob.target, Some(99));
    let player = driver.actors.iter().find(|actor| actor.id == 99).unwrap();
    assert!(player.health < 20.0);
}

#[test]
fn look_at_player_sets_the_look_control_target() {
    let world = test_world();
    let mut agent = AiAgent::new(test_mob());
    agent
        .goal_selector
        .add_goal(1, Box::new(LookAtPlayerGoal::with_probability(8.0, 1.0)));
    let actors = vec![ActorView::player(99, Vec3::new(3.5, 64.0, 0.5))];

    let mut events = Vec::new();
    agent.tick(&world, &actors, 1, &mut events);

    assert!(agent.look.is_looking_at_target());
    let wanted = agent.look.wanted().expect("look target");
    assert!((wanted.x() - 3.5).abs() < 1.0e-6);
}

#[test]
fn float_goal_jumps_in_water() {
    let mut world = test_world();
    world.set_block_name(0, 64, 0, "minecraft:water");
    world.set_block_name(0, 65, 0, "minecraft:water");
    let mut agent = AiAgent::new(test_mob());
    agent.goal_selector.add_goal(0, Box::new(FloatGoal::new()));

    let mut events = Vec::new();
    for time in 1..=10 {
        agent.tick(&world, &[], time, &mut events);
    }

    assert!(agent.mob.jump_requests > 0);
    assert!(agent.mob.in_water);
}
