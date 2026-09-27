use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, String, Symbol, Vec};

// ─── Capacity Constants ───────────────────────────────────────────────────────

/// Maximum number of active scheduled events.
pub const MAX_ACTIVE_EVENTS: u32 = 20;
/// Maximum simultaneous active time-limited challenges.
pub const MAX_ACTIVE_CHALLENGES: u32 = 10;

// ─── Interval Constants ───────────────────────────────────────────────────────

/// Weekly nebula festival interval (7 days in seconds).
pub const WEEKLY_FESTIVAL_INTERVAL: u64 = 7 * 24 * 60 * 60;
/// Double-essence weekend interval (7 days, offset by 2 days from festival).
pub const DOUBLE_ESSENCE_INTERVAL: u64 = 7 * 24 * 60 * 60;
/// Seasonal boss event fires once per chapter (~30 days).
pub const SEASONAL_BOSS_INTERVAL: u64 = 30 * 24 * 60 * 60;
/// Fleet challenge interval: bi-weekly.
pub const FLEET_CHALLENGE_INTERVAL: u64 = 14 * 24 * 60 * 60;
/// Nebula surge: 3-day rotating windows.
pub const NEBULA_SURGE_INTERVAL: u64 = 3 * 24 * 60 * 60;
/// Community goal: monthly.
pub const COMMUNITY_GOAL_INTERVAL: u64 = 30 * 24 * 60 * 60;

// ─── Storage Keys ─────────────────────────────────────────────────────────────

#[derive(Clone)]
#[contracttype]
pub enum EventKey {
    /// Event data keyed by event ID.
    Event(u64),
    /// Global event counter.
    EventCounter,
    /// Admin address for scheduling permissions.
    Admin,
    /// Active event IDs list.
    ActiveEvents,
    /// Burst counter for rate limiting.
    BurstCounter,
    /// Last execution timestamp for a recurring event type.
    RecurringLastRun(Symbol),
    /// Time-limited challenge data keyed by challenge ID.
    Challenge(u64),
    /// Global challenge counter.
    ChallengeCounter,
    /// Active challenge IDs for the current season.
    ActiveChallenges,
    /// Player progress toward a challenge: `(challenge_id, profile_id) -> u64`
    ChallengeProgress(u64, u64),
    /// Tracks whether a player has claimed a challenge reward: `(challenge_id, profile_id) -> bool`
    ChallengeClaimed(u64, u64),
    /// Time-limited seasonal event keyed by ID.
    SeasonalEvent(u64),
    /// Global seasonal event counter.
    SeasonalEventCounter,
    /// IDs of seasonal events that are scheduled or active.
    PendingSeasonalEvents,
    /// Timestamp before which no new event of a category may start.
    CategoryCooldown(Symbol),
    /// Player participation in a seasonal event: `(event_id, profile_id)`.
    SeasonalEventEntry(u64, u64),
}

// ─── Recurring Event Types ────────────────────────────────────────────────────

/// Named recurring event templates that auto-schedule based on a fixed interval.
///
/// Each variant maps to a reward multiplier (in bps relative to base pool) and
/// an interval constant. Admin calls `schedule_recurring_event` to queue the
/// next occurrence — no manual `start_time` needed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum RecurringEventType {
    /// Weekly nebula festival: essence yield +20% for all players.
    WeeklyFestival,
    /// Double-essence weekend: 2× essence from all scans for 48 hours.
    DoubleEssenceWeekend,
    /// Seasonal boss encounter: high-reward `PvE` event tied to current chapter.
    SeasonalBoss,
    /// Fleet challenge: team-based ship coordination mini-game.
    FleetChallenge,
    /// Nebula surge: a specific nebula region spawns 3× anomalies for 72 hours.
    NebulaSurge,
    /// Community goal: a global collective target (total scans / essence mined).
    CommunityGoal,
}

impl RecurringEventType {
    /// Return the interval in seconds between successive occurrences.
    pub fn interval_secs(&self) -> u64 {
        match self {
            RecurringEventType::WeeklyFestival => WEEKLY_FESTIVAL_INTERVAL,
            RecurringEventType::DoubleEssenceWeekend => DOUBLE_ESSENCE_INTERVAL,
            RecurringEventType::SeasonalBoss => SEASONAL_BOSS_INTERVAL,
            RecurringEventType::FleetChallenge => FLEET_CHALLENGE_INTERVAL,
            RecurringEventType::NebulaSurge => NEBULA_SURGE_INTERVAL,
            RecurringEventType::CommunityGoal => COMMUNITY_GOAL_INTERVAL,
        }
    }

    /// Reward multiplier in basis points relative to the base reward pool.
    /// `10_000` bps = 1× (no change). `15_000` = 1.5×, `20_000` = 2×.
    pub fn reward_multiplier_bps(&self) -> u32 {
        match self {
            RecurringEventType::WeeklyFestival => 12_000, // 1.2×
            RecurringEventType::DoubleEssenceWeekend => 20_000, // 2×
            RecurringEventType::SeasonalBoss => 25_000,   // 2.5×
            RecurringEventType::FleetChallenge => 15_000, // 1.5×
            RecurringEventType::NebulaSurge => 10_000,    // 1×
            RecurringEventType::CommunityGoal => 18_000,  // 1.8×
        }
    }

    /// Short symbol tag used as a storage / event discriminator.
    pub fn type_symbol(&self) -> Symbol {
        match self {
            RecurringEventType::WeeklyFestival => symbol_short!("festival"),
            RecurringEventType::DoubleEssenceWeekend => symbol_short!("dbl_ess"),
            RecurringEventType::SeasonalBoss => symbol_short!("boss"),
            RecurringEventType::FleetChallenge => symbol_short!("fleet"),
            RecurringEventType::NebulaSurge => symbol_short!("surge"),
            RecurringEventType::CommunityGoal => symbol_short!("comm_gol"),
        }
    }
}

