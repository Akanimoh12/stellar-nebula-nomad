/// # Achievements Module
///
/// Provides the public achievement catalog, on-chain progress tracking,
/// leaderboard integration, and achievement event emission for the Stellar
/// Nebula Nomad game.
///
/// ## Overview
///
/// [`achievement_engine`] handles the low-level storage and unlock logic.
/// This module builds on top of it to expose:
///
/// - A typed [`AchievementId`] enum for all 20+ catalog entries.
/// - An extended [`AchievementDef`] struct with category and rarity metadata.
/// - A leaderboard (top-10 by achievement count).
/// - Structured achievement-unlocked events.
/// - A convenience [`try_unlock`] wrapper: unlock + event + leaderboard in one call.
///
/// ## Usage
///
/// ```rust
/// // Check a single achievement's progress
/// let progress = query_progress(env, &player_addr, AchievementId::Surveyor as u64)?;
///
/// // Unlock an achievement (mints badge, emits event, updates leaderboard)
/// let badge_id = try_unlock(env, &player_addr, AchievementId::FirstScan as u64)?;
///
/// // Fetch the top-10 leaderboard
/// let top = leaderboard_top(env);
/// ```
use soroban_sdk::{contracttype, contracterror, symbol_short, Address, Env, String, Symbol, Vec};

use crate::achievement_engine::{
    AchievementError, AchievementKey, AchievementTemplate, unlock_achievement,
};
use crate::player_profile::get_profile_by_owner;
use crate::ship_nft::get_ships_by_owner;

// ─── Achievement IDs ──────────────────────────────────────────────────────────

/// Stable numeric identifiers for every achievement in the catalog.
///
/// Values map 1-to-1 to the `AchievementTemplate.id` stored on-chain.
/// IDs are **never** re-numbered so existing on-chain records remain valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
#[repr(u64)]
pub enum AchievementId {
    // ── Scan milestones ──────────────────────────────────────────────────────
    /// Complete the very first nebula scan.
    FirstScan    = 1,
    /// Accumulate 10 total scans.
    Surveyor     = 2,
    /// Accumulate 25 total scans.
    Pathfinder   = 3,
    /// Accumulate 50 total scans.
    Voyager      = 4,
    /// Accumulate 100 total scans.
    Navigator    = 5,
    // ── Essence milestones ───────────────────────────────────────────────────
    /// Earn 100 essence.
    EssenceOne   = 6,
    /// Earn 500 essence.
    EssenceTwo   = 7,
    /// Earn 1 000 essence.
    EssenceThree = 8,
    /// Earn 5 000 essence.
    EssenceFour  = 9,
    // ── Fleet milestones ─────────────────────────────────────────────────────
    /// Own 1 ship.
    FleetOne     = 10,
    /// Own 3 ships.
    FleetThree   = 11,
    /// Own 5 ships.
    FleetFive    = 12,
    /// Own 10 ships.
    FleetTen     = 13,
    // ── Higher scan milestones ───────────────────────────────────────────────
    /// Accumulate 150 total scans.
    DeepSurvey   = 14,
    /// Accumulate 200 total scans.
    GrandSurvey  = 15,
    /// Accumulate 300 total scans.
    CosmicReach  = 16,
    // ── Higher essence & fleet milestones ────────────────────────────────────
    /// Earn 10 000 essence.
    CosmicWealth = 17,
    /// Own 20 ships (rare).
    Armada       = 18,
    /// Accumulate 400 total scans (rare).
    Trailblazer  = 19,
    /// Reach 500 scans + 20 000 essence + 10 ships (rare, combined).
    Legend       = 20,
}

// ─── Achievement Definition ───────────────────────────────────────────────────

