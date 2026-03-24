use soroban_sdk::{
    contract, contractimpl, contracttype, contracterror, symbol_short,
    Address, Env, Symbol, IntoVal,
};

// ─── Resource Types ───────────────────────────────────────────────────────────

/// All in-game resource types supported by the economy layer.
/// Future resource types (e.g. DarkMatter) can be appended without breaking
/// existing storage keys because DataKey::Balance holds the variant value.
#[derive(Clone, Debug, PartialEq, Eq)]
#[contracttype]
pub enum ResourceType {
    Stardust = 0,
    Plasma   = 1,
    Crystals = 2,
}

// ─── Storage Keys ─────────────────────────────────────────────────────────────

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    /// Global contract configuration (instance storage)
    Config,
    /// Per-ship daily harvest tracking, keyed by ship_id
    HarvestRecord(u64),
    /// Per-address staking position
    StakeRecord(Address),
    /// Liquid resource balance: (owner, resource_type)
    Balance(Address, ResourceType),
    /// Cumulative minted supply per resource type
    TotalMinted(ResourceType),
}

// ─── Structs ──────────────────────────────────────────────────────────────────

/// Immutable-after-init global configuration pulled from contract storage.
/// Rates (APY, daily cap) are updatable by admin to support future tuning.
#[derive(Clone)]
#[contracttype]
pub struct Config {
    pub admin: Address,
    /// Address of the Ship Registry (NFT) contract
    pub ship_contract: Address,
    /// Address of the Nebula Explorer (generation) contract
    pub nebula_contract: Address,
    /// Annual percentage yield expressed in basis points (e.g. 500 = 5 % APY)
    pub apy_basis_points: u32,
    /// Maximum stardust units harvestable by a single ship per ~day window
    pub daily_harvest_cap: i128,
    /// Minimum ledgers a stake must be held before unstake is permitted
    /// (security time-lock; default ≈ 1 day = 17 280 ledgers at 5 s/ledger)
    pub min_stake_duration: u32,
    /// Base stardust awarded per anomaly harvest before rarity bonus
    pub stardust_per_anomaly: i128,
}

/// Tracks how much a ship has harvested in the current day-window.
#[derive(Clone)]
#[contracttype]
pub struct HarvestRecord {
    /// Ledger sequence at which the daily window last reset
    pub last_reset_ledger: u32,
    /// Stardust harvested since the last reset
    pub harvested_today: i128,
}

/// Records an active staking position for a single address.
/// One position per address enforced at the contract level.
#[derive(Clone)]
#[contracttype]
pub struct StakeRecord {
    pub owner: Address,
    pub resource_type: ResourceType,
    /// Principal locked in this position
    pub amount: i128,
    /// Ledger when stake was opened (used for time-lock)
    pub start_ledger: u32,
    /// Requested lock duration in ledgers
    pub duration_ledgers: u32,
    /// Ledger of the last yield claim (used for pro-rated yield accrual)
    pub last_claim_ledger: u32,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Custom contract errors.  Each variant maps to a u32 code surfaced in
/// transaction results as a JSON-compatible numeric error identifier.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ResourceError {
    /// Contract has not been initialised yet
    NotInitialized        = 1,
    /// init() called more than once
    AlreadyInitialized    = 2,
    /// Caller is not authorised for the requested operation
    Unauthorized          = 3,
    /// Caller's liquid balance is too low (JSON code: {"error":4,"msg":"insufficient_resources"})
    InsufficientResources = 4,
    /// Ship's daily harvest cap has been reached
    DailyCapExceeded      = 5,
    /// No active stake found for the given address
    StakeNotFound         = 6,
    /// Stake time-lock has not yet expired; unstake is blocked
    TimeLockActive        = 7,
    /// Requested duration is below the minimum stake duration
    InvalidDuration       = 8,
    /// Caller does not own the specified ship (cross-contract check failed)
    ShipNotOwned          = 9,
    /// Anomaly index has not been scanned in the nebula contract
    AnomalyNotScanned     = 10,
    /// resource_amount must be > 0
    InvalidAmount         = 11,
    /// Address already has an open staking position
    AlreadyStaked         = 12,
}

// ─── Constants ────────────────────────────────────────────────────────────────

/// Approximate ledger count per 24-hour day (5 s per ledger on Stellar).
pub const LEDGERS_PER_DAY: u32 = 17_280;