// ─── Data Types ───────────────────────────────────────────────────────────────

/// Scheduled event record.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct ScheduledEvent {
    pub event_id: u64,
    pub event_type: Symbol,
    pub start_time: u64,
    pub creator: Address,
    pub executed: bool,
    pub reward_pool: i128,
    pub participants: u32,
}

/// Event execution result.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct EventResult {
    pub event_id: u64,
    pub executed_at: u64,
    pub rewards_distributed: i128,
    pub participants: u32,
}

/// A time-limited seasonal challenge that players can complete for rewards.
///
/// Both free and premium players can attempt any challenge; premium holders
/// receive the larger `reward_premium` payout on completion.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct TimeLimitedChallenge {
    /// Unique challenge identifier.
    pub challenge_id: u64,
    /// Display title shown in game UI.
    pub title: String,
    /// Short description of the objective.
    pub description: String,
    /// Season this challenge belongs to.
    pub season_id: u64,
    /// When the challenge window opens.
    pub start_time: u64,
    /// When the challenge window closes.
    pub end_time: u64,
    /// The metric being tracked (e.g. `"scans"`, `"essence"`, `"pvp_wins"`).
    pub target_metric: Symbol,
    /// How much of `target_metric` a player must accumulate to complete.
    pub target_value: u64,
    /// Essence/token reward for free-pass players on completion.
    pub reward_free: i128,
    /// Essence/token reward for premium-pass players on completion.
    pub reward_premium: i128,
    /// Total unique players who have submitted progress.
    pub participants: u32,
    /// Total unique players who have completed the challenge.
    pub completed_by: u32,
}

/// Snapshot of a single player's progress on a challenge.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct ChallengeProgress {
    pub challenge_id: u64,
    pub profile_id: u64,
    /// Accumulated metric value so far.
    pub current_value: u64,
    /// Whether the player has reached `target_value`.
    pub completed: bool,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

#[contracterror]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EventError {
    /// Event start time is in the past.
    EventAlreadyPassed = 1,
    /// Event not found.
    EventNotFound = 2,
    /// Event already executed.
    EventAlreadyExecuted = 3,
    /// Event not yet ready to execute.
    EventNotReady = 4,
    /// Unauthorized — admin only.
    Unauthorized = 5,
    /// Too many active events (max 20).
    TooManyActiveEvents = 6,
    /// Burst limit exceeded.
    BurstLimitExceeded = 7,
    /// Invalid event type.
    InvalidEventType = 8,
    /// Challenge not found.
    ChallengeNotFound = 9,
    /// Challenge has expired.
    ChallengeExpired = 10,
    /// Player has already claimed reward for this challenge.
    AlreadyClaimed = 11,
    /// Player has not yet completed the challenge.
    ChallengeNotComplete = 12,
    /// Too many active challenges.
    TooManyChallenges = 13,
    /// Recurring event fired too recently; interval not yet elapsed.
    RecurringNotDue = 14,
    /// Challenge window has not opened yet.
    ChallengeNotStarted = 15,
    /// Event window is invalid or falls outside the current season.
    InvalidEventWindow = 16,
    /// The event category is still cooling down from a previous event.
    EventCooldownActive = 17,
    /// The window overlaps an exclusive event (or this exclusive event overlaps another).
    ExclusiveEventConflict = 18,
    /// The event is not in the state required for this operation.
    InvalidEventState = 19,
    /// No season is running.
    NoActiveSeason = 20,
    /// Too many seasonal events scheduled or active.
    TooManySeasonalEvents = 21,
    /// Reward pool must be positive and must not overflow.
    InvalidRewardPool = 22,
    /// Player has no reward for this event.
    NoEventReward = 23,
    /// The reward claim window has closed.
    ClaimWindowClosed = 24,
}

