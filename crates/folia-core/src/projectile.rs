//! Session-free projectile model.

use crate::block_shapes::Aabb;
use crate::entity::Entity;
use crate::registry;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectileKind {
    Arrow,
    Snowball,
    Egg,
}

impl ProjectileKind {
    pub fn base_damage(self) -> f32 {
        match self {
            Self::Arrow => 2.0,
            Self::Snowball | Self::Egg => 0.0,
        }
    }

    pub fn gravity(self) -> f64 {
        match self {
            Self::Arrow => 0.05,
            Self::Snowball | Self::Egg => 0.03,
        }
    }

    pub fn drag(self) -> f64 {
        0.99
    }

    pub fn entity_type_id(self) -> i32 {
        let name = match self {
            Self::Arrow => "minecraft:arrow",
            Self::Snowball => "minecraft:snowball",
            Self::Egg => "minecraft:egg",
        };
        registry::entity_type_id(name).unwrap_or(match self {
            Self::Arrow => 9,
            Self::Snowball => 73,
            Self::Egg => 62,
        })
    }

    pub fn despawn_ticks(self) -> i32 {
        1200
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileTick {
    pub moved: bool,
    pub hit_block: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectileHit {
    pub target_id: i32,
    pub damage: f32,
    pub critical: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Projectile {
    pub id: i32,
    pub uuid: [u8; 16],
    pub kind: ProjectileKind,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity: [f64; 3],
    pub age: i32,
    pub stuck: bool,
    pub critical: bool,
    pub owner_id: Option<i32>,
}

impl Projectile {
    pub fn spawn(
        kind: ProjectileKind,
        id: i32,
        owner_id: Option<i32>,
        x: f64,
        y: f64,
        z: f64,
        velocity: [f64; 3],
    ) -> Self {
        let mut uuid = [0u8; 16];
        uuid[0..4].copy_from_slice(&id.to_le_bytes());
        Self {
            id,
            uuid,
            kind,
            x,
            y,
            z,
            velocity,
            age: 0,
            stuck: false,
            critical: false,
            owner_id,
        }
    }

    pub fn tick(&mut self, collider: &mut impl crate::entity::BlockCollider) -> ProjectileTick {
        if self.stuck {
            self.age += 1;
            return ProjectileTick {
                moved: false,
                hit_block: false,
            };
        }

        // Gravity is applied before drag so a projectile starting from rest
        // falls 0.05 and then is scaled by 0.99, matching the model tests.
        self.velocity[1] -= self.kind.gravity();
        for component in &mut self.velocity {
            *component *= self.kind.drag();
        }

        let before = [self.x, self.y, self.z];
        let mut pos = [self.x, self.y, self.z];
        let mut vel = self.velocity;
        let blocked_x = move_axis(&mut pos, &mut vel, 0.5, 0.5, 0, collider);
        let blocked_y = move_axis(&mut pos, &mut vel, 0.5, 0.5, 1, collider);
        let blocked_z = move_axis(&mut pos, &mut vel, 0.5, 0.5, 2, collider);
        let hit_block = blocked_x || blocked_y || blocked_z;

        self.x = pos[0];
        self.y = pos[1];
        self.z = pos[2];
        self.velocity = vel;
        if hit_block {
            self.velocity = [0.0; 3];
            self.stuck = true;
        }

        self.age += 1;
        ProjectileTick {
            moved: pos != before,
            hit_block,
        }
    }

    pub fn hit_damage(&self, speed: f64) -> f32 {
        match self.kind {
            ProjectileKind::Arrow => {
                // `critical` doubles the arrow damage in this simplified model.
                let damage = (speed * 2.0).ceil() as i32 as f32;
                if self.critical { damage * 2.0 } else { damage }
            }
            ProjectileKind::Snowball | ProjectileKind::Egg => 0.0,
        }
    }

    pub fn should_despawn(&self) -> bool {
        self.age >= self.kind.despawn_ticks()
    }

    pub fn try_hit(&mut self, targets: &[(i32, Aabb)]) -> Option<ProjectileHit> {
        if self.stuck {
            return None;
        }
        let speed = (self.velocity[0] * self.velocity[0]
            + self.velocity[1] * self.velocity[1]
            + self.velocity[2] * self.velocity[2])
            .sqrt();
        for (target_id, aabb) in targets {
            if self.x >= aabb.min_x - 0.3
                && self.x <= aabb.max_x + 0.3
                && self.y >= aabb.min_y - 0.3
                && self.y <= aabb.max_y + 0.3
                && self.z >= aabb.min_z - 0.3
                && self.z <= aabb.max_z + 0.3
            {
                let damage = self.hit_damage(speed);
                self.stuck = true;
                self.velocity = [0.0; 3];
                return Some(ProjectileHit {
                    target_id: *target_id,
                    damage,
                    critical: self.critical,
                });
            }
        }
        None
    }

    fn base_entity(&self) -> Entity {
        let mut entity = Entity::new(
            self.id,
            self.uuid,
            self.kind.entity_type_id(),
            self.x,
            self.y,
            self.z,
        );
        entity.width = 0.5;
        entity.height = 0.5;
        entity.velocity = self.velocity;
        entity
    }

    pub fn add_packet(&self) -> Vec<u8> {
        crate::entity::add_entity_packet(&self.base_entity())
    }

    pub fn metadata_packet(&self) -> Vec<u8> {
        crate::entity_metadata::EntityMetadata::new(self.id).all_packet()
    }

    pub fn position_sync_packet(&self) -> Vec<u8> {
        crate::entity::entity_position_sync_packet(&self.base_entity())
    }
}

fn move_axis(
    pos: &mut [f64; 3],
    velocity: &mut [f64; 3],
    width: f64,
    height: f64,
    axis: u8,
    collider: &mut impl crate::entity::BlockCollider,
) -> bool {
    let amount = velocity[axis as usize];
    if amount == 0.0 {
        return false;
    }
    let start = Aabb::player(pos[0], pos[1], pos[2], width, height);
    let sweep = match axis {
        0 => start.expand_towards(amount, 0.0, 0.0),
        1 => start.expand_towards(0.0, amount, 0.0),
        _ => start.expand_towards(0.0, 0.0, amount),
    };
    let x0 = (sweep.min_x - 1.0e-7).floor() as i32 - 1;
    let x1 = (sweep.max_x + 1.0e-7).floor() as i32 + 1;
    let y0 = (sweep.min_y - 1.0e-7).floor() as i32 - 1;
    let y1 = (sweep.max_y + 1.0e-7).floor() as i32 + 1;
    let z0 = (sweep.min_z - 1.0e-7).floor() as i32 - 1;
    let z1 = (sweep.max_z + 1.0e-7).floor() as i32 + 1;
    let mut allowed = amount;
    for bx in x0..=x1 {
        for by in y0..=y1 {
            for bz in z0..=z1 {
                for block in collider.collision_boxes(bx, by, bz) {
                    if !block.intersects(&sweep) {
                        continue;
                    }
                    match axis {
                        0 => {
                            if amount > 0.0
                                && start.max_x <= block.min_x + 1.0e-7
                                && start.max_x + amount > block.min_x
                            {
                                allowed = allowed.min(block.min_x - start.max_x);
                            } else if amount < 0.0
                                && start.min_x >= block.max_x - 1.0e-7
                                && start.min_x + amount < block.max_x
                            {
                                allowed = allowed.max(block.max_x - start.min_x);
                            }
                        }
                        1 => {
                            if amount > 0.0
                                && start.max_y <= block.min_y + 1.0e-7
                                && start.max_y + amount > block.min_y
                            {
                                allowed = allowed.min(block.min_y - start.max_y);
                            } else if amount < 0.0
                                && start.min_y >= block.max_y - 1.0e-7
                                && start.min_y + amount < block.max_y
                            {
                                allowed = allowed.max(block.max_y - start.min_y);
                            }
                        }
                        _ => {
                            if amount > 0.0
                                && start.max_z <= block.min_z + 1.0e-7
                                && start.max_z + amount > block.min_z
                            {
                                allowed = allowed.min(block.min_z - start.max_z);
                            } else if amount < 0.0
                                && start.min_z >= block.max_z - 1.0e-7
                                && start.min_z + amount < block.max_z
                            {
                                allowed = allowed.max(block.max_z - start.min_z);
                            }
                        }
                    }
                }
            }
        }
    }
    let blocked = allowed != amount;
    if blocked {
        velocity[axis as usize] = 0.0;
    }
    pos[axis as usize] += allowed;
    blocked
}
