use soroban_sdk::{contracterror, contracttype, symbol_short, Address, BytesN, Env, String, Symbol};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum GovError {
    VotingClosed = 1,
    AlreadyVoted = 2,
    ProposalNotFound = 3,
    QuorumNotMet = 4,
    InsufficientEssence = 5,
    NotDao = 6,
    NotAdmin = 7,
}

impl crate::error_standard::StandardContractError for GovError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::VotingClosed | Self::AlreadyVoted | Self::QuorumNotMet => {
                (ErrorKind::Conflict, false)
            }
            Self::ProposalNotFound => (ErrorKind::NotFound, false),
            Self::InsufficientEssence => (ErrorKind::ResourceLimit, false),
            Self::NotDao | Self::NotAdmin => (ErrorKind::Authorization, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "governance",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub id: u64,
    pub description: String,
    pub param_change: BytesN<128>,
    pub creator: Address,
    pub expiration: u64,
    pub for_votes: i128,
    pub against_votes: i128,
    pub status: ProposalStatus,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProposalStatus {
    Active,
    Passed,
    Failed,
    Executed,
}

const QUORUM: i128 = 100_000_000_000; // 100k essence example
const VOTING_PERIOD: u64 = 86400 * 3; // 3 days

// ─── Storage Keys ─────────────────────────────────────────────────────────

#[derive(Clone)]
#[contracttype]
enum GovernanceDataKey {
    Admin,
    DaoContract,
    GameParameter(Symbol),
}

pub fn create_proposal(env: Env, creator: Address, description: String, param_change: BytesN<128>) -> Result<u64, GovError> {
    creator.require_auth();

    let mut proposal_id = env.storage().instance().get::<_, u64>(&symbol_short!("next_gid")).unwrap_or(0);
    
    let proposal = Proposal {
        id: proposal_id,
        description,
        param_change,
        creator: creator.clone(),
        expiration: env.ledger().timestamp() + VOTING_PERIOD,
        for_votes: 0,
        against_votes: 0,
        status: ProposalStatus::Active,
    };

    env.storage().persistent().set(&proposal_id, &proposal);
    env.storage().instance().set(&symbol_short!("next_gid"), &(proposal_id + 1));

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("prop_crtd")),
        (proposal_id, creator),
    );

    Ok(proposal_id)
}

pub fn cast_vote(env: Env, voter: Address, proposal_id: u64, support: bool, essence_weight: i128) -> Result<(), GovError> {
    voter.require_auth();

    let mut proposal: Proposal = env.storage().persistent().get(&proposal_id).ok_or(GovError::ProposalNotFound)?;
    
    if env.ledger().timestamp() > proposal.expiration {
        return Err(GovError::VotingClosed);
    }

    let vote_key = (proposal_id, voter.clone());
    if env.storage().persistent().has(&vote_key) {
        return Err(GovError::AlreadyVoted);
    }

    if support {
        proposal.for_votes += essence_weight;
    } else {
        proposal.against_votes += essence_weight;
    }

    env.storage().persistent().set(&proposal_id, &proposal);
    env.storage().persistent().set(&vote_key, &true);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("vote_cast")),
        (proposal_id, voter, support, essence_weight),
    );

    Ok(())
}

pub fn finalize_proposal(env: Env, proposal_id: u64) -> Result<ProposalStatus, GovError> {
    let mut proposal: Proposal = env.storage().persistent().get(&proposal_id).ok_or(GovError::ProposalNotFound)?;
    
    if env.ledger().timestamp() <= proposal.expiration {
        return Err(GovError::VotingClosed); // Still active
    }

    if proposal.for_votes + proposal.against_votes < QUORUM {
        proposal.status = ProposalStatus::Failed;
    } else if proposal.for_votes > proposal.against_votes {
        proposal.status = ProposalStatus::Passed;
    } else {
        proposal.status = ProposalStatus::Failed;
    }

    env.storage().persistent().set(&proposal_id, &proposal);
    Ok(proposal.status.clone())
}

/// Set a game parameter. Only callable by the DAO contract.
/// This allows the DAO to modify game parameters via proposal execution.
pub fn set_game_parameter(
    env: Env,
    caller: Address,
    parameter_key: Symbol,
    parameter_value: i128,
) -> Result<(), GovError> {
    caller.require_auth();

    // Security: Check caller is the DAO contract. This check MUST come first.
    let dao_address: Address = env
        .storage()
        .instance()
        .get(&GovernanceDataKey::DaoContract)
        .ok_or(GovError::NotDao)?;

    if caller != dao_address {
        return Err(GovError::NotDao);
    }

    env.storage().instance().set(
        &GovernanceDataKey::GameParameter(parameter_key.clone()),
        &parameter_value,
    );

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("param")),
        (parameter_key, parameter_value),
    );

    Ok(())
}

/// Get the current value of a game parameter.
pub fn get_game_parameter(env: Env, parameter_key: Symbol) -> Option<i128> {
    env.storage()
        .instance()
        .get(&GovernanceDataKey::GameParameter(parameter_key))
}

/// Register the DAO contract address. Can only be called by admin during initialization.
pub fn set_dao_contract(env: Env, admin: Address, dao_address: Address) -> Result<(), GovError> {
    admin.require_auth();

    // Verify caller is admin.
    let stored_admin: Address = env
        .storage()
        .instance()
        .get(&GovernanceDataKey::Admin)
        .ok_or(GovError::NotAdmin)?;

    if admin != stored_admin {
        return Err(GovError::NotAdmin);
    }

    env.storage()
        .instance()
        .set(&GovernanceDataKey::DaoContract, &dao_address);

    env.events().publish(
        (symbol_short!("gov"), symbol_short!("dao_set")),
        (dao_address,),
    );

    Ok(())
}