/// Extended display metadata for a catalog achievement.
///
/// Wraps the engine's `AchievementTemplate` with UI-specific fields that
/// don't affect unlock logic.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct AchievementDef {
    /// Unique achievement ID.
    pub id: u64,
    /// Short display title.
    pub title: String,
    /// Human-readable unlock criteria.
    pub description: String,
    /// Category tag: `"scan"`, `"essence"`, `"fleet"`, or `"combined"`.
    pub category: Symbol,
    /// `true` for achievements that trigger a celebration animation in the UI.
    pub is_rare: bool,
    /// Minimum total scans required (0 = not required).
    pub min_scans: u32,
    /// Minimum essence earned required (0 = not required).
    pub min_essence: i128,
    /// Minimum ships owned required (0 = not required).
    pub min_ships: u32,
}

// ─── Progress Snapshot ────────────────────────────────────────────────────────

/// Point-in-time progress for a single achievement including rarity metadata.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct AchievementProgressEx {
    /// Numeric achievement ID.
    pub achievement_id: u64,
    /// Display title.
    pub title: String,
    /// Whether the player has already unlocked this achievement.
    pub unlocked: bool,
    /// Whether the player currently meets all criteria (and has not unlocked yet).
    pub eligible: bool,
    /// Completion percentage in the range \[0, 100\].
    pub progress_pct: u32,
    /// Whether this is a rare achievement.
    pub is_rare: bool,
}

// ─── Leaderboard ─────────────────────────────────────────────────────────────

/// Storage key namespace for the achievement leaderboard.
#[derive(Clone)]
#[contracttype]
pub enum LeaderboardKey {
    /// Per-player achievement count used for scoring.
    PlayerScore(Address),
    /// Sorted top-10 entries list.
    TopEntries,
}

/// A single leaderboard entry.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct AchievementLeaderboardEntry {
    /// Player address.
    pub player: Address,
    /// Number of achievements unlocked by this player.
    pub achievement_count: u32,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AchievementsError {
    /// The requested achievement ID does not exist in the catalog.
    NotFound = 1,
    /// The player's profile could not be located.
    ProfileNotFound = 2,
}

// ─── Progress Query ───────────────────────────────────────────────────────────

/// Return an extended progress snapshot for a single achievement.
///
/// # Errors
///
/// - [`AchievementsError::NotFound`] — achievement ID not in catalog.
/// - [`AchievementsError::ProfileNotFound`] — player has no profile.
pub fn query_progress(
    env: &Env,
    player: &Address,
    achievement_id: u64,
) -> Result<AchievementProgressEx, AchievementsError> {
    let profile =
        get_profile_by_owner(env, player).map_err(|_| AchievementsError::ProfileNotFound)?;

    let ships = get_ships_by_owner(env, player);
    let ship_count = ships.len() as u32;

    let template: AchievementTemplate = env
        .storage()
        .persistent()
        .get(&AchievementKey::Template(achievement_id))
        .ok_or(AchievementsError::NotFound)?;

    let unlocked = env
        .storage()
        .persistent()
        .has(&AchievementKey::PlayerAchievement(
            player.clone(),
            achievement_id,
        ));

    let pct = compute_pct(&template, profile.total_scans, profile.essence_earned, ship_count);

    Ok(AchievementProgressEx {
        achievement_id,
        title: template.title,
        unlocked,
        eligible: pct >= 100 && !unlocked,
        progress_pct: pct,
        is_rare: is_rare_achievement(achievement_id),
    })
}

// ─── Unlock Wrapper ───────────────────────────────────────────────────────────

/// Unlock `achievement_id` for `player`.
///
/// On success: mints a badge (via the engine), emits an achievement event,
/// and increments + records the leaderboard entry.
///
/// # Errors
///
/// Delegates to [`AchievementError`] variants from the engine.
pub fn try_unlock(
    env: &Env,
    player: &Address,
    achievement_id: u64,
) -> Result<u64, AchievementError> {
    let badge = unlock_achievement(env, player.clone(), achievement_id)?;
    increment_leaderboard(env, player);
    record_leaderboard_entry(env, player);
    emit_achievement_event(env, player, achievement_id, badge.badge_id);
    Ok(badge.badge_id)
}

// ─── Events ───────────────────────────────────────────────────────────────────