impl crate::error_standard::StandardContractError for EventError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::EventAlreadyPassed
            | Self::EventAlreadyExecuted
            | Self::ChallengeExpired
            | Self::AlreadyClaimed
            | Self::ChallengeNotComplete
            | Self::ExclusiveEventConflict
            | Self::InvalidEventState
            | Self::ClaimWindowClosed => (ErrorKind::Conflict, false),
            Self::EventNotFound
            | Self::ChallengeNotFound
            | Self::NoActiveSeason
            | Self::NoEventReward => (ErrorKind::NotFound, false),
            Self::EventNotReady
            | Self::RecurringNotDue
            | Self::ChallengeNotStarted
            | Self::EventCooldownActive => (ErrorKind::Conflict, true),
            Self::Unauthorized => (ErrorKind::Authorization, false),
            Self::TooManyActiveEvents | Self::TooManyChallenges | Self::TooManySeasonalEvents => {
                (ErrorKind::ResourceLimit, false)
            }
            Self::BurstLimitExceeded => (ErrorKind::ResourceLimit, true),
            Self::InvalidEventType | Self::InvalidEventWindow | Self::InvalidRewardPool => {
                (ErrorKind::Validation, false)
            }
        };
        crate::error_standard::ErrorDescriptor {
            module: "event_scheduler",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

// ─── Helper Functions ─────────────────────────────────────────────────────────

fn require_admin(env: &Env, caller: &Address) -> Result<(), EventError> {
    caller.require_auth();
    let admin: Address = env
        .storage()
        .instance()
        .get(&EventKey::Admin)
        .ok_or(EventError::Unauthorized)?;

    if caller != &admin {
        return Err(EventError::Unauthorized);
    }
    Ok(())
}

fn get_current_timestamp(env: &Env) -> u64 {
    env.ledger().timestamp()
}

fn next_event_id(env: &Env) -> u64 {
    let current: u64 = env
        .storage()
        .instance()
        .get(&EventKey::EventCounter)
        .unwrap_or(0);
    let next = current + 1;
    env.storage().instance().set(&EventKey::EventCounter, &next);
    next
}

fn next_challenge_id(env: &Env) -> u64 {
    let current: u64 = env
        .storage()
        .instance()
        .get(&EventKey::ChallengeCounter)
        .unwrap_or(0);
    let next = current + 1;
    env.storage()
        .instance()
        .set(&EventKey::ChallengeCounter, &next);
    next
}

fn check_burst_limit(env: &Env) -> Result<(), EventError> {
    let counter: u32 = env
        .storage()
        .instance()
        .get(&EventKey::BurstCounter)
        .unwrap_or(0);

    if counter >= MAX_ACTIVE_EVENTS {
        return Err(EventError::BurstLimitExceeded);
    }

    env.storage()
        .instance()
        .set(&EventKey::BurstCounter, &(counter + 1));
    Ok(())
}

/// Reset burst counter.
pub fn reset_burst_counter(env: &Env) {
    env.storage().instance().set(&EventKey::BurstCounter, &0u32);
}

fn add_to_active_events(env: &Env, event_id: u64) -> Result<(), EventError> {
    let mut active: Vec<u64> = env
        .storage()
        .instance()
        .get(&EventKey::ActiveEvents)
        .unwrap_or(Vec::new(env));

    if active.len() >= MAX_ACTIVE_EVENTS {
        return Err(EventError::TooManyActiveEvents);
    }

    active.push_back(event_id);
    env.storage()
        .instance()
        .set(&EventKey::ActiveEvents, &active);
    Ok(())
}

fn remove_from_active_events(env: &Env, event_id: u64) {
    let active: Vec<u64> = env
        .storage()
        .instance()
        .get(&EventKey::ActiveEvents)
        .unwrap_or(Vec::new(env));

    let mut new_active = Vec::new(env);
    for i in 0..active.len() {
        let id = active.get(i).unwrap();
        if id != event_id {
            new_active.push_back(id);
        }
    }

    env.storage()
        .instance()
        .set(&EventKey::ActiveEvents, &new_active);
}

// ─── Initialization ───────────────────────────────────────────────────────────

/// Initialize the event scheduler with an admin address.
pub fn initialize_scheduler(env: &Env, admin: &Address) {
    admin.require_auth();
    env.storage().instance().set(&EventKey::Admin, admin);
    env.storage().instance().set(&EventKey::EventCounter, &0u64);
    env.storage()
        .instance()
        .set(&EventKey::ActiveEvents, &Vec::<u64>::new(env));
    env.storage()
        .instance()
        .set(&EventKey::ChallengeCounter, &0u64);
    env.storage()
        .instance()
        .set(&EventKey::ActiveChallenges, &Vec::<u64>::new(env));

    env.events().publish((symbol_short!("init_sch"),), admin);
}

// ─── Scheduled Events (unchanged API) ─────────────────────────────────────────

/// Schedule a new community event.
pub fn schedule_event(
    env: &Env,
    admin: &Address,
    event_type: Symbol,
    start_time: u64,
    reward_pool: i128,
) -> Result<u64, EventError> {
    require_admin(env, admin)?;
    schedule_event_authorized(env, admin, event_type, start_time, reward_pool)
}

/// Schedule an event for a caller already verified by `require_admin`
/// (authorizing twice in one invocation is rejected by the host).
fn schedule_event_authorized(
    env: &Env,
    admin: &Address,
    event_type: Symbol,
    start_time: u64,
    reward_pool: i128,
) -> Result<u64, EventError> {
    // Report a full active list before consuming burst budget.
    if get_active_events(env).len() >= MAX_ACTIVE_EVENTS {
        return Err(EventError::TooManyActiveEvents);
    }
    check_burst_limit(env)?;

    let current_time = get_current_timestamp(env);

    if start_time <= current_time {
        return Err(EventError::EventAlreadyPassed);
    }

    // Validate event type against both legacy symbols and new recurring types.
    let valid_types = [
        symbol_short!("festival"),
        symbol_short!("raid"),
        symbol_short!("harvest"),
        symbol_short!("pvp"),
        symbol_short!("explore"),
        symbol_short!("dbl_ess"),
        symbol_short!("boss"),
        symbol_short!("fleet"),
        symbol_short!("surge"),
        symbol_short!("comm_gol"),
    ];

    let mut is_valid = false;
    for valid_type in &valid_types {
        if &event_type == valid_type {
            is_valid = true;
            break;
        }
    }

    if !is_valid {
        return Err(EventError::InvalidEventType);
    }

    let event_id = next_event_id(env);

    let event = ScheduledEvent {
        event_id,
        event_type: event_type.clone(),
        start_time,
        creator: admin.clone(),
        executed: false,
        reward_pool,
        participants: 0,
    };

    env.storage()
        .instance()
        .set(&EventKey::Event(event_id), &event);

    add_to_active_events(env, event_id)?;

    env.events().publish(
        (symbol_short!("evt_sched"), event_id),
        (event_type, start_time, reward_pool),
    );

    Ok(event_id)
}

// ─── Recurring Event Scheduling ───────────────────────────────────────────────

/// Schedule the next occurrence of a recurring event type.
///
/// The `start_time` is computed automatically as `now + interval`.  If the
/// same recurring type was recently scheduled, this call will fail with
/// `RecurringNotDue` unless enough time has passed.
///
/// Returns the new event ID.
pub fn schedule_recurring_event(
    env: &Env,
    admin: &Address,
    event_type: &RecurringEventType,
    reward_pool: i128,
) -> Result<u64, EventError> {
    require_admin(env, admin)?;

    let type_sym = event_type.type_symbol();
    let now = get_current_timestamp(env);
    let interval = event_type.interval_secs();

    // Enforce minimum interval between same-type recurring events.
    let last_run_key = EventKey::RecurringLastRun(type_sym.clone());
    if let Some(last_run) = env.storage().instance().get::<EventKey, u64>(&last_run_key) {
        if now < last_run + interval {
            return Err(EventError::RecurringNotDue);
        }
    }

    let start_time = now + interval;
    let event_id =
        schedule_event_authorized(env, admin, type_sym.clone(), start_time, reward_pool)?;

    // Record this scheduling timestamp.
    env.storage().instance().set(&last_run_key, &now);

    Ok(event_id)
}

// ─── Trigger / Cancel Events ──────────────────────────────────────────────────

/// Trigger a scheduled event when its time arrives.
pub fn trigger_scheduled_event(env: &Env, event_id: u64) -> Result<EventResult, EventError> {
    let mut event: ScheduledEvent = env
        .storage()
        .instance()
        .get(&EventKey::Event(event_id))
        .ok_or(EventError::EventNotFound)?;

    if event.executed {
        return Err(EventError::EventAlreadyExecuted);
    }

    let current_time = get_current_timestamp(env);

    if current_time < event.start_time {
        return Err(EventError::EventNotReady);
    }

    event.executed = true;
    env.storage()
        .instance()
        .set(&EventKey::Event(event_id), &event);

    remove_from_active_events(env, event_id);

    let rewards_distributed = event.reward_pool;
    let participants = event.participants;

    let result = EventResult {
        event_id,
        executed_at: current_time,
        rewards_distributed,
        participants,
    };

    env.events().publish(
        (symbol_short!("evt_trig"), event_id),
        (current_time, rewards_distributed, participants),
    );

    Ok(result)
}

/// Cancel a scheduled event (admin only).
pub fn cancel_event(env: &Env, admin: &Address, event_id: u64) -> Result<(), EventError> {
    require_admin(env, admin)?;

    let mut event: ScheduledEvent = env
        .storage()
        .instance()
        .get(&EventKey::Event(event_id))
        .ok_or(EventError::EventNotFound)?;

    if event.executed {
        return Err(EventError::EventAlreadyExecuted);
    }

    event.executed = true;
    env.storage()
        .instance()
        .set(&EventKey::Event(event_id), &event);

    remove_from_active_events(env, event_id);

    env.events()
        .publish((symbol_short!("evt_cncl"), event_id), admin);

    Ok(())
}

// ─── Time-Limited Challenges ──────────────────────────────────────────────────

/// Create a new time-limited seasonal challenge.
///
/// Challenges are scoped to the current season and expire at `end_time`.
/// Both free and premium players can participate; premium holders receive
/// `reward_premium` instead of `reward_free` on successful claim.
pub fn create_time_limited_challenge(
    env: &Env,
    admin: &Address,
    season_id: u64,
    duration_secs: u64,
    spec: SeasonalChallengeSpec,
) -> Result<u64, EventError> {
    require_admin(env, admin)?;

    let now = get_current_timestamp(env);
    register_challenge(
        env,
        TimeLimitedChallenge {
            challenge_id: 0,
            title: spec.title,
            description: spec.description,
            season_id,
            start_time: now,
            end_time: now + duration_secs,
            target_metric: spec.target_metric,
            target_value: spec.target_value,
            reward_free: spec.reward_free,
            reward_premium: spec.reward_premium,
            participants: 0,
            completed_by: 0,
        },
    )
}

/// Assign an ID to `challenge`, store it and add it to the active list.
fn register_challenge(env: &Env, mut challenge: TimeLimitedChallenge) -> Result<u64, EventError> {
    let mut active: Vec<u64> = env
        .storage()
        .instance()
        .get(&EventKey::ActiveChallenges)
        .unwrap_or(Vec::new(env));

    if active.len() >= MAX_ACTIVE_CHALLENGES {
        return Err(EventError::TooManyChallenges);
    }

    let challenge_id = next_challenge_id(env);
    challenge.challenge_id = challenge_id;

    env.storage()
        .instance()
        .set(&EventKey::Challenge(challenge_id), &challenge);

    active.push_back(challenge_id);
    env.storage()
        .instance()
        .set(&EventKey::ActiveChallenges, &active);

    env.events().publish(
        (symbol_short!("chal_new"), challenge_id),
        (
            challenge.season_id,
            challenge.target_metric,
            challenge.target_value,
        ),
    );

    Ok(challenge_id)
}

/// Record a player's incremental progress toward a challenge.
///
/// `metric_delta` is the amount gained in this action (e.g. scans done, essence
/// earned).  The function is additive — call it whenever a relevant action occurs.
/// Returns the player's new cumulative value for this challenge.
pub fn record_challenge_progress(
    env: &Env,
    profile_id: u64,
    challenge_id: u64,
    metric_delta: u64,
) -> Result<u64, EventError> {
    let mut challenge: TimeLimitedChallenge = env
        .storage()
        .instance()
        .get(&EventKey::Challenge(challenge_id))
        .ok_or(EventError::ChallengeNotFound)?;

    let now = get_current_timestamp(env);
    if now < challenge.start_time {
        return Err(EventError::ChallengeNotStarted);
    }
    if now > challenge.end_time {
        return Err(EventError::ChallengeExpired);
    }

    let prog_key = EventKey::ChallengeProgress(challenge_id, profile_id);
    let mut prog: ChallengeProgress =
        env.storage()
            .persistent()
            .get(&prog_key)
            .unwrap_or(ChallengeProgress {
                challenge_id,
                profile_id,
                current_value: 0,
                completed: false,
            });

    let was_new_participant = prog.current_value == 0 && metric_delta > 0;
    prog.current_value = prog.current_value.saturating_add(metric_delta);

    let just_completed = !prog.completed && prog.current_value >= challenge.target_value;
    if just_completed {
        prog.completed = true;
        challenge.completed_by += 1;
    }
    if was_new_participant {
        challenge.participants += 1;
    }

    env.storage().persistent().set(&prog_key, &prog);
    env.storage()
        .instance()
        .set(&EventKey::Challenge(challenge_id), &challenge);

    if just_completed {
        env.events().publish(
            (symbol_short!("chal_done"), challenge_id),
            (profile_id, prog.current_value),
        );
    }

    Ok(prog.current_value)
}

/// Claim the reward for completing a time-limited challenge.
///
/// `is_premium` should reflect whether the player holds a premium battle pass
/// for the current season; pass `true` to receive `reward_premium`.
///
/// Returns the reward amount credited.
pub fn claim_challenge_reward(
    env: &Env,
    player: &Address,
    profile_id: u64,
    challenge_id: u64,
    is_premium: bool,
) -> Result<i128, EventError> {
    player.require_auth();

    let challenge: TimeLimitedChallenge = env
        .storage()
        .instance()
        .get(&EventKey::Challenge(challenge_id))
        .ok_or(EventError::ChallengeNotFound)?;

    let now = get_current_timestamp(env);
    // Allow claiming up to 7 days after challenge end (grace period).
    let grace = challenge.end_time + 7 * 24 * 60 * 60;
    if now > grace {
        return Err(EventError::ChallengeExpired);
    }

    let claimed_key = EventKey::ChallengeClaimed(challenge_id, profile_id);
    let already_claimed: bool = env
        .storage()
        .persistent()
        .get(&claimed_key)
        .unwrap_or(false);

    if already_claimed {
        return Err(EventError::AlreadyClaimed);
    }

    let prog_key = EventKey::ChallengeProgress(challenge_id, profile_id);
    let prog: ChallengeProgress = env
        .storage()
        .persistent()
        .get(&prog_key)
        .ok_or(EventError::ChallengeNotComplete)?;

    if !prog.completed {
        return Err(EventError::ChallengeNotComplete);
    }

    let reward = if is_premium {
        challenge.reward_premium
    } else {
        challenge.reward_free
    };

    env.storage().persistent().set(&claimed_key, &true);

    env.events().publish(
        (symbol_short!("chal_clm"), challenge_id),
        (profile_id, reward, is_premium),
    );

    Ok(reward)
}

/// Get all active (non-expired) challenge IDs for the current season.
pub fn get_active_challenges(env: &Env) -> Vec<u64> {
    let now = get_current_timestamp(env);
    let all: Vec<u64> = env
        .storage()
        .instance()
        .get(&EventKey::ActiveChallenges)
        .unwrap_or(Vec::new(env));

    let mut active = Vec::new(env);
    for i in 0..all.len() {
        if let Some(id) = all.get(i) {
            if let Some(challenge) = env
                .storage()
                .instance()
                .get::<EventKey, TimeLimitedChallenge>(&EventKey::Challenge(id))
            {
                if now <= challenge.end_time {
                    active.push_back(id);
                }
            }
        }
    }
    active
}

/// Admin: expire and remove challenges that have passed their grace period.
///
/// Call this during season rollover to clean up stale challenge IDs.
pub fn expire_challenges(env: &Env, admin: &Address) -> Result<u32, EventError> {
    require_admin(env, admin)?;

    let now = get_current_timestamp(env);
    let grace = 7 * 24 * 60 * 60u64;

    let all: Vec<u64> = env
        .storage()
        .instance()
        .get(&EventKey::ActiveChallenges)
        .unwrap_or(Vec::new(env));

    let mut retained = Vec::new(env);
    let mut removed: u32 = 0;

    for i in 0..all.len() {
        if let Some(id) = all.get(i) {
            let keep = env
                .storage()
                .instance()
                .get::<EventKey, TimeLimitedChallenge>(&EventKey::Challenge(id))
                .is_some_and(|c| now <= c.end_time + grace);

            if keep {
                retained.push_back(id);
            } else {
                removed += 1;
            }
        }
    }

    env.storage()
        .instance()
        .set(&EventKey::ActiveChallenges, &retained);

    Ok(removed)
}

// ─── Participant Count ─────────────────────────────────────────────────────────

/// Update event participant count (for future integration).
pub fn update_participants(
    env: &Env,
    event_id: u64,
    participant_count: u32,
) -> Result<(), EventError> {
    let mut event: ScheduledEvent = env
        .storage()
        .instance()
        .get(&EventKey::Event(event_id))
        .ok_or(EventError::EventNotFound)?;

    event.participants = participant_count;
    env.storage()
        .instance()
        .set(&EventKey::Event(event_id), &event);

    Ok(())
}

// ─── Read Queries ─────────────────────────────────────────────────────────────

/// Get event details by ID.
pub fn get_event(env: &Env, event_id: u64) -> Result<ScheduledEvent, EventError> {
    env.storage()
        .instance()
        .get(&EventKey::Event(event_id))
        .ok_or(EventError::EventNotFound)
}

/// Get a challenge by ID.
pub fn get_challenge(env: &Env, challenge_id: u64) -> Result<TimeLimitedChallenge, EventError> {
    env.storage()
        .instance()
        .get(&EventKey::Challenge(challenge_id))
        .ok_or(EventError::ChallengeNotFound)
}

/// Get a player's progress on a specific challenge.
pub fn get_challenge_progress(
    env: &Env,
    challenge_id: u64,
    profile_id: u64,
) -> Option<ChallengeProgress> {
    env.storage()
        .persistent()
        .get(&EventKey::ChallengeProgress(challenge_id, profile_id))
}

/// Get all active event IDs.
pub fn get_active_events(env: &Env) -> Vec<u64> {
    env.storage()
        .instance()
        .get(&EventKey::ActiveEvents)
        .unwrap_or(Vec::new(env))
}

/// Schedule a weekly nebula festival (convenience wrapper).
pub fn schedule_weekly_festival(
    env: &Env,
    admin: &Address,
    reward_pool: i128,
) -> Result<u64, EventError> {
    schedule_recurring_event(env, admin, &RecurringEventType::WeeklyFestival, reward_pool)
}

/// Get total number of events scheduled.
pub fn get_event_count(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&EventKey::EventCounter)
        .unwrap_or(0)
}

