//! Crafting grid matching, furnace state, stonecutting, and smithing.
//!
//! The typed recipe catalog lives in [`crate::loot`]; this module only matches
//! it against inventories and advances container state. Matching follows the
//! 1.21.11 Java implementation:
//!
//! * `CraftingInput` is cropped to the tight bounding box of its non-empty
//!   slots before a shaped pattern is tested, so a 2x2 recipe matches at every
//!   offset inside a 3x3 grid.
//! * `ShapedRecipePattern` is itself shrunk to the bounding box of its
//!   non-space symbols, checks the normalized input size, then tries the
//!   mirrored pattern before the plain one.
//! * Shapeless recipes are matched as a multiset: every slot contributes one
//!   item, each ingredient consumes one matching item, and duplicate
//!   ingredients therefore need duplicate items.
//! * `RecipeManager.getRecipeFor` returns the *last* matching recipe in
//!   resource-location order, which is the rule used by every lookup here.
//!
//! Special crafting recipes (`crafting_special_*`), decorated pots, trims, and
//! opaque recipe types have no implemented transform. They are skipped by the
//! automatic lookup and fail loudly with [`CraftingError::UnsupportedRecipe`]
//! when addressed by id.
//!
//! This is the state model menus and block entities will drive; it does not
//! encode packets, consume fuel remainders other than lava buckets, or model
//! brewing.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::fmt;

use crate::inventory::{ItemStack, add_stack};
use crate::loot::{
    CookingKind, Ingredient, Recipe, RecipeCatalog, RecipeKind, ShapedRecipe, ShapelessRecipe,
    TransmuteRecipe,
};

/// Grids are at most 3x3, matching `CraftingInput` and `ShapedRecipePattern`.
pub const MAX_GRID_DIMENSION: usize = 3;

pub const FURNACE_INPUT_SLOT: usize = 0;
pub const FURNACE_FUEL_SLOT: usize = 1;
pub const FURNACE_RESULT_SLOT: usize = 2;

/// Vanilla's baseline fuel unit: one smelting operation is 200 ticks.
const SMELTING_TIME: i32 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CraftingError {
    /// No recipe in the catalog matches the supplied input.
    NoRecipe,
    UnknownRecipe(String),
    /// The addressed recipe exists but this port has no transform for it.
    UnsupportedRecipe {
        id: String,
        kind: String,
    },
    /// A recipe for another station was used from this entry point.
    WrongStation {
        id: String,
        kind: String,
        expected: &'static str,
    },
    /// The addressed recipe exists but does not match the supplied input.
    RecipeDoesNotMatch(String),
    InvalidGrid {
        width: usize,
        height: usize,
        slots: usize,
    },
    InvalidPattern {
        id: String,
        reason: String,
    },
    /// The result slot cannot accept another item.
    OutputFull,
}

impl fmt::Display for CraftingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRecipe => write!(formatter, "no matching recipe"),
            Self::UnknownRecipe(id) => write!(formatter, "unknown recipe `{id}`"),
            Self::UnsupportedRecipe { id, kind } => {
                write!(formatter, "recipe `{id}` uses unsupported type `{kind}`")
            }
            Self::WrongStation { id, kind, expected } => {
                write!(
                    formatter,
                    "recipe `{id}` ({kind}) must be used at {expected}"
                )
            }
            Self::RecipeDoesNotMatch(id) => {
                write!(formatter, "recipe `{id}` does not match the supplied input")
            }
            Self::InvalidGrid {
                width,
                height,
                slots,
            } => write!(
                formatter,
                "invalid crafting grid: {width}x{height} with {slots} slots"
            ),
            Self::InvalidPattern { id, reason } => {
                write!(
                    formatter,
                    "recipe `{id}` has an invalid shaped pattern: {reason}"
                )
            }
            Self::OutputFull => write!(formatter, "the result slot is full"),
        }
    }
}

impl std::error::Error for CraftingError {}

/// A tight bounding box around the non-empty slots of a grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridBounds {
    pub min_x: usize,
    pub min_y: usize,
    pub width: usize,
    pub height: usize,
}

/// A 1x1 through 3x3 crafting grid in row-major order.
///
/// Slot indices are `y * width + x`, the same order as `CraftingInput`.
#[derive(Debug, Clone, PartialEq)]
pub struct CraftingGrid {
    width: usize,
    height: usize,
    slots: Vec<ItemStack>,
}

impl CraftingGrid {
    pub fn new(width: usize, height: usize, slots: Vec<ItemStack>) -> Result<Self, CraftingError> {
        if width == 0 || height == 0 || width > MAX_GRID_DIMENSION || height > MAX_GRID_DIMENSION {
            return Err(CraftingError::InvalidGrid {
                width,
                height,
                slots: slots.len(),
            });
        }
        if slots.len() != width * height {
            return Err(CraftingError::InvalidGrid {
                width,
                height,
                slots: slots.len(),
            });
        }
        Ok(Self {
            width,
            height,
            slots,
        })
    }

    pub fn empty(width: usize, height: usize) -> Result<Self, CraftingError> {
        Self::new(width, height, vec![ItemStack::empty(); width * height])
    }

    pub fn two_by_two() -> Self {
        Self::empty(2, 2).expect("a 2x2 grid is always valid")
    }

