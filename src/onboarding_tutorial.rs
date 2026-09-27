use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Vec};

pub const TOTAL_STEPS: u32 = 5;
pub const STEP_REWARDS: [i128; 5] = [25, 35, 50, 65, 100];

#[derive(Clone)]
#[contracttype]
pub struct TutorialProgress {
    pub next_step: u32,
    pub completed_mask: u32,
    pub completed_count: u32,
    pub started_at: u64,
    pub completed_at: u64,
}

#[derive(Clone)]
#[contracttype]
pub struct PlayerProfile {
    pub created_at: u64,
    pub tutorial_started: bool,
}

#[derive(Copy, Clone, Eq, PartialEq)]
#[repr(u32)]
#[contracterror]
pub enum OnboardingError {
    AlreadyInitialized = 1,
    ProfileAlreadyExists = 2,
    ProfileNotFound = 3,
    TutorialAlreadyStarted = 4,
    TutorialNotStarted = 5,
    InvalidStep = 6,
    StepOutOfOrder = 7,
    StepAlreadyCompleted = 8,
    TutorialAlreadyCompleted = 9,
    InvalidPath = 10,
    Unauthorized = 11,
}

impl crate::error_standard::StandardContractError for OnboardingError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::AlreadyInitialized
            | Self::ProfileAlreadyExists
            | Self::TutorialAlreadyStarted
            | Self::TutorialNotStarted
            | Self::StepOutOfOrder
            | Self::StepAlreadyCompleted
            | Self::TutorialAlreadyCompleted => (ErrorKind::Conflict, false),
            Self::ProfileNotFound => (ErrorKind::NotFound, false),
            Self::InvalidStep | Self::InvalidPath => (ErrorKind::Validation, false),
            Self::Unauthorized => (ErrorKind::Authorization, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "onboarding_tutorial",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

fn progress_key(player: &Address) -> (soroban_sdk::Symbol, Address) {
    (symbol_short!("onb_prg"), player.clone())
}

fn profile_key(player: &Address) -> (soroban_sdk::Symbol, Address) {
    (symbol_short!("onb_pfl"), player.clone())
}

fn resources_key(player: &Address) -> (soroban_sdk::Symbol, Address) {
    (symbol_short!("onb_res"), player.clone())
}

fn init_key() -> soroban_sdk::Symbol {
    symbol_short!("onb_init")
}

fn admin_key() -> soroban_sdk::Symbol {
    symbol_short!("onb_adm")
}

fn path_key() -> soroban_sdk::Symbol {
    symbol_short!("onb_path")
}

fn default_path(env: &Env) -> Vec<u32> {
    let mut path = Vec::new(env);
    for i in 0..TOTAL_STEPS {
        path.push_back(i);
    }
    path
}

fn ensure_initialized(env: &Env) {
    if !env.storage().persistent().has(&init_key()) {
        env.storage()
            .persistent()
            .set(&path_key(), &default_path(env));
        env.storage().persistent().set(&init_key(), &true);
    }
}

fn reward_for_step(step_id: u32) -> i128 {
    STEP_REWARDS[step_id as usize]
}

pub fn init_onboarding(env: &Env, admin: &Address) -> Result<(), OnboardingError> {
    admin.require_auth();
    if env.storage().persistent().has(&admin_key()) {
        return Err(OnboardingError::AlreadyInitialized);
    }
    ensure_initialized(env);
    env.storage().persistent().set(&admin_key(), admin);
    Ok(())
}

pub fn create_profile(env: &Env, player: &Address) -> Result<(), OnboardingError> {
    player.require_auth();
    ensure_initialized(env);

    let p_key = profile_key(player);
    if env.storage().persistent().has(&p_key) {
        return Err(OnboardingError::ProfileAlreadyExists);
    }

    let profile = PlayerProfile {
        created_at: env.ledger().timestamp(),
        tutorial_started: true,
    };
    env.storage().persistent().set(&p_key, &profile);

    start_tutorial_internal(env, player)
}

pub fn start_tutorial(env: &Env, player: &Address) -> Result<(), OnboardingError> {
    player.require_auth();
    start_tutorial_internal(env, player)
}

fn start_tutorial_internal(env: &Env, player: &Address) -> Result<(), OnboardingError> {
    ensure_initialized(env);

    let p_key = profile_key(player);
    if !env.storage().persistent().has(&p_key) {
        return Err(OnboardingError::ProfileNotFound);
    }

    let key = progress_key(player);
    if env.storage().persistent().has(&key) {
        return Err(OnboardingError::TutorialAlreadyStarted);
    }

    let progress = TutorialProgress {
        next_step: 0,
        completed_mask: 0,
        completed_count: 0,
        started_at: env.ledger().timestamp(),
        completed_at: 0,
    };

    env.storage().persistent().set(&key, &progress);
    env.events().publish(
        (symbol_short!("tutorial"), symbol_short!("started")),
        player.clone(),
    );

    Ok(())
}

pub fn complete_tutorial_step(
    env: &Env,
    player: &Address,
    step_id: u32,
) -> Result<i128, OnboardingError> {
    player.require_auth();
    ensure_initialized(env);

    if step_id >= TOTAL_STEPS {
        return Err(OnboardingError::InvalidStep);
    }

    let key = progress_key(player);
    let mut progress = env
        .storage()
        .persistent()
        .get::<_, TutorialProgress>(&key)
        .ok_or(OnboardingError::TutorialNotStarted)?;

    if progress.completed_count >= TOTAL_STEPS {
        return Err(OnboardingError::TutorialAlreadyCompleted);
    }

    if (progress.completed_mask & (1 << step_id)) != 0 {
        return Err(OnboardingError::StepAlreadyCompleted);
    }

    // Proof requirement: strict linear progression against path map.
    let path = env
        .storage()
        .persistent()
        .get::<_, Vec<u32>>(&path_key())
        .unwrap_or(default_path(env));

    let expected_step = path
        .get(progress.next_step)
        .ok_or(OnboardingError::InvalidPath)?;

    if step_id != expected_step {
        return Err(OnboardingError::StepOutOfOrder);
    }

    progress.completed_mask |= 1 << step_id;
    progress.completed_count += 1;
    progress.next_step += 1;

    let reward = reward_for_step(step_id);
    let r_key = resources_key(player);
    let current = env
        .storage()
        .persistent()
        .get::<_, i128>(&r_key)
        .unwrap_or(0);
    env.storage().persistent().set(&r_key, &(current + reward));

    if progress.completed_count == TOTAL_STEPS {
        progress.completed_at = env.ledger().timestamp();
        env.events().publish(
            (symbol_short!("tutorial"), symbol_short!("done")),
            (player.clone(), current + reward),
        );
    }

    env.storage().persistent().set(&key, &progress);

    Ok(reward)
}

pub fn get_tutorial_progress(env: &Env, player: &Address) -> Option<TutorialProgress> {
    env.storage().persistent().get(&progress_key(player))
}

pub fn get_starter_resources(env: &Env, player: &Address) -> i128 {
    env.storage()
        .persistent()
        .get::<_, i128>(&resources_key(player))
        .unwrap_or(0)
}

pub fn set_tutorial_path(
    env: &Env,
    admin: &Address,
    path: Vec<u32>,
) -> Result<(), OnboardingError> {
    admin.require_auth();
    ensure_initialized(env);

    let stored_admin = env
        .storage()
        .persistent()
        .get::<_, Address>(&admin_key())
        .ok_or(OnboardingError::Unauthorized)?;

    if stored_admin != *admin {
        return Err(OnboardingError::Unauthorized);
    }

    if path.len() != TOTAL_STEPS {
        return Err(OnboardingError::InvalidPath);
    }

    env.storage().persistent().set(&path_key(), &path);
    Ok(())
}