// ─── Seasonal Events ──────────────────────────────────────────────────────────
//
// Time-limited events that run inside the current season. Each event:
// - is scheduled by the admin for a fixed window within the season,
// - is activated / ended by anyone once its window opens / closes,
// - pays a special seasonal reward pool boosted by the season theme,
// - may carry event-specific challenges that share its window,
// - puts its category on cooldown after it ends, and
// - may be `exclusive`, blocking every other event during its window.

/// Shortest allowed seasonal event window (1 hour).
pub const MIN_SEASONAL_EVENT_DURATION: u64 = 60 * 60;
/// Longest allowed seasonal event window (14 days).
pub const MAX_SEASONAL_EVENT_DURATION: u64 = 14 * 24 * 60 * 60;
/// Longest cooldown that may be configured for an event (30 days).
pub const MAX_SEASONAL_EVENT_COOLDOWN: u64 = 30 * 24 * 60 * 60;
/// Maximum seasonal events that may be scheduled or active at once.
pub const MAX_PENDING_SEASONAL_EVENTS: u32 = 10;
/// Maximum challenges attached to a single seasonal event.
pub const MAX_CHALLENGES_PER_EVENT: u32 = 5;
/// How long after an event ends its rewards can still be claimed (7 days).
pub const SEASONAL_REWARD_CLAIM_WINDOW: u64 = 7 * 24 * 60 * 60;