/// Basis-points denominator (10 000 bps = 100 %).
const BPS_DENOM: i128 = 10_000;

/// Ledgers per calendar year (365 × 17 280).
const LEDGERS_PER_YEAR: i128 = 17_280_i128 * 365;

// ─── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct ResourceMinter;

#[contractimpl]
impl ResourceMinter {
    // ── Initialisation ───────────────────────────────────────────────────────

    /// Initialise the resource minter.  Must be called exactly once by the
    /// deploying admin.  Rates are stored in instance storage and can be
    /// updated later via `update_daily_cap` / `update_apy`.
    ///
    /// # Parameters
    /// - `apy_basis_points` – e.g. `500` for 5 % APY
    /// - `daily_harvest_cap` – max stardust per ship per ~day
    /// - `min_stake_duration` – ledgers before unstake is allowed (≈ `17280` = 1 day)
    pub fn init(
        env: Env,
        admin: Address,
        ship_contract: Address,
        nebula_contract: Address,
        apy_basis_points: u32,
        daily_harvest_cap: i128,
        min_stake_duration: u32,
    ) -> Result<(), ResourceError> {
        if env.storage().instance().has(&DataKey::Config) {
            return Err(ResourceError::AlreadyInitialized);
        }
        admin.require_auth();

        env.storage().instance().set(
            &DataKey::Config,
            &Config {
                admin,
                ship_contract,
                nebula_contract,
                apy_basis_points,
                daily_harvest_cap,
                min_stake_duration,
                stardust_per_anomaly: 100,
            },
        );
        Ok(())
    }

    // ── Harvesting ───────────────────────────────────────────────────────────

    /// Harvest stardust from a previously-scanned anomaly.
    ///
    /// Logic flow:
    /// 1. `caller.require_auth()` – transaction must be signed by the caller.
    /// 2. Cross-contract call → Ship Registry: `owns_ship(caller, ship_id)`.
    /// 3. Cross-contract call → Nebula Explorer: `has_anomaly(ship_id, anomaly_index)`.
    /// 4. Enforce per-ship daily cap; reset window after `LEDGERS_PER_DAY`.
    /// 5. Mint `stardust_per_anomaly + anomaly_index * 10` stardust (rarity bonus).
    /// 6. Emit `ResourceHarvested` event.
    ///
    /// # Returns
    /// Amount of stardust minted (may be less than raw amount if near daily cap).
    pub fn harvest_resource(
        env: Env,
        caller: Address,
        ship_id: u64,
        anomaly_index: u32,
    ) -> Result<i128, ResourceError> {
        caller.require_auth();
        let config = Self::require_config(&env)?;

        // ── Cross-contract: ship ownership ────────────────────────────────────
        let owns: bool = env.invoke_contract(
            &config.ship_contract,
            &Symbol::new(&env, "owns_ship"),
            soroban_sdk::vec![
                &env,
                caller.clone().into_val(&env),
                ship_id.into_val(&env),
            ],
        );
        if !owns {
            return Err(ResourceError::ShipNotOwned);
        }

        // ── Cross-contract: anomaly validation ────────────────────────────────
        let valid: bool = env.invoke_contract(
            &config.nebula_contract,
            &Symbol::new(&env, "has_anomaly"),
            soroban_sdk::vec![
                &env,
                ship_id.into_val(&env),
                anomaly_index.into_val(&env),
            ],
        );
        if !valid {
            return Err(ResourceError::AnomalyNotScanned);
        }

        // ── Daily cap enforcement ─────────────────────────────────────────────
        let current = env.ledger().sequence();
        let harvest_key = DataKey::HarvestRecord(ship_id);
        let mut rec: HarvestRecord = env
            .storage()
            .persistent()
            .get(&harvest_key)
            .unwrap_or(HarvestRecord {
                last_reset_ledger: 0,
                harvested_today: 0,
            });

        if current.saturating_sub(rec.last_reset_ledger) >= LEDGERS_PER_DAY {
            rec.harvested_today = 0;
            rec.last_reset_ledger = current;
        }

        if rec.harvested_today >= config.daily_harvest_cap {
            return Err(ResourceError::DailyCapExceeded);
        }

        // ── Mint stardust (with anomaly rarity bonus) ─────────────────────────
        let raw_amount = config.stardust_per_anomaly + (anomaly_index as i128 * 10);
        let amount = raw_amount.min(config.daily_harvest_cap - rec.harvested_today);

        let bal_key = DataKey::Balance(caller.clone(), ResourceType::Stardust);
        let prev: i128 = env.storage().persistent().get(&bal_key).unwrap_or(0);
        env.storage().persistent().set(&bal_key, &(prev + amount));

        let total_key = DataKey::TotalMinted(ResourceType::Stardust);
        let total: i128 = env.storage().persistent().get(&total_key).unwrap_or(0);
        env.storage().persistent().set(&total_key, &(total + amount));

        rec.harvested_today += amount;
        env.storage().persistent().set(&harvest_key, &rec);

        // ── Emit ResourceHarvested ────────────────────────────────────────────
        // Topics: ("res_harv", caller)   Data: (ship_id, anomaly_index, amount)
        env.events().publish(
            (symbol_short!("res_harv"), caller.clone()),
            (ship_id, anomaly_index, amount),
        );

        Ok(amount)
    }

