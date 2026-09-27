/// Gas-optimized storage utilities
/// Provides efficient storage patterns for common operations
use soroban_sdk::{Env, Symbol, Vec};

/// Recommended TTL values for different data types
pub const TTL_TEMPORARY: u32 = 17_280; // 1 day
pub const TTL_PERSISTENT: u32 = 518_400; // 30 days
pub const TTL_PERMANENT: u32 = 3_110_400; // 180 days

/// Batch bump storage entries to reduce gas costs
pub fn batch_bump_persistent(env: &Env, keys: &Vec<Symbol>, ttl: u32) {
    for i in 0..keys.len() {
        let key = keys.get(i).unwrap();
        env.storage().persistent().extend_ttl(&key, ttl, ttl);
    }
}

/// Efficient counter increment with minimal storage operations
pub fn increment_counter(env: &Env, key: Symbol) -> u64 {
    let current: u64 = env.storage().instance().get(&key).unwrap_or(0);
    let next = current + 1;
    env.storage().instance().set(&key, &next);
    next
}

/// Batch increment for multiple counters
pub fn batch_increment_counters(env: &Env, keys: Vec<Symbol>) -> Vec<u64> {
    let mut results = Vec::new(env);
    for key in keys.iter() {
        let next = increment_counter(env, key);
        results.push_back(next);
    }
    results
}

/// Optimized existence check without full read
pub fn exists_optimized(env: &Env, key: &Symbol) -> bool {
    env.storage().persistent().has(key)
}

/// Batch existence checks
pub fn batch_exists(env: &Env, keys: &Vec<Symbol>) -> Vec<bool> {
    let mut results = Vec::new(env);
    for key in keys.iter() {
        results.push_back(env.storage().persistent().has(&key));
    }
    results
}

/// Conditional write - only write if value changed
pub fn conditional_write_u32(
    env: &Env,
    key: Symbol,
    new_value: u32,
) -> bool {
    let current: Option<u32> = env.storage().persistent().get(&key);
    
    match current {
        Some(old) if old == new_value => false, // No change, skip write
        _ => {
            env.storage().persistent().set(&key, &new_value);
            true
        }
    }
}

/// Conditional write for i128
pub fn conditional_write_i128(
    env: &Env,
    key: Symbol,
    new_value: i128,
) -> bool {
    let current: Option<i128> = env.storage().persistent().get(&key);
    
    match current {
        Some(old) if old == new_value => false,
        _ => {
            env.storage().persistent().set(&key, &new_value);
            true
        }
    }
}

/// Packed storage for related u32 values (saves storage slots)
#[derive(Clone, Copy)]
pub struct PackedU32x4 {
    pub packed: u128,
}

impl PackedU32x4 {
    pub fn new(a: u32, b: u32, c: u32, d: u32) -> Self {
        let packed = ((a as u128) << 96)
            | ((b as u128) << 64)
            | ((c as u128) << 32)
            | (d as u128);
        Self { packed }
    }
    
    pub fn get_a(&self) -> u32 {
        (self.packed >> 96) as u32
    }
    
    pub fn get_b(&self) -> u32 {
        (self.packed >> 64) as u32
    }
    
    pub fn get_c(&self) -> u32 {
        (self.packed >> 32) as u32
    }
    
    pub fn get_d(&self) -> u32 {
        self.packed as u32
    }
}

/// Efficient batch read with single storage access pattern
pub fn batch_read_u32(env: &Env, keys: &Vec<Symbol>) -> Vec<Option<u32>> {
    let mut results = Vec::new(env);
    for i in 0..keys.len() {
        let key = keys.get(i).unwrap();
        let value: Option<u32> = env.storage().persistent().get(&key);
        results.push_back(value);
    }
    results
}

/// Efficient batch read for i128
pub fn batch_read_i128(env: &Env, keys: &Vec<Symbol>) -> Vec<Option<i128>> {
    let mut results = Vec::new(env);
    for i in 0..keys.len() {
        let key = keys.get(i).unwrap();
        let value: Option<i128> = env.storage().persistent().get(&key);
        results.push_back(value);
    }
    results
}

/// Efficient batch write with minimal overhead
pub fn batch_write_u32(env: &Env, keys: &Vec<Symbol>, values: &Vec<u32>) {
    let len = keys.len().min(values.len());
    for i in 0..len {
        let key = keys.get(i).unwrap();
        let value = values.get(i).unwrap();
        env.storage().persistent().set(&key, &value);
    }
}

/// Efficient batch write for i128
pub fn batch_write_i128(env: &Env, keys: &Vec<Symbol>, values: &Vec<i128>) {
    let len = keys.len().min(values.len());
    for i in 0..len {
        let key = keys.get(i).unwrap();
        let value = values.get(i).unwrap();
        env.storage().persistent().set(&key, &value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{symbol_short, Env};

    #[test]
    fn test_packed_u32x4() {
        let packed = PackedU32x4::new(1, 2, 3, 4);
        assert_eq!(packed.get_a(), 1);
        assert_eq!(packed.get_b(), 2);
        assert_eq!(packed.get_c(), 3);
        assert_eq!(packed.get_d(), 4);
    }

    #[test]
    fn test_increment_counter() {
        let env = Env::default();
        let key = symbol_short!("counter");
        
        assert_eq!(increment_counter(&env, key.clone()), 1);
        assert_eq!(increment_counter(&env, key.clone()), 2);
        assert_eq!(increment_counter(&env, key), 3);
    }

    #[test]
    fn test_conditional_write() {
        let env = Env::default();
        let key = symbol_short!("test");
        
        // First write should succeed
        assert!(conditional_write_u32(&env, key.clone(), 42u32));
        
        // Same value should skip write
        assert!(!conditional_write_u32(&env, key.clone(), 42u32));
        
        // Different value should write
        assert!(conditional_write_u32(&env, key, 43u32));
    }
}