/// Lifecycle state of a seasonal event.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum SeasonalEventStatus {
    Scheduled,
    Active,
    Ended,
    Cancelled,
}

/// Admin-supplied parameters for a new seasonal event.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct SeasonalEventConfig {
    /// Event category (e.g. `"boss"`, `"surge"`); cooldowns apply per category.
    pub category: Symbol,
    pub start_time: u64,
    pub duration_secs: u64,
    /// Time after the event ends before the category may run again.
    pub cooldown_secs: u64,
    /// Base reward pool before the seasonal bonus is applied.
    pub reward_pool: i128,
    /// Exclusive events cannot overlap any other seasonal event.
    pub exclusive: bool,
}

/// A time-limited seasonal event.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct SeasonalEvent {
    pub event_id: u64,
    pub season_id: u64,
    pub category: Symbol,
    pub start_time: u64,
    pub end_time: u64,
    pub cooldown_secs: u64,
    pub exclusive: bool,
    pub base_reward_pool: i128,
    /// Reward pool after the seasonal theme (and exclusivity) bonus.
    pub reward_pool: i128,
    pub status: SeasonalEventStatus,
    pub participants: u32,
    pub total_points: u64,
    pub rewards_claimed: i128,
    /// Event-specific challenges sharing this event's window.
    pub challenge_ids: Vec<u64>,
}