    pub fn three_by_three() -> Self {
        Self::empty(3, 3).expect("a 3x3 grid is always valid")
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn slots(&self) -> &[ItemStack] {
        &self.slots
    }

    pub fn slots_mut(&mut self) -> &mut [ItemStack] {
        &mut self.slots
    }

    pub fn slot(&self, index: usize) -> Option<&ItemStack> {
        self.slots.get(index)
    }

    pub fn slot_mut(&mut self, index: usize) -> Option<&mut ItemStack> {
        self.slots.get_mut(index)
    }

    pub fn set_slot(&mut self, index: usize, stack: ItemStack) -> bool {
        match self.slots.get_mut(index) {
            Some(slot) => {
                *slot = stack;
                true
            }
            None => false,
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Option<&ItemStack> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.slots.get(y * self.width + x)
    }

    pub fn set(&mut self, x: usize, y: usize, stack: ItemStack) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        self.slots[y * self.width + x] = stack;
        true
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(ItemStack::is_empty)
    }

    /// The number of non-empty slots. This is vanilla's `ingredientCount`.
    pub fn ingredient_count(&self) -> usize {
        self.slots.iter().filter(|stack| !stack.is_empty()).count()
    }

    /// The tight bounding box around non-empty slots, or `None` for an empty
    /// grid. `CraftingInput.ofPositioned` performs the same crop.
    pub fn tight_bounds(&self) -> Option<GridBounds> {
        let mut min_x = self.width;
        let mut min_y = self.height;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        let mut found = false;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.slots[y * self.width + x].is_empty() {
                    continue;
                }
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
        found.then_some(GridBounds {
            min_x,
            min_y,
            width: max_x - min_x + 1,
            height: max_y - min_y + 1,
        })
    }

    /// The last matching crafting recipe in catalog order, which is vanilla's
    /// `RecipeManager.getRecipeFor` priority.
    pub fn find_recipe<'a>(
        &self,
        catalog: &'a RecipeCatalog,
    ) -> Result<Option<&'a Recipe>, CraftingError> {
        find_crafting_recipe(catalog, self)
    }

    /// Find and apply an automatic crafting recipe, consuming one item from
    /// every non-empty slot.
    pub fn craft(&mut self, catalog: &RecipeCatalog) -> Result<CraftOutput, CraftingError> {
        let recipe = self
            .find_recipe(catalog)?
            .ok_or(CraftingError::NoRecipe)?
            .clone();
        self.apply_crafting_recipe(&recipe)
    }

    /// Apply a named crafting recipe. Unsupported recipe types error instead of
    /// silently producing nothing.
    pub fn craft_recipe(
        &mut self,
        catalog: &RecipeCatalog,
        id: &str,
    ) -> Result<CraftOutput, CraftingError> {
        let recipe = catalog
            .recipe(id)
            .ok_or_else(|| CraftingError::UnknownRecipe(id.to_owned()))?
            .clone();
        require_crafting_kind(&recipe)?;
        if !matches_crafting_recipe(&recipe, self)? {
            return Err(CraftingError::RecipeDoesNotMatch(recipe.id.clone()));
        }
        self.apply_crafting_recipe(&recipe)
    }

    fn apply_crafting_recipe(&mut self, recipe: &Recipe) -> Result<CraftOutput, CraftingError> {
        let result = match &recipe.kind {
            RecipeKind::Shaped(shaped) => shaped.result.clone(),
            RecipeKind::Shapeless(shapeless) => shapeless.result.clone(),
            RecipeKind::Transmute(transmute) => assemble_transmute(transmute, self)
                .ok_or_else(|| CraftingError::RecipeDoesNotMatch(recipe.id.clone()))?,
            RecipeKind::Cooking(_) => {
                return Err(CraftingError::WrongStation {
                    id: recipe.id.clone(),
                    kind: recipe_kind_name(&recipe.kind),
                    expected: "a furnace",
                });
            }
            RecipeKind::Stonecutting(_) => {
                return Err(CraftingError::WrongStation {
                    id: recipe.id.clone(),
                    kind: recipe_kind_name(&recipe.kind),
                    expected: "a stonecutter",
                });
            }
            RecipeKind::SmithingTransform(_) | RecipeKind::SmithingTrim(_) => {
                return Err(CraftingError::WrongStation {
                    id: recipe.id.clone(),
                    kind: recipe_kind_name(&recipe.kind),
                    expected: "a smithing table",
                });
            }
            RecipeKind::DecoratedPot | RecipeKind::Special { .. } | RecipeKind::Opaque { .. } => {
                return Err(CraftingError::UnsupportedRecipe {
                    id: recipe.id.clone(),
                    kind: recipe_kind_name(&recipe.kind),
                });
            }
        };
        let consumed_slots = self.consume_one_each();
        Ok(CraftOutput {
            recipe: recipe.id.clone(),
            result,
            consumed_slots,
        })
    }

    fn consume_one_each(&mut self) -> Vec<usize> {
        let mut consumed = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_empty() {
                continue;
            }
            slot.shrink(1);
            consumed.push(index);
        }
        consumed
    }
}

/// A recipe output plus bookkeeping needed by a menu/block entity.
#[derive(Debug, Clone, PartialEq)]
pub struct CraftOutput {
    pub recipe: String,
    pub result: ItemStack,
    pub consumed_slots: Vec<usize>,
}

impl CraftOutput {
    /// Insert the crafted stack with the shared inventory rules. Returns the
    /// part that did not fit.
    pub fn deposit(self, inventory: &mut [ItemStack]) -> ItemStack {
        add_stack(inventory, self.result)
    }
}

/// Find the last crafting recipe matching `grid` in sorted catalog order.
///
/// `RecipeManager.getRecipeFor` takes `list.getLast()` over a resource-location
/// sorted stream, so lexicographically later recipe ids win conflicts.
pub fn find_crafting_recipe<'a>(
    catalog: &'a RecipeCatalog,
    grid: &CraftingGrid,
) -> Result<Option<&'a Recipe>, CraftingError> {
    let mut best = None;
    for (_, recipe) in catalog.recipes() {
        if matches_crafting_recipe(recipe, grid)? {
            best = Some(recipe);
        }
    }
    Ok(best)
}