/// Publish an `ach_unlck` contract event.
///
/// **Topics:** `(symbol_short!("ach_unlck"), player)`
/// **Data:** `(achievement_id: u64, badge_id: u64, rare: bool)`
pub fn emit_achievement_event(
    env: &Env,
    player: &Address,
    achievement_id: u64,
    badge_id: u64,
) {
    let rare = is_rare_achievement(achievement_id);
    env.events().publish(
        (symbol_short!("ach_unlck"), player.clone()),
        (achievement_id, badge_id, rare),
    );
}

// ─── Leaderboard ─────────────────────────────────────────────────────────────

/// Increment `player`'s achievement score by 1.
pub fn increment_leaderboard(env: &Env, player: &Address) {
    let key = LeaderboardKey::PlayerScore(player.clone());
    let current: u32 = env.storage().persistent().get(&key).unwrap_or(0);
    env.storage().persistent().set(&key, &(current + 1));
}

/// Return the total achievements unlocked by `player`.
pub fn leaderboard_score(env: &Env, player: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&LeaderboardKey::PlayerScore(player.clone()))
        .unwrap_or(0)
}

/// Return the stored top-10 leaderboard snapshot.
pub fn leaderboard_top(env: &Env) -> Vec<AchievementLeaderboardEntry> {
    env.storage()
        .persistent()
        .get(&LeaderboardKey::TopEntries)
        .unwrap_or_else(|| Vec::new(env))
}

/// Upsert `player` into the top-10 list based on their current score.
///
/// The list is sorted descending by `achievement_count` and capped at 10.
pub fn record_leaderboard_entry(env: &Env, player: &Address) {
    let score = leaderboard_score(env, player);
    let entries: Vec<AchievementLeaderboardEntry> = leaderboard_top(env);

    // Remove existing entry for this player.
    let mut without_player: Vec<AchievementLeaderboardEntry> = Vec::new(env);
    let mut i = 0u32;
    while i < entries.len() {
        if let Some(e) = entries.get(i) {
            if e.player != *player {
                without_player.push_back(e);
            }
        }
        i += 1;
    }

    // Insert at the correct sorted position.
    let new_entry = AchievementLeaderboardEntry {
        player: player.clone(),
        achievement_count: score,
    };

    let mut sorted: Vec<AchievementLeaderboardEntry> = Vec::new(env);
    let mut inserted = false;
    let mut j = 0u32;
    while j < without_player.len() {
        if let Some(e) = without_player.get(j) {
            if !inserted && score >= e.achievement_count {
                sorted.push_back(new_entry.clone());
                inserted = true;
            }
            sorted.push_back(e);
        }
        j += 1;
    }
    if !inserted {
        sorted.push_back(new_entry);
    }

    // Cap at 10.
    let mut capped: Vec<AchievementLeaderboardEntry> = Vec::new(env);
    let mut k = 0u32;
    while k < sorted.len() && k < 10 {
        if let Some(e) = sorted.get(k) {
            capped.push_back(e);
        }
        k += 1;
    }

    env.storage()
        .persistent()
        .set(&LeaderboardKey::TopEntries, &capped);
}

// ─── Internal Helpers ─────────────────────────────────────────────────────────

fn compute_pct(template: &AchievementTemplate, scans: u32, essence: i128, ships: u32) -> u32 {
    let mut pct = 100u32;

    if template.min_scans > 0 {
        pct = pct.min((scans.saturating_mul(100) / template.min_scans).min(100));
    }
    if template.min_essence > 0 {
        let eu = if essence < 0 { 0u128 } else { essence as u128 };
        let req = template.min_essence as u128;
        pct = pct.min((eu.saturating_mul(100) / req).min(100) as u32);
    }
    if template.min_ships > 0 {
        pct = pct.min((ships.saturating_mul(100) / template.min_ships).min(100));
    }

    pct
}

/// `true` for achievements that deserve a rare-unlock celebration in the UI.
fn is_rare_achievement(id: u64) -> bool {
    // Legend = combined milestone; Armada = 20 ships; Trailblazer = 400 scans
    matches!(id, 18 | 19 | 20)
}