/// A player's participation in a seasonal event.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct SeasonalEventEntry {
    pub event_id: u64,
    pub profile_id: u64,
    pub points: u64,
    pub claimed: bool,
}

/// Parameters for an event-specific challenge.
#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct SeasonalChallengeSpec {
    pub title: String,
    pub description: String,
    pub target_metric: Symbol,
    pub target_value: u64,
    pub reward_free: i128,
    pub reward_premium: i128,
}

fn load_seasonal_event(env: &Env, event_id: u64) -> Result<SeasonalEvent, EventError> {
    env.storage()
        .instance()
        .get(&EventKey::SeasonalEvent(event_id))
        .ok_or(EventError::EventNotFound)
}

fn save_seasonal_event(env: &Env, event: &SeasonalEvent) {
    env.storage()
        .instance()
        .set(&EventKey::SeasonalEvent(event.event_id), event);
}

fn pending_seasonal_events(env: &Env) -> Vec<u64> {
    env.storage()
        .instance()
        .get(&EventKey::PendingSeasonalEvents)
        .unwrap_or(Vec::new(env))
}

fn remove_pending_seasonal_event(env: &Env, event_id: u64) {
    let mut retained = Vec::new(env);
    for id in pending_seasonal_events(env).iter() {
        if id != event_id {
            retained.push_back(id);
        }
    }
    env.storage()
        .instance()
        .set(&EventKey::PendingSeasonalEvents, &retained);
}

/// Check that a new window does not violate exclusivity or same-category
/// cooldowns against the events that are already scheduled or active.
fn check_window_conflicts(
    env: &Env,
    pending: &Vec<u64>,
    category: &Symbol,
    start: u64,
    end: u64,
    cooldown: u64,
    exclusive: bool,
) -> Result<(), EventError> {
    for id in pending.iter() {
        let other = load_seasonal_event(env, id)?;
        let overlaps = start < other.end_time && other.start_time < end;
        if overlaps && (exclusive || other.exclusive) {
            return Err(EventError::ExclusiveEventConflict);
        }
        if &other.category == category {
            // Same category: one event must finish and cool down before the
            // other starts, whichever order they run in.
            let after_other = start >= other.end_time.saturating_add(other.cooldown_secs);
            let before_other = end.saturating_add(cooldown) <= other.start_time;
            if !after_other && !before_other {
                return Err(EventError::EventCooldownActive);
            }
        }
    }
    Ok(())
}

