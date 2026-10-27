// The steps of a hero's look, from plain to legendary, and the level each
// one starts at. Shared by the level logic (`heroModel`) and the two artists
// (`hiSlime`, `hiHuman`), so a new step is added here.

/// What each tier adds on top of the ones below it.
export const TIER = {
  /// Lv 1: the class's starter gear (headband, small hat, bandana, circlet, ears).
  STARTER: 1,
  /// Lv 3: the full outfit (helmet, tall hat and robe, hood, halo, collar).
  OUTFIT: 2,
  /// Lv 5: a jewel set in the gear.
  JEWEL: 3,
  /// Lv 10: the class's item (sword, staff, dagger, holy staff, fish).
  ITEM: 4,
  /// Lv 20: a cape (humans) or a sash (slimes) in the class colour.
  CAPE: 5,
  /// Lv 30: metal and trim turn gold; cats get a crown.
  GOLD: 6,
  /// Lv 50: wings for everyone.
  WINGS: 7,
  /// Lv 75: a glowing aura and sparkles.
  AURA: 8,
  /// Lv 100: legendary: a crown and a rainbow aura with a star in orbit.
  LEGEND: 9,
} as const;

/// The level each tier starts at, by tier.
export const TIER_LEVELS = [0, 1, 3, 5, 10, 20, 30, 50, 75, 100] as const;
export type Tier = 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9;