/// Test one recipe against a grid without consuming anything.
///
/// Non-crafting recipes return `false`; special/decorated/opaque recipes are
/// also `false` here because they have no data-driven matcher. Explicitly
/// addressing one through [`CraftingGrid::craft_recipe`] raises
/// [`CraftingError::UnsupportedRecipe`] instead.
pub fn matches_crafting_recipe(
    recipe: &Recipe,
    grid: &CraftingGrid,
) -> Result<bool, CraftingError> {
    match &recipe.kind {
        RecipeKind::Shaped(shaped) => {
            let pattern = NormalizedPattern::from_recipe(&recipe.id, shaped)?;
            Ok(pattern.matches(grid))
        }
        RecipeKind::Shapeless(shapeless) => Ok(shapeless_matches(shapeless, grid)),
        RecipeKind::Transmute(transmute) => Ok(transmute_matches(transmute, grid)),
        RecipeKind::Cooking(_)
        | RecipeKind::Stonecutting(_)
        | RecipeKind::SmithingTransform(_)
        | RecipeKind::SmithingTrim(_)
        | RecipeKind::DecoratedPot
        | RecipeKind::Special { .. }
        | RecipeKind::Opaque { .. } => Ok(false),
    }
}

/// Reject recipe types that cannot be produced by this module before a matcher
/// is attempted, so unsupported types fail loudly rather than looking like a
/// non-match.
fn require_crafting_kind(recipe: &Recipe) -> Result<(), CraftingError> {
    match &recipe.kind {
        RecipeKind::Shaped(_) | RecipeKind::Shapeless(_) | RecipeKind::Transmute(_) => Ok(()),
        RecipeKind::Cooking(_) => Err(CraftingError::WrongStation {
            id: recipe.id.clone(),
            kind: recipe_kind_name(&recipe.kind),
            expected: "a furnace",
        }),
        RecipeKind::Stonecutting(_) => Err(CraftingError::WrongStation {
            id: recipe.id.clone(),
            kind: recipe_kind_name(&recipe.kind),
            expected: "a stonecutter",
        }),
        RecipeKind::SmithingTransform(_) | RecipeKind::SmithingTrim(_) => {
            Err(CraftingError::WrongStation {
                id: recipe.id.clone(),
                kind: recipe_kind_name(&recipe.kind),
                expected: "a smithing table",
            })
        }
        RecipeKind::DecoratedPot | RecipeKind::Special { .. } | RecipeKind::Opaque { .. } => {
            Err(CraftingError::UnsupportedRecipe {
                id: recipe.id.clone(),
                kind: recipe_kind_name(&recipe.kind),
            })
        }
    }
}

fn recipe_kind_name(kind: &RecipeKind) -> String {
    match kind {
        RecipeKind::Shaped(_) => "minecraft:crafting_shaped".to_owned(),
        RecipeKind::Shapeless(_) => "minecraft:crafting_shapeless".to_owned(),
        RecipeKind::Cooking(cooking) => cooking.cooking.id().to_owned(),
        RecipeKind::Stonecutting(_) => "minecraft:stonecutting".to_owned(),
        RecipeKind::SmithingTransform(_) => "minecraft:smithing_transform".to_owned(),
        RecipeKind::SmithingTrim(_) => "minecraft:smithing_trim".to_owned(),
        RecipeKind::Transmute(_) => "minecraft:crafting_transmute".to_owned(),
        RecipeKind::DecoratedPot => "minecraft:crafting_decorated_pot".to_owned(),
        RecipeKind::Special { kind } | RecipeKind::Opaque { kind, .. } => kind.clone(),
    }
}

/// A shaped pattern after `ShapedRecipePattern.shrink` and key validation.
#[derive(Debug, Clone, PartialEq)]
struct NormalizedPattern {
    width: usize,
    height: usize,
    ingredients: Vec<Option<Ingredient>>,
    ingredient_count: usize,
    symmetrical: bool,
}

impl NormalizedPattern {
    fn from_recipe(id: &str, recipe: &ShapedRecipe) -> Result<Self, CraftingError> {
        let pattern: Vec<Vec<char>> = recipe
            .pattern
            .iter()
            .map(|row| row.chars().collect())
            .collect();
        if pattern.is_empty() {
            return Err(invalid_pattern(id, "an empty pattern is not allowed"));
        }
        if pattern.len() > MAX_GRID_DIMENSION {
            return Err(invalid_pattern(id, "more than 3 rows"));
        }
        let row_width = pattern[0].len();
        if row_width == 0 {
            return Err(invalid_pattern(id, "an empty pattern row"));
        }
        if row_width > MAX_GRID_DIMENSION {
            return Err(invalid_pattern(id, "more than 3 columns"));
        }
        if pattern.iter().any(|row| row.len() != row_width) {
            return Err(invalid_pattern(id, "each row must be the same width"));
        }

        let mut min_x = row_width;
        let mut max_x = 0usize;
        let mut first_row = None;
        let mut last_row = None;
        for (y, row) in pattern.iter().enumerate() {
            let row_min = row.iter().position(|cell| *cell != ' ');
            let row_max = row.iter().rposition(|cell| *cell != ' ');
            let (Some(row_min), Some(row_max)) = (row_min, row_max) else {
                continue;
            };
            min_x = min_x.min(row_min);
            max_x = max_x.max(row_max);
            if first_row.is_none() {
                first_row = Some(y);
            }
            last_row = Some(y);
        }
        let (Some(first_row), Some(last_row)) = (first_row, last_row) else {
            return Err(invalid_pattern(id, "a pattern needs at least one symbol"));
        };

        let mut used = HashSet::new();
        let mut ingredients = Vec::new();
        for row in pattern.iter().take(last_row + 1).skip(first_row) {
            for cell in row.iter().take(max_x + 1).skip(min_x) {
                if *cell == ' ' {
                    ingredients.push(None);
                    continue;
                }
                let Some(ingredient) = recipe.key.get(cell) else {
                    return Err(invalid_pattern(
                        id,
                        &format!("pattern references undefined symbol `{cell}`"),
                    ));
                };
                used.insert(*cell);
                ingredients.push(Some(ingredient.clone()));
            }
        }
        if let Some(symbol) = recipe.key.keys().find(|symbol| !used.contains(symbol)) {
            return Err(invalid_pattern(
                id,
                &format!("key symbol `{symbol}` is not used in the pattern"),
            ));
        }

        let width = max_x - min_x + 1;
        let height = last_row - first_row + 1;
        let ingredient_count = ingredients.iter().filter(|entry| entry.is_some()).count();
        let symmetrical = is_symmetrical(width, height, &ingredients);
        Ok(Self {
            width,
            height,
            ingredients,
            ingredient_count,
            symmetrical,
        })
    }