/// Admin: schedule a time-limited seasonal event within the current season.
///
/// Enforces the category cooldown, exclusivity with overlapping events, and
/// applies the season's special reward bonus to the pool.
pub fn schedule_seasonal_event(
    env: &Env,
    admin: &Address,
    config: SeasonalEventConfig,
) -> Result<u64, EventError> {
    require_admin(env, admin)?;

    let season = crate::seasons::get_current_season(env).map_err(|_| EventError::NoActiveSeason)?;
    let now = get_current_timestamp(env);

    if config.start_time < now {
        return Err(EventError::EventAlreadyPassed);
    }
    if config.duration_secs < MIN_SEASONAL_EVENT_DURATION
        || config.duration_secs > MAX_SEASONAL_EVENT_DURATION
        || config.cooldown_secs > MAX_SEASONAL_EVENT_COOLDOWN
    {
        return Err(EventError::InvalidEventWindow);
    }
    let end_time = config
        .start_time
        .checked_add(config.duration_secs)
        .ok_or(EventError::InvalidEventWindow)?;
    if config.start_time < season.start_time || end_time > season.end_time {
        return Err(EventError::InvalidEventWindow);
    }

    let cooldown_until: u64 = env
        .storage()
        .instance()
        .get(&EventKey::CategoryCooldown(config.category.clone()))
        .unwrap_or(0);
    if config.start_time < cooldown_until {
        return Err(EventError::EventCooldownActive);
    }

    let mut pending = pending_seasonal_events(env);
    if pending.len() >= MAX_PENDING_SEASONAL_EVENTS {
        return Err(EventError::TooManySeasonalEvents);
    }
    check_window_conflicts(
        env,
        &pending,
        &config.category,
        config.start_time,
        end_time,
        config.cooldown_secs,
        config.exclusive,
    )?;

    let reward_pool = crate::seasons::seasonal_event_reward_pool(
        &season.config.theme,
        config.reward_pool,
        config.exclusive,
    )
    .ok_or(EventError::InvalidRewardPool)?;

    let event_id = env
        .storage()
        .instance()
        .get::<EventKey, u64>(&EventKey::SeasonalEventCounter)
        .unwrap_or(0)
        + 1;
    env.storage()
        .instance()
        .set(&EventKey::SeasonalEventCounter, &event_id);

    let event = SeasonalEvent {
        event_id,
        season_id: season.id,
        category: config.category.clone(),
        start_time: config.start_time,
        end_time,
        cooldown_secs: config.cooldown_secs,
        exclusive: config.exclusive,
        base_reward_pool: config.reward_pool,
        reward_pool,
        status: SeasonalEventStatus::Scheduled,
        participants: 0,
        total_points: 0,
        rewards_claimed: 0,
        challenge_ids: Vec::new(env),
    };
    save_seasonal_event(env, &event);

    pending.push_back(event_id);
    env.storage()
        .instance()
        .set(&EventKey::PendingSeasonalEvents, &pending);

    env.events().publish(
        (symbol_short!("sevt_new"), event_id),
        (config.category, config.start_time, end_time, reward_pool),
    );

    Ok(event_id)
}

/// Activate a scheduled event once its window has opened. Callable by anyone
/// (e.g. a keeper bot).
pub fn activate_seasonal_event(env: &Env, event_id: u64) -> Result<SeasonalEvent, EventError> {
    let mut event = load_seasonal_event(env, event_id)?;
    if event.status != SeasonalEventStatus::Scheduled {
        return Err(EventError::InvalidEventState);
    }

    let now = get_current_timestamp(env);
    if now < event.start_time {
        return Err(EventError::EventNotReady);
    }
    if now >= event.end_time {
        return Err(EventError::EventAlreadyPassed);
    }

    event.status = SeasonalEventStatus::Active;
    save_seasonal_event(env, &event);

    env.events()
        .publish((symbol_short!("sevt_on"), event_id), now);

    Ok(event)
}

/// End an event once its window has closed and start its category cooldown.
/// Callable by anyone. An event that was never activated is ended too.
pub fn end_seasonal_event(env: &Env, event_id: u64) -> Result<SeasonalEvent, EventError> {
    let mut event = load_seasonal_event(env, event_id)?;
    if event.status != SeasonalEventStatus::Active && event.status != SeasonalEventStatus::Scheduled
    {
        return Err(EventError::InvalidEventState);
    }

    let now = get_current_timestamp(env);
    if now < event.end_time {
        return Err(EventError::EventNotReady);
    }

    event.status = SeasonalEventStatus::Ended;
    save_seasonal_event(env, &event);
    remove_pending_seasonal_event(env, event_id);

    let cooldown_key = EventKey::CategoryCooldown(event.category.clone());
    let current: u64 = env.storage().instance().get(&cooldown_key).unwrap_or(0);
    let until = event
        .end_time
        .saturating_add(event.cooldown_secs)
        .max(current);
    env.storage().instance().set(&cooldown_key, &until);

    env.events().publish(
        (symbol_short!("sevt_end"), event_id),
        (event.participants, event.total_points, until),
    );

    Ok(event)
}

