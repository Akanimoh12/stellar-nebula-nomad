use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env};

const WEEKLY_GAS_THRESHOLD: u64 = 10_000;
const CO2_PER_GAS: u64 = 42; // 42 gCO2 per gas unit approximated

#[derive(Clone)]
#[contracttype]
pub enum SustainabilityKey {
    Footprint(Address),
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct FootprintRecord {
    pub gas_used: u64,
    pub co2_emissions: u64,
    pub tx_count: u32,
    pub last_update_ts: u64,
}

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SustainabilityError {
    NoRewardEligible = 1,
    InvalidGasValue = 2,
    Unauthorized = 3,
}

impl crate::error_standard::StandardContractError for SustainabilityError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::NoRewardEligible | Self::Unauthorized => (ErrorKind::Authorization, false),
            Self::InvalidGasValue => (ErrorKind::Validation, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "sustainability_metrics",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

pub fn record_transaction_footprint(
    env: &Env,
    player: &Address,
    gas_used: u64,
) -> Result<FootprintRecord, SustainabilityError> {
    if gas_used == 0 {
        return Err(SustainabilityError::InvalidGasValue);
    }

    player.require_auth();

    let mut record = env
        .storage()
        .instance()
        .get::<SustainabilityKey, FootprintRecord>(&SustainabilityKey::Footprint(player.clone()))
        .unwrap_or(FootprintRecord {
            gas_used: 0,
            co2_emissions: 0,
            tx_count: 0,
            last_update_ts: env.ledger().timestamp(),
        });

    record.gas_used = record.gas_used.saturating_add(gas_used);
    let added_co2 = gas_used.saturating_mul(CO2_PER_GAS) / 1000;
    record.co2_emissions = record.co2_emissions.saturating_add(added_co2);
    record.tx_count = record.tx_count.saturating_add(1);
    record.last_update_ts = env.ledger().timestamp();

    env.storage()
        .instance()
        .set(&SustainabilityKey::Footprint(player.clone()), &record);

    env.events().publish(
        (symbol_short!("sust"), symbol_short!("footprnt")),
        (player.clone(), record.gas_used, record.co2_emissions, record.tx_count),
    );

    Ok(record)
}

pub fn claim_sustainability_reward(
    env: &Env,
    player: &Address,
) -> Result<i128, SustainabilityError> {
    player.require_auth();

    let record = env
        .storage()
        .instance()
        .get::<SustainabilityKey, FootprintRecord>(&SustainabilityKey::Footprint(player.clone()))
        .unwrap_or(FootprintRecord {
            gas_used: 0,
            co2_emissions: 0,
            tx_count: 0,
            last_update_ts: env.ledger().timestamp(),
        });

    if record.gas_used < WEEKLY_GAS_THRESHOLD {
        let reward = (WEEKLY_GAS_THRESHOLD - record.gas_used) as i128 / 100;
        if reward > 0 {
            env.storage()
                .instance()
                .remove(&SustainabilityKey::Footprint(player.clone()));
            env.events().publish(
                (symbol_short!("sust"), symbol_short!("ecorew")),
                (player.clone(), reward),
            );
            return Ok(reward);
        }
    }

    Err(SustainabilityError::NoRewardEligible)
}

pub fn get_footprint(env: &Env, player: &Address) -> FootprintRecord {
    env.storage()
        .instance()
        .get::<SustainabilityKey, FootprintRecord>(&SustainabilityKey::Footprint(player.clone()))
        .unwrap_or(FootprintRecord {
            gas_used: 0,
            co2_emissions: 0,
            tx_count: 0,
            last_update_ts: env.ledger().timestamp(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, testutils::Address as _};

    #[contract]
    struct Stub;
    #[contractimpl]
    impl Stub {}

    fn make_env() -> (Env, Address) {
        let env = Env::default();
        env.mock_all_auths();
        let id = env.register_contract(None, Stub);
        (env, id)
    }

    #[test]
    fn test_record_transaction_footprint_rejects_zero_gas() {
        let (env, contract_id) = make_env();
        let player = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = record_transaction_footprint(&env, &player, 0);
            assert_eq!(result, Err(SustainabilityError::InvalidGasValue));
        });
    }

    #[test]
    fn test_record_transaction_footprint_accumulates() {
        let (env, contract_id) = make_env();
        let player = Address::generate(&env);
        env.as_contract(&contract_id, || {
            record_transaction_footprint(&env, &player, 100).unwrap();
            let rec = record_transaction_footprint(&env, &player, 50).unwrap();
            assert_eq!(rec.gas_used, 150);
            assert_eq!(rec.tx_count, 2);
            assert_eq!(rec.co2_emissions, (100 * 42 / 1000) + (50 * 42 / 1000));
        });
    }

    #[test]
    fn test_get_footprint_defaults_to_zero_when_unset() {
        let (env, contract_id) = make_env();
        let player = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let rec = get_footprint(&env, &player);
            assert_eq!(rec.gas_used, 0);
            assert_eq!(rec.tx_count, 0);
        });
    }

    #[test]
    fn test_claim_sustainability_reward_below_threshold_is_eligible() {
        let (env, contract_id) = make_env();
        let player = Address::generate(&env);
        env.as_contract(&contract_id, || {
            record_transaction_footprint(&env, &player, 100).unwrap();
            let reward = claim_sustainability_reward(&env, &player).unwrap();
            assert_eq!(reward, (WEEKLY_GAS_THRESHOLD - 100) as i128 / 100);
        });
    }

    #[test]
    fn test_claim_sustainability_reward_at_threshold_boundary_ineligible() {
        let (env, contract_id) = make_env();
        let player = Address::generate(&env);
        env.as_contract(&contract_id, || {
            record_transaction_footprint(&env, &player, WEEKLY_GAS_THRESHOLD).unwrap();
            let result = claim_sustainability_reward(&env, &player);
            assert_eq!(result, Err(SustainabilityError::NoRewardEligible));
        });
    }

    #[test]
    fn test_claim_sustainability_reward_never_recorded_still_eligible() {
        let (env, contract_id) = make_env();
        let player = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let reward = claim_sustainability_reward(&env, &player).unwrap();
            assert_eq!(reward, WEEKLY_GAS_THRESHOLD as i128 / 100);
        });
    }
}