    // ── Staking ──────────────────────────────────────────────────────────────

    /// Lock `resource_amount` of `resource_type` for `duration_ledgers` to earn
    /// cosmic essence (Plasma) yield.
    ///
    /// Security: `duration_ledgers` must be ≥ `config.min_stake_duration` to
    /// prevent flash-stake attacks where someone stakes and immediately unstakes
    /// to claim a tiny yield without meaningful lock-up.
    ///
    /// One position per address.  Stake more by calling `unstake` first.
    pub fn stake_for_yield(
        env: Env,
        caller: Address,
        resource_type: ResourceType,
        resource_amount: i128,
        duration_ledgers: u32,
    ) -> Result<(), ResourceError> {
        caller.require_auth();
        let config = Self::require_config(&env)?;

        if resource_amount <= 0 {
            return Err(ResourceError::InvalidAmount);
        }
        if duration_ledgers < config.min_stake_duration {
            return Err(ResourceError::InvalidDuration);
        }
        if env
            .storage()
            .persistent()
            .has(&DataKey::StakeRecord(caller.clone()))
        {
            return Err(ResourceError::AlreadyStaked);
        }

        let bal_key = DataKey::Balance(caller.clone(), resource_type.clone());
        let balance: i128 = env.storage().persistent().get(&bal_key).unwrap_or(0);
        if balance < resource_amount {
            return Err(ResourceError::InsufficientResources);
        }

        // Deduct principal from liquid balance
        env.storage()
            .persistent()
            .set(&bal_key, &(balance - resource_amount));

        let current = env.ledger().sequence();
        env.storage().persistent().set(
            &DataKey::StakeRecord(caller.clone()),
            &StakeRecord {
                owner: caller,
                resource_type,
                amount: resource_amount,
                start_ledger: current,
                duration_ledgers,
                last_claim_ledger: current,
            },
        );
        Ok(())
    }

    /// Claim accumulated cosmic essence (Plasma) yield without unstaking.
    ///
    /// Yield formula (pro-rated APY):
    /// ```text
    /// yield = principal × apy_bps / 10_000 × Δledgers / LEDGERS_PER_YEAR
    /// ```
    ///
    /// Emits: `YieldClaimed` event.
    pub fn claim_yield(env: Env, caller: Address) -> Result<i128, ResourceError> {
        caller.require_auth();
        let config = Self::require_config(&env)?;

        let stake_key = DataKey::StakeRecord(caller.clone());
        let mut stake: StakeRecord = env
            .storage()
            .persistent()
            .get(&stake_key)
            .ok_or(ResourceError::StakeNotFound)?;

        let current = env.ledger().sequence();
        let yield_amount = Self::calculate_yield(&stake, current, config.apy_basis_points);

        if yield_amount > 0 {
            let essence_key = DataKey::Balance(caller.clone(), ResourceType::Plasma);
            let prev: i128 = env.storage().persistent().get(&essence_key).unwrap_or(0);
            env.storage()
                .persistent()
                .set(&essence_key, &(prev + yield_amount));

            stake.last_claim_ledger = current;
            env.storage().persistent().set(&stake_key, &stake);

            // Topics: ("yld_claim", caller)   Data: (staked_amount, yield_amount, ledger)
            env.events().publish(
                (symbol_short!("yld_claim"), caller.clone()),
                (stake.amount, yield_amount, current),
            );
        }

        Ok(yield_amount)
    }