/// Admin: cancel a scheduled or active event. Cancelled events pay no rewards
/// and do not trigger a cooldown.
pub fn cancel_seasonal_event(env: &Env, admin: &Address, event_id: u64) -> Result<(), EventError> {
    require_admin(env, admin)?;

    let mut event = load_seasonal_event(env, event_id)?;
    if event.status != SeasonalEventStatus::Active && event.status != SeasonalEventStatus::Scheduled
    {
        return Err(EventError::InvalidEventState);
    }

    event.status = SeasonalEventStatus::Cancelled;
    save_seasonal_event(env, &event);
    remove_pending_seasonal_event(env, event_id);

    env.events()
        .publish((symbol_short!("sevt_cnl"), event_id), admin.clone());

    Ok(())
}

/// Admin: attach an event-specific challenge that runs for the event's window.
pub fn add_seasonal_event_challenge(
    env: &Env,
    admin: &Address,
    event_id: u64,
    spec: SeasonalChallengeSpec,
) -> Result<u64, EventError> {
    require_admin(env, admin)?;

    let mut event = load_seasonal_event(env, event_id)?;
    if event.status != SeasonalEventStatus::Active && event.status != SeasonalEventStatus::Scheduled
    {
        return Err(EventError::InvalidEventState);
    }
    if event.challenge_ids.len() >= MAX_CHALLENGES_PER_EVENT {
        return Err(EventError::TooManyChallenges);
    }

    let challenge_id = register_challenge(
        env,
        TimeLimitedChallenge {
            challenge_id: 0,
            title: spec.title,
            description: spec.description,
            season_id: event.season_id,
            start_time: event.start_time,
            end_time: event.end_time,
            target_metric: spec.target_metric,
            target_value: spec.target_value,
            reward_free: spec.reward_free,
            reward_premium: spec.reward_premium,
            participants: 0,
            completed_by: 0,
        },
    )?;

    event.challenge_ids.push_back(challenge_id);
    save_seasonal_event(env, &event);

    Ok(challenge_id)
}

/// Record event points earned by a player while the event is live.
///
/// Returns the player's cumulative points for the event.
pub fn record_seasonal_event_points(
    env: &Env,
    event_id: u64,
    profile_id: u64,
    points: u64,
) -> Result<u64, EventError> {
    let mut event = load_seasonal_event(env, event_id)?;
    if event.status != SeasonalEventStatus::Active {
        return Err(EventError::InvalidEventState);
    }
    let now = get_current_timestamp(env);
    if now >= event.end_time {
        return Err(EventError::EventAlreadyPassed);
    }

    let key = EventKey::SeasonalEventEntry(event_id, profile_id);
    let mut entry: SeasonalEventEntry =
        env.storage()
            .persistent()
            .get(&key)
            .unwrap_or(SeasonalEventEntry {
                event_id,
                profile_id,
                points: 0,
                claimed: false,
            });

    if entry.points == 0 && points > 0 {
        event.participants = event.participants.saturating_add(1);
    }
    entry.points = entry.points.saturating_add(points);
    event.total_points = event.total_points.saturating_add(points);

    env.storage().persistent().set(&key, &entry);
    save_seasonal_event(env, &event);

    Ok(entry.points)
}

/// Claim a player's share of an ended event's seasonal reward pool.
///
/// `reward = reward_pool × player_points / total_points`, rounded down, so the
/// sum of all claims never exceeds the pool.
pub fn claim_seasonal_event_reward(
    env: &Env,
    player: &Address,
    event_id: u64,
    profile_id: u64,
) -> Result<i128, EventError> {
    player.require_auth();

    let mut event = load_seasonal_event(env, event_id)?;
    if event.status != SeasonalEventStatus::Ended {
        return Err(EventError::InvalidEventState);
    }
    let now = get_current_timestamp(env);
    if now > event.end_time.saturating_add(SEASONAL_REWARD_CLAIM_WINDOW) {
        return Err(EventError::ClaimWindowClosed);
    }

    let key = EventKey::SeasonalEventEntry(event_id, profile_id);
    let mut entry: SeasonalEventEntry = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(EventError::NoEventReward)?;
    if entry.claimed {
        return Err(EventError::AlreadyClaimed);
    }
    if entry.points == 0 || event.total_points == 0 {
        return Err(EventError::NoEventReward);
    }

    let reward = event
        .reward_pool
        .checked_mul(i128::from(entry.points))
        .and_then(|v| v.checked_div(i128::from(event.total_points)))
        .ok_or(EventError::InvalidRewardPool)?;
    if reward <= 0 {
        return Err(EventError::NoEventReward);
    }

    entry.claimed = true;
    event.rewards_claimed = event.rewards_claimed.saturating_add(reward);
    env.storage().persistent().set(&key, &entry);
    save_seasonal_event(env, &event);

    env.events()
        .publish((symbol_short!("sevt_clm"), event_id), (profile_id, reward));

    Ok(reward)
}

/// Get a seasonal event by ID.
pub fn get_seasonal_event(env: &Env, event_id: u64) -> Result<SeasonalEvent, EventError> {
    load_seasonal_event(env, event_id)
}

/// IDs of seasonal events that are scheduled or active.
pub fn get_pending_seasonal_events(env: &Env) -> Vec<u64> {
    pending_seasonal_events(env)
}

/// A player's participation in a seasonal event, if any.
pub fn get_seasonal_event_entry(
    env: &Env,
    event_id: u64,
    profile_id: u64,
) -> Option<SeasonalEventEntry> {
    env.storage()
        .persistent()
        .get(&EventKey::SeasonalEventEntry(event_id, profile_id))
}

/// Timestamp before which no new event of `category` may start (0 if none).
pub fn get_category_cooldown(env: &Env, category: Symbol) -> u64 {
    env.storage()
        .instance()
        .get(&EventKey::CategoryCooldown(category))
        .unwrap_or(0)
}