    fn matches(&self, grid: &CraftingGrid) -> bool {
        let Some(bounds) = grid.tight_bounds() else {
            return false;
        };
        if grid.ingredient_count() != self.ingredient_count {
            return false;
        }
        if bounds.width != self.width || bounds.height != self.height {
            return false;
        }
        if !self.symmetrical && self.matches_at(grid, bounds, true) {
            return true;
        }
        self.matches_at(grid, bounds, false)
    }

    fn matches_at(&self, grid: &CraftingGrid, bounds: GridBounds, mirrored: bool) -> bool {
        for y in 0..self.height {
            for x in 0..self.width {
                let pattern_x = if mirrored { self.width - x - 1 } else { x };
                let expected = &self.ingredients[pattern_x + y * self.width];
                let stack = &grid.slots[(bounds.min_y + y) * grid.width + bounds.min_x + x];
                match expected {
                    Some(ingredient) => {
                        if stack.is_empty() || !ingredient.matches(&stack.item) {
                            return false;
                        }
                    }
                    None => {
                        if !stack.is_empty() {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

fn is_symmetrical(width: usize, height: usize, ingredients: &[Option<Ingredient>]) -> bool {
    for y in 0..height {
        for x in 0..width / 2 {
            let left = &ingredients[x + y * width];
            let right = &ingredients[width - x - 1 + y * width];
            if left != right {
                return false;
            }
        }
    }
    true
}

fn invalid_pattern(id: &str, reason: &str) -> CraftingError {
    CraftingError::InvalidPattern {
        id: id.to_owned(),
        reason: reason.to_owned(),
    }
}

fn shapeless_matches(recipe: &ShapelessRecipe, grid: &CraftingGrid) -> bool {
    let items: Vec<&str> = grid
        .slots
        .iter()
        .filter(|stack| !stack.is_empty())
        .map(|stack| stack.item.as_str())
        .collect();
    if items.len() != recipe.ingredients.len() {
        return false;
    }

    // Vanilla `StackedContents.tryPick` solves a bipartite matching rather than
    // greedily consuming the first matching item. That matters when a tag
    // ingredient and a concrete ingredient can both accept the same item:
    // `[#planks, oak_planks]` must assign the tag to birch and the concrete
    // ingredient to oak, even though oak appears first in the grid.
    let mut ingredient_for_item: Vec<Option<usize>> = vec![None; items.len()];
    for ingredient_index in 0..recipe.ingredients.len() {
        let mut visited = vec![false; items.len()];
        if !assign_ingredient(
            ingredient_index,
            &recipe.ingredients,
            &items,
            &mut ingredient_for_item,
            &mut visited,
        ) {
            return false;
        }
    }
    true
}

fn assign_ingredient(
    ingredient_index: usize,
    ingredients: &[Ingredient],
    items: &[&str],
    ingredient_for_item: &mut [Option<usize>],
    visited: &mut [bool],
) -> bool {
    for (item_index, item) in items.iter().enumerate() {
        if visited[item_index] || !ingredients[ingredient_index].matches(item) {
            continue;
        }
        visited[item_index] = true;
        let available = match ingredient_for_item[item_index] {
            Some(other) => {
                assign_ingredient(other, ingredients, items, ingredient_for_item, visited)
            }
            None => true,
        };
        if available {
            ingredient_for_item[item_index] = Some(ingredient_index);
            return true;
        }
    }
    false
}

fn transmute_matches(recipe: &TransmuteRecipe, grid: &CraftingGrid) -> bool {
    let stacks: Vec<&ItemStack> = grid
        .slots
        .iter()
        .filter(|stack| !stack.is_empty())
        .collect();
    if stacks.len() != 2 {
        return false;
    }
    let mut found_input = false;
    let mut found_material = false;
    for stack in stacks {
        if !found_input && recipe.input.matches(&stack.item) {
            if transmute_result(recipe, stack).same_item_and_components(stack) {
                return false;
            }
            found_input = true;
        } else if !found_material && recipe.material.matches(&stack.item) {
            found_material = true;
        } else {
            return false;
        }
    }
    found_input && found_material
}

fn assemble_transmute(recipe: &TransmuteRecipe, grid: &CraftingGrid) -> Option<ItemStack> {
    grid.slots
        .iter()
        .find(|stack| !stack.is_empty() && recipe.input.matches(&stack.item))
        .map(|stack| transmute_result(recipe, stack))
}

/// `TransmuteResult.apply`: keep the input's component patch, then overlay the
/// recipe result's patch. Used by both transmute and smithing transform.
fn transmute_result(recipe: &TransmuteRecipe, input: &ItemStack) -> ItemStack {
    apply_result_over_components(&recipe.result, &input.components)
}

fn apply_result_over_components(
    result: &ItemStack,
    base: &crate::item_components::ComponentMap,
) -> ItemStack {
    let mut stack = ItemStack {
        item: result.item.clone(),
        count: result.count,
        components: base.clone(),
    };
    for (component, value) in result.components.iter_entries() {
        match value {
            Some(value) => {
                stack.components.set(value.clone());
            }
            None => stack.components.mark_removed(component.name()),
        }
    }
    stack
}

// ---------------------------------------------------------------------------
// Furnaces and campfires
// ---------------------------------------------------------------------------

/// The four shipped cooking stations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FurnaceKind {
    Furnace,
    BlastFurnace,
    Smoker,
    Campfire,
}

impl FurnaceKind {
    pub fn cooking_kind(self) -> CookingKind {
        match self {
            Self::Furnace => CookingKind::Smelting,
            Self::BlastFurnace => CookingKind::Blasting,
            Self::Smoker => CookingKind::Smoking,
            Self::Campfire => CookingKind::Campfire,
        }
    }

    /// `AbstractFurnaceBlockEntity` needs fuel; a campfire burns continually
    /// while lit and has no fuel slot.
    pub fn requires_fuel(self) -> bool {
        !matches!(self, Self::Campfire)
    }

    pub fn default_cook_time(self) -> i32 {
        match self {
            Self::Furnace => 200,
            Self::BlastFurnace | Self::Smoker => 100,
            Self::Campfire => 600,
        }
    }
}

/// Furnace block-entity state mapped onto the four vanilla data values:
/// `lit_time_remaining`, `lit_total_time`, `cooking_timer`, and
/// `cooking_total_time`.
#[derive(Debug, Clone, PartialEq)]
pub struct FurnaceState {
    pub kind: FurnaceKind,
    pub input: ItemStack,
    pub fuel: ItemStack,
    pub output: ItemStack,
    pub lit_time_remaining: i32,
    pub lit_total_time: i32,
    pub cooking_timer: i32,
    pub cooking_total_time: i32,
    /// Vanilla stores per-recipe counts and computes XP on extraction; this
    /// model accumulates the exact recipe experience as each item completes.
    pub experience: f32,
    pub recipes_used: BTreeMap<String, i32>,
}

impl FurnaceState {
    pub fn new(kind: FurnaceKind) -> Self {
        Self {
            kind,
            input: ItemStack::empty(),
            fuel: ItemStack::empty(),
            output: ItemStack::empty(),
            lit_time_remaining: 0,
            lit_total_time: 0,
            cooking_timer: 0,
            cooking_total_time: 0,
            experience: 0.0,
            recipes_used: BTreeMap::new(),
        }
    }

    pub fn is_lit(&self) -> bool {
        self.lit_time_remaining > 0
    }

    pub fn slot(&self, slot: usize) -> Option<&ItemStack> {
        match slot {
            FURNACE_INPUT_SLOT => Some(&self.input),
            FURNACE_FUEL_SLOT => Some(&self.fuel),
            FURNACE_RESULT_SLOT => Some(&self.output),
            _ => None,
        }
    }

    /// Replacing the input resets cooking progress, as `setItem(0, …)` does.
    pub fn set_input(&mut self, stack: ItemStack) {
        self.input = stack;
        self.cooking_timer = 0;
        self.cooking_total_time = 0;
    }

    pub fn set_fuel(&mut self, stack: ItemStack) {
        self.fuel = stack;
    }

    /// The last recipe in catalog order matching the input and station.
    pub fn find_recipe<'a>(&self, catalog: &'a RecipeCatalog) -> Option<&'a Recipe> {
        if self.input.is_empty() {
            return None;
        }
        let wanted = self.kind.cooking_kind();
        let mut best = None;
        for (_, recipe) in catalog.recipes() {
            let RecipeKind::Cooking(cooking) = &recipe.kind else {
                continue;
            };
            if cooking.cooking == wanted && cooking.ingredient.matches(&self.input.item) {
                best = Some(recipe);
            }
        }
        best
    }

    /// Whether `recipe` can currently be cooked into the result slot.
    pub fn can_smelt(&self, recipe: &Recipe) -> bool {
        if self.input.is_empty() {
            return false;
        }
        let RecipeKind::Cooking(cooking) = &recipe.kind else {
            return false;
        };
        if cooking.cooking != self.kind.cooking_kind()
            || !cooking.ingredient.matches(&self.input.item)
        {
            return false;
        }
        self.can_accept(&cooking.result)
    }

    fn can_accept(&self, result: &ItemStack) -> bool {
        if result.is_empty() {
            return false;
        }
        if self.output.is_empty() {
            return true;
        }
        self.output.same_item_and_components(result)
            && self.output.count + result.count <= result.max_stack_size()
    }

    /// Advance one server tick.
    ///
    /// Mirrors `AbstractFurnaceBlockEntity.serverTick`: burn time is consumed
    /// first, a new fuel item is lit only when a valid recipe can complete,
    /// cooking progresses once per tick, and an interrupted cook cools down by
    /// two ticks.
    pub fn tick(&mut self, catalog: &RecipeCatalog) {
        if self.kind == FurnaceKind::Campfire {
            self.tick_campfire(catalog);
            return;
        }

        if self.lit_time_remaining > 0 {
            self.lit_time_remaining -= 1;
        }
        let recipe = self.find_recipe(catalog).cloned();
        let can_smelt = recipe.as_ref().is_some_and(|recipe| self.can_smelt(recipe));
        let startable = !self.fuel.is_empty() && !self.input.is_empty();

        if self.lit_time_remaining > 0 || startable {
            if self.lit_time_remaining == 0 && can_smelt {
                let burn_time = fuel_burn_time(&self.fuel.item).unwrap_or(0);
                if burn_time > 0 {
                    self.lit_time_remaining = burn_time;
                    self.lit_total_time = burn_time;
                    self.consume_fuel();
                }
            }

            if self.lit_time_remaining > 0 && can_smelt {
                let recipe = recipe.as_ref().expect("can_smelt requires a recipe");
                let cook_time = cooking_time(recipe).unwrap_or(self.kind.default_cook_time());
                if self.cooking_total_time != cook_time {
                    self.cooking_total_time = cook_time;
                }
                self.cooking_timer += 1;
                if self.cooking_timer >= self.cooking_total_time {
                    self.complete(recipe);
                }
            } else {
                self.cooking_timer = 0;
            }
        } else if self.cooking_timer > 0 {
            self.cooking_timer = (self.cooking_timer - 2).max(0);
        }
    }

    fn tick_campfire(&mut self, catalog: &RecipeCatalog) {
        let recipe = self.find_recipe(catalog).cloned();
        if let Some(recipe) = &recipe
            && self.can_smelt(recipe)
        {
            let cook_time = cooking_time(recipe).unwrap_or(self.kind.default_cook_time());
            self.cooking_total_time = cook_time;
            self.cooking_timer += 1;
            if self.cooking_timer >= self.cooking_total_time {
                self.complete(recipe);
            }
        } else {
            self.cooking_timer = 0;
        }
    }

    fn consume_fuel(&mut self) {
        let was_lava_bucket = self.fuel.item == "minecraft:lava_bucket";
        self.fuel.shrink(1);
        if was_lava_bucket && self.fuel.is_empty() {
            self.fuel = ItemStack::new("minecraft:bucket", 1);
        }
    }

    fn complete(&mut self, recipe: &Recipe) {
        let Some(result) = recipe.result().cloned() else {
            self.cooking_timer = 0;
            return;
        };
        if !self.can_accept(&result) {
            self.cooking_timer = 0;
            self.cooking_total_time = 0;
            return;
        }
        if self.output.is_empty() {
            self.output = result;
        } else {
            self.output.count += result.count;
        }
        self.input.shrink(1);
        if let RecipeKind::Cooking(cooking) = &recipe.kind {
            self.experience += cooking.experience;
        }
        *self.recipes_used.entry(recipe.id.clone()).or_insert(0) += 1;
        self.cooking_timer = 0;
        self.cooking_total_time = 0;
    }

    /// Remove the result slot without touching accumulated experience.
    pub fn take_output(&mut self) -> ItemStack {
        std::mem::replace(&mut self.output, ItemStack::empty())
    }

    /// Remove all accumulated experience.
    pub fn take_experience(&mut self) -> f32 {
        std::mem::take(&mut self.experience)
    }

    /// Vanilla's "player took the result" operation: output and XP together.
    pub fn extract(&mut self) -> Option<FurnaceExtract> {
        if self.output.is_empty() {
            return None;
        }
        Some(FurnaceExtract {
            output: self.take_output(),
            experience: self.take_experience(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FurnaceExtract {
    pub output: ItemStack,
    pub experience: f32,
}

fn cooking_time(recipe: &Recipe) -> Option<i32> {
    match &recipe.kind {
        RecipeKind::Cooking(cooking) => Some(cooking.cooking_time),
        _ => None,
    }
}

/// `FuelValues.vanillaBurnTimes` for the common shipped fuels, using the
/// standard 200-tick smelting time.
///
/// Non-flammable crimson/warped wood is removed after the tag additions, so it
/// is checked first.
pub fn fuel_burn_time(item: &str) -> Option<i32> {
    let item = if item.contains(':') {
        item.to_owned()
    } else {
        format!("minecraft:{item}")
    };
    if crate::tags::item_has("minecraft:non_flammable_wood", &item) {
        return None;
    }
    match item.as_str() {
        "minecraft:lava_bucket" => return Some(SMELTING_TIME * 100),
        "minecraft:coal_block" => return Some(SMELTING_TIME * 8 * 10),
        "minecraft:blaze_rod" => return Some(SMELTING_TIME * 12),
        "minecraft:coal" | "minecraft:charcoal" => return Some(SMELTING_TIME * 8),
        "minecraft:stick" | "minecraft:bowl" => return Some(SMELTING_TIME / 2),
        "minecraft:bamboo" | "minecraft:scaffolding" => return Some(SMELTING_TIME / 4),
        "minecraft:dried_kelp_block" => return Some(1 + SMELTING_TIME * 20),
        "minecraft:bamboo_mosaic"
        | "minecraft:note_block"
        | "minecraft:bookshelf"
        | "minecraft:chiseled_bookshelf"
        | "minecraft:lectern"
        | "minecraft:jukebox"
        | "minecraft:chest"
        | "minecraft:trapped_chest"
        | "minecraft:crafting_table"
        | "minecraft:daylight_detector"
        | "minecraft:bow"
        | "minecraft:fishing_rod"
        | "minecraft:crossbow"
        | "minecraft:ladder"
        | "minecraft:loom"
        | "minecraft:barrel"
        | "minecraft:cartography_table"
        | "minecraft:fletching_table"
        | "minecraft:smithing_table"
        | "minecraft:composter"
        | "minecraft:mangrove_roots" => return Some(SMELTING_TIME * 3 / 2),
        "minecraft:bamboo_mosaic_stairs" => return Some(SMELTING_TIME * 3 / 2),
        "minecraft:bamboo_mosaic_slab" => return Some(SMELTING_TIME * 3 / 4),
        "minecraft:dead_bush"
        | "minecraft:short_dry_grass"
        | "minecraft:tall_dry_grass"
        | "minecraft:azalea"
        | "minecraft:flowering_azalea"
        | "minecraft:leaf_litter" => return Some(SMELTING_TIME / 2),
        "minecraft:wooden_shovel"
        | "minecraft:wooden_sword"
        | "minecraft:wooden_spear"
        | "minecraft:wooden_hoe"
        | "minecraft:wooden_axe"
        | "minecraft:wooden_pickaxe" => return Some(SMELTING_TIME),
        _ => {}
    }

    let tag_value = |tag: &str| crate::tags::item_has(tag, &item);
    if tag_value("minecraft:logs")
        || tag_value("minecraft:bamboo_blocks")
        || tag_value("minecraft:planks")
        || tag_value("minecraft:wooden_stairs")
        || tag_value("minecraft:wooden_trapdoors")
        || tag_value("minecraft:wooden_pressure_plates")
        || tag_value("minecraft:wooden_shelves")
        || tag_value("minecraft:wooden_fences")
        || tag_value("minecraft:fence_gates")
        || tag_value("minecraft:banners")
    {
        return Some(SMELTING_TIME * 3 / 2);
    }
    if tag_value("minecraft:wooden_slabs") {
        return Some(SMELTING_TIME * 3 / 4);
    }
    if tag_value("minecraft:signs") {
        return Some(SMELTING_TIME);
    }
    if tag_value("minecraft:hanging_signs") {
        return Some(SMELTING_TIME * 4);
    }
    if tag_value("minecraft:wooden_doors") {
        return Some(SMELTING_TIME);
    }
    if tag_value("minecraft:boats") {
        return Some(SMELTING_TIME * 6);
    }
    if tag_value("minecraft:wool") {
        return Some(SMELTING_TIME / 2);
    }
    if tag_value("minecraft:wooden_buttons") {
        return Some(SMELTING_TIME / 2);
    }
    if tag_value("minecraft:saplings") {
        return Some(SMELTING_TIME / 2);
    }
    if tag_value("minecraft:wool_carpets") {
        return Some(1 + SMELTING_TIME / 3);
    }
    None
}

// ---------------------------------------------------------------------------
// Stonecutting and smithing
// ---------------------------------------------------------------------------

/// Stonecutting recipes matching `input`, in catalog order.
pub fn stonecutting_recipes<'a>(
    catalog: &'a RecipeCatalog,
    input: &ItemStack,
) -> Vec<(&'a str, &'a Recipe)> {
    if input.is_empty() {
        return Vec::new();
    }
    catalog
        .recipes()
        .filter(|(_, recipe)| {
            matches!(
                &recipe.kind,
                RecipeKind::Stonecutting(stonecutting) if stonecutting.ingredient.matches(&input.item)
            )
        })
        .collect()
}

/// Apply a named stonecutting recipe, consuming one input item.
pub fn stonecut_recipe(
    catalog: &RecipeCatalog,
    id: &str,
    input: &mut ItemStack,
) -> Result<CraftOutput, CraftingError> {
    let recipe = catalog
        .recipe(id)
        .ok_or_else(|| CraftingError::UnknownRecipe(id.to_owned()))?;
    let RecipeKind::Stonecutting(stonecutting) = &recipe.kind else {
        return Err(CraftingError::WrongStation {
            id: recipe.id.clone(),
            kind: recipe_kind_name(&recipe.kind),
            expected: "a stonecutter",
        });
    };
    if input.is_empty() || !stonecutting.ingredient.matches(&input.item) {
        return Err(CraftingError::RecipeDoesNotMatch(recipe.id.clone()));
    }
    let result = stonecutting.result.clone();
    input.shrink(1);
    Ok(CraftOutput {
        recipe: recipe.id.clone(),
        result,
        consumed_slots: vec![0],
    })
}

/// Apply the last matching stonecutting recipe, consuming one input item.
pub fn stonecut(
    catalog: &RecipeCatalog,
    input: &mut ItemStack,
) -> Result<CraftOutput, CraftingError> {
    let mut best = None;
    for (_, recipe) in catalog.recipes() {
        if let RecipeKind::Stonecutting(stonecutting) = &recipe.kind
            && stonecutting.ingredient.matches(&input.item)
        {
            best = Some(recipe);
        }
    }
    let recipe = best.ok_or(CraftingError::NoRecipe)?;
    stonecut_recipe(catalog, &recipe.id, input)
}

/// Smithing transform recipes whose three slots all match.
pub fn smithing_transform_recipes<'a>(
    catalog: &'a RecipeCatalog,
    template: &ItemStack,
    base: &ItemStack,
    addition: &ItemStack,
) -> Vec<(&'a str, &'a Recipe)> {
    if template.is_empty() || base.is_empty() || addition.is_empty() {
        return Vec::new();
    }
    catalog
        .recipes()
        .filter(|(_, recipe)| {
            matches!(
                &recipe.kind,
                RecipeKind::SmithingTransform(transform)
                    if transform.template.matches(&template.item)
                        && transform.base.matches(&base.item)
                        && transform.addition.matches(&addition.item)
            )
        })
        .collect()
}

/// Apply a named smithing transform, consuming one template, base, and
/// addition. Trim recipes are not implemented and fail explicitly.
pub fn smith_recipe(
    catalog: &RecipeCatalog,
    id: &str,
    template: &mut ItemStack,
    base: &mut ItemStack,
    addition: &mut ItemStack,
) -> Result<CraftOutput, CraftingError> {
    let recipe = catalog
        .recipe(id)
        .ok_or_else(|| CraftingError::UnknownRecipe(id.to_owned()))?;
    let RecipeKind::SmithingTransform(transform) = &recipe.kind else {
        return match &recipe.kind {
            RecipeKind::SmithingTrim(_) => Err(CraftingError::UnsupportedRecipe {
                id: recipe.id.clone(),
                kind: recipe_kind_name(&recipe.kind),
            }),
            _ => Err(CraftingError::WrongStation {
                id: recipe.id.clone(),
                kind: recipe_kind_name(&recipe.kind),
                expected: "a smithing table",
            }),
        };
    };
    if template.is_empty()
        || base.is_empty()
        || addition.is_empty()
        || !transform.template.matches(&template.item)
        || !transform.base.matches(&base.item)
        || !transform.addition.matches(&addition.item)
    {
        return Err(CraftingError::RecipeDoesNotMatch(recipe.id.clone()));
    }
    // `SmithingTransformRecipe.assemble` starts from the base stack, so the
    // base component patch survives the item transmutation.
    let result = apply_result_over_components(&transform.result, &base.components);
    template.shrink(1);
    base.shrink(1);
    addition.shrink(1);
    Ok(CraftOutput {
        recipe: recipe.id.clone(),
        result,
        consumed_slots: vec![0, 1, 2],
    })
}

/// Apply the last matching smithing transform.
pub fn smith(
    catalog: &RecipeCatalog,
    template: &mut ItemStack,
    base: &mut ItemStack,
    addition: &mut ItemStack,
) -> Result<CraftOutput, CraftingError> {
    let mut best = None;
    for (_, recipe) in catalog.recipes() {
        if let RecipeKind::SmithingTransform(transform) = &recipe.kind
            && transform.template.matches(&template.item)
            && transform.base.matches(&base.item)
            && transform.addition.matches(&addition.item)
        {
            best = Some(recipe);
        }
    }
    let recipe = best.ok_or(CraftingError::NoRecipe)?;
    smith_recipe(catalog, &recipe.id, template, base, addition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loot::Ingredient;

    fn recipe(id: &str, kind: RecipeKind) -> Recipe {
        Recipe {
            id: id.to_owned(),
            category: crate::loot::RecipeCategory::CraftingMisc,
            group: None,
            kind,
        }
    }

    fn shaped(id: &str, pattern: &[&str], key: &[(char, Ingredient)], result: &str) -> Recipe {
        recipe(
            id,
            RecipeKind::Shaped(ShapedRecipe {
                pattern: pattern.iter().map(|row| (*row).to_owned()).collect(),
                key: key.iter().cloned().collect(),
                result: ItemStack::new(result, 1),
                group: None,
                show_notification: true,
            }),
        )
    }

    #[test]
    fn pattern_shrink_crops_spaces_and_empty_rows() {
        let recipe = shaped(
            "minecraft:test",
            &["   ", "  #", "   "],
            &[('#', Ingredient::Item("minecraft:stick".to_owned()))],
            "minecraft:oak_planks",
        );
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            unreachable!()
        };
        let pattern = NormalizedPattern::from_recipe(&recipe.id, shaped).unwrap();
        assert_eq!((pattern.width, pattern.height), (1, 1));
        assert_eq!(pattern.ingredients.len(), 1);
        assert!(pattern.ingredients[0].is_some());
    }

    #[test]
    fn undefined_key_symbol_is_an_error() {
        let recipe = shaped(
            "minecraft:test",
            &["#X"],
            &[('#', Ingredient::Item("minecraft:stick".to_owned()))],
            "minecraft:oak_planks",
        );
        let RecipeKind::Shaped(shaped) = &recipe.kind else {
            unreachable!()
        };
        let error = NormalizedPattern::from_recipe(&recipe.id, shaped).unwrap_err();
        assert!(matches!(error, CraftingError::InvalidPattern { .. }));
    }

    #[test]
    fn normalizes_bare_item_names() {
        assert_eq!(fuel_burn_time("coal"), Some(1600));
        assert_eq!(fuel_burn_time("minecraft:oak_planks"), Some(300));
        assert_eq!(fuel_burn_time("minecraft:crimson_planks"), None);
        assert_eq!(fuel_burn_time("minecraft:dried_kelp_block"), Some(4001));
    }

    #[test]
    fn last_sorted_recipe_wins_a_conflict() {
        let catalog = RecipeCatalog::global();
        // `minecraft:oak_planks` is the only shapeless log recipe for oak.
        let mut grid = CraftingGrid::two_by_two();
        grid.set(0, 0, ItemStack::new("minecraft:oak_log", 1));
        let found = find_crafting_recipe(catalog, &grid).unwrap().unwrap();
        assert_eq!(found.id, "minecraft:oak_planks");
    }

    #[test]
    fn furnace_rejects_crafting_recipes() {
        let mut grid = CraftingGrid::two_by_two();
        grid.set(0, 0, ItemStack::new("minecraft:oak_log", 1));
        let error = grid
            .craft_recipe(
                RecipeCatalog::global(),
                "minecraft:iron_ingot_from_smelting_iron_ore",
            )
            .unwrap_err();
        assert!(matches!(error, CraftingError::WrongStation { .. }));
    }
}
