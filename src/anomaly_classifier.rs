//! Classification of suspicious gameplay and telemetry anomalies.
//!
use soroban_sdk::{contracterror, contracttype, symbol_short, Env, Symbol, Vec};

#[derive(Clone)]
#[contracttype]
pub enum AnomalyKey {
    Classification(u64),
}

#[derive(Clone, Debug, PartialEq)]
#[contracttype]
pub struct ClassificationRecord {
    pub anomaly_id: u64,
    pub anomaly_type: Symbol,
    pub confidence: u32,
    pub last_updated: u64,
    pub scan_count: u32,
}

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AnomalyError {
    InsufficientFeatures = 1,
    NotFound = 2,
    Unauthorized = 3,
}

impl crate::error_standard::StandardContractError for AnomalyError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::InsufficientFeatures => (ErrorKind::ResourceLimit, false),
            Self::NotFound => (ErrorKind::NotFound, false),
            Self::Unauthorized => (ErrorKind::Authorization, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "anomaly_classifier",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

pub fn classify_anomaly(
    env: &Env,
    anomaly_id: u64,
    features: Vec<u32>,
) -> Result<ClassificationRecord, AnomalyError> {
    if features.len() < 3 {
        return Err(AnomalyError::InsufficientFeatures);
    }

    let mut score = 0u32;
    for f in features.iter() {
        score = score.saturating_add(f);
    }

    let anomaly_type = if score > 200 {
        symbol_short!("blackhole")
    } else if score > 120 {
        symbol_short!("wormhole")
    } else {
        symbol_short!("nebula")
    };

    let confidence = core::cmp::min(100, score / 3);
    let record = ClassificationRecord {
        anomaly_id,
        anomaly_type: anomaly_type.clone(),
        confidence,
        last_updated: env.ledger().timestamp(),
        scan_count: 1,
    };

    env.storage()
        .instance()
        .set(&AnomalyKey::Classification(anomaly_id), &record);

    env.events().publish(
        (symbol_short!("anomaly"), symbol_short!("classify")),
        (anomaly_id, anomaly_type, confidence),
    );

    Ok(record)
}

pub fn classify_batch(
    env: &Env,
    records: Vec<(u64, Vec<u32>)>,
) -> Vec<ClassificationRecord> {
    let mut out = Vec::new(env);

    for rec in records.into_iter() {
        let (id, features) = rec;
        if let Ok(classified) = classify_anomaly(env, id, features.clone()) {
            out.push_back(classified);
        }
    }

    out
}

pub fn refine_classification(
    env: &Env,
    anomaly_id: u64,
    new_data: Vec<u32>,
) -> Result<ClassificationRecord, AnomalyError> {
    let mut existing = env
        .storage()
        .instance()
        .get::<AnomalyKey, ClassificationRecord>(&AnomalyKey::Classification(anomaly_id))
        .ok_or(AnomalyError::NotFound)?;

    if new_data.len() < 1 {
        return Err(AnomalyError::InsufficientFeatures);
    }

    let mut score = existing.confidence * existing.scan_count;
    let mut new_score = 0u32;
    for f in new_data.iter() {
        new_score = new_score.saturating_add(f);
    }
    score = score.saturating_add(new_score);
    existing.scan_count = existing.scan_count.saturating_add(1);
    existing.confidence = core::cmp::min(100, score / existing.scan_count);
    existing.last_updated = env.ledger().timestamp();
    existing.anomaly_type = if existing.confidence > 80 {
        symbol_short!("wormhole")
    } else if existing.confidence > 50 {
        symbol_short!("nebula")
    } else {
        symbol_short!("anomaly")
    };

    env.storage()
        .instance()
        .set(&AnomalyKey::Classification(anomaly_id), &existing);

    env.events().publish(
        (symbol_short!("anomaly"), symbol_short!("refined")),
        (anomaly_id, existing.anomaly_type.clone(), existing.confidence),
    );

    Ok(existing)
}

pub fn get_classification(env: &Env, anomaly_id: u64) -> Option<ClassificationRecord> {
    env.storage()
        .instance()
        .get::<AnomalyKey, ClassificationRecord>(&AnomalyKey::Classification(anomaly_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, vec, Address, Env};

    #[contract]
    struct Stub;
    #[contractimpl]
    impl Stub {}

    fn make_env() -> (Env, Address) {
        let env = Env::default();
        let id = env.register_contract(None, Stub);
        (env, id)
    }

    #[test]
    fn test_classify_anomaly_rejects_too_few_features() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            let result = classify_anomaly(&env, 1, vec![&env, 1u32, 2u32]);
            assert_eq!(result, Err(AnomalyError::InsufficientFeatures));
        });
    }

    #[test]
    fn test_classify_anomaly_boundary_scores() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            // score exactly 120 -> "nebula" (not > 120)
            let r = classify_anomaly(&env, 1, vec![&env, 40u32, 40u32, 40u32]).unwrap();
            assert_eq!(r.anomaly_type, symbol_short!("nebula"));

            // score exactly 201 -> "blackhole" (> 200)
            let r = classify_anomaly(&env, 2, vec![&env, 100u32, 100u32, 1u32]).unwrap();
            assert_eq!(r.anomaly_type, symbol_short!("blackhole"));

            // score exactly 121 -> "wormhole" (> 120, <= 200)
            let r = classify_anomaly(&env, 3, vec![&env, 100u32, 20u32, 1u32]).unwrap();
            assert_eq!(r.anomaly_type, symbol_short!("wormhole"));
        });
    }

    #[test]
    fn test_classify_anomaly_saturates_on_overflow() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            let r = classify_anomaly(&env, 1, vec![&env, u32::MAX, u32::MAX, u32::MAX]).unwrap();
            // confidence must clamp to 100, never overflow/panic.
            assert_eq!(r.confidence, 100);
        });
    }

    #[test]
    fn test_refine_classification_missing_record() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            let result = refine_classification(&env, 999, vec![&env, 1u32]);
            assert_eq!(result, Err(AnomalyError::NotFound));
        });
    }

    #[test]
    fn test_refine_classification_rejects_empty_data() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            classify_anomaly(&env, 1, vec![&env, 1u32, 2u32, 3u32]).unwrap();
            let result = refine_classification(&env, 1, vec![&env]);
            assert_eq!(result, Err(AnomalyError::InsufficientFeatures));
        });
    }

    #[test]
    fn test_classify_batch_skips_invalid_entries() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            let batch = vec![
                &env,
                (1u64, vec![&env, 1u32, 2u32, 3u32]),
                (2u64, vec![&env, 1u32]), // invalid: too few features, silently skipped
            ];
            let out = classify_batch(&env, batch);
            assert_eq!(out.len(), 1);
            assert_eq!(out.get(0).unwrap().anomaly_id, 1);
            assert!(get_classification(&env, 2).is_none());
        });
    }

    #[test]
    fn test_get_classification_missing_returns_none() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            assert!(get_classification(&env, 42).is_none());
        });
    }
}