    /// Unstake principal after the time-lock has expired.
    /// Automatically claims any residual yield before returning the principal,
    /// so the caller never loses accrued cosmic essence.
    ///
    /// Security: reverts with `TimeLockActive` if
    /// `current_ledger < start_ledger + min_stake_duration`.
    pub fn unstake(env: Env, caller: Address) -> Result<i128, ResourceError> {
        caller.require_auth();
        let config = Self::require_config(&env)?;

        let stake_key = DataKey::StakeRecord(caller.clone());
        let stake: StakeRecord = env
            .storage()
            .persistent()
            .get(&stake_key)
            .ok_or(ResourceError::StakeNotFound)?;

        let current = env.ledger().sequence();
        if current < stake.start_ledger + config.min_stake_duration {
            return Err(ResourceError::TimeLockActive);
        }

        // Auto-claim residual yield
        let yield_amount = Self::calculate_yield(&stake, current, config.apy_basis_points);
        if yield_amount > 0 {
            let essence_key = DataKey::Balance(caller.clone(), ResourceType::Plasma);
            let prev: i128 = env.storage().persistent().get(&essence_key).unwrap_or(0);
            env.storage()
                .persistent()
                .set(&essence_key, &(prev + yield_amount));

            env.events().publish(
                (symbol_short!("yld_claim"), caller.clone()),
                (stake.amount, yield_amount, current),
            );
        }

        // Return principal to liquid balance
        let bal_key = DataKey::Balance(caller.clone(), stake.resource_type.clone());
        let prev: i128 = env.storage().persistent().get(&bal_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&bal_key, &(prev + stake.amount));

        env.storage().persistent().remove(&stake_key);

        Ok(stake.amount)
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Return the liquid (non-staked) balance for an owner / resource pair.
    pub fn get_balance(env: Env, owner: Address, resource_type: ResourceType) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Balance(owner, resource_type))
            .unwrap_or(0)
    }

    /// Return the active staking record for `owner`, or `None` if not staking.
    pub fn get_stake(env: Env, owner: Address) -> Option<StakeRecord> {
        env.storage()
            .persistent()
            .get(&DataKey::StakeRecord(owner))
    }

    /// Return the current contract configuration.
    pub fn get_config(env: Env) -> Option<Config> {
        env.storage().instance().get(&DataKey::Config)
    }

    /// Preview yield accrued since the last claim without modifying state.
    pub fn get_pending_yield(env: Env, owner: Address) -> Result<i128, ResourceError> {
        let config = Self::require_config(&env)?;
        let stake: StakeRecord = env
            .storage()
            .persistent()
            .get(&DataKey::StakeRecord(owner))
            .ok_or(ResourceError::StakeNotFound)?;
        Ok(Self::calculate_yield(
            &stake,
            env.ledger().sequence(),
            config.apy_basis_points,
        ))
    }

    // ── Admin ─────────────────────────────────────────────────────────────────

    /// Update the configurable daily harvest cap.  Admin-only.
    pub fn update_daily_cap(env: Env, new_cap: i128) -> Result<(), ResourceError> {
        let mut config = Self::require_config(&env)?;
        config.admin.require_auth();
        config.daily_harvest_cap = new_cap;
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    /// Update the APY rate in basis points.  Admin-only.
    pub fn update_apy(env: Env, new_apy_bps: u32) -> Result<(), ResourceError> {
        let mut config = Self::require_config(&env)?;
        config.admin.require_auth();
        config.apy_basis_points = new_apy_bps;
        env.storage().instance().set(&DataKey::Config, &config);
        Ok(())
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn require_config(env: &Env) -> Result<Config, ResourceError> {
        env.storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(ResourceError::NotInitialized)
    }

    /// Pro-rated APY yield calculation.
    ///
    /// ```text
    /// yield = principal × apy_bps / 10_000 × elapsed_ledgers / LEDGERS_PER_YEAR
    /// ```
    ///
    /// Integer division truncates fractional cosmic essence — this is intentional
    /// to keep the contract deterministic and avoid rounding exploits.
    fn calculate_yield(stake: &StakeRecord, current_ledger: u32, apy_bps: u32) -> i128 {
        let elapsed = current_ledger.saturating_sub(stake.last_claim_ledger) as i128;
        if elapsed == 0 {
            return 0;
        }
        stake.amount * (apy_bps as i128) * elapsed / (BPS_DENOM * LEDGERS_PER_YEAR)
    }
}
