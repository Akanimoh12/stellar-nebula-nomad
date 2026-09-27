# Comprehensive Developer Onboarding Guide

Welcome to `stellar-nebula-nomad`! This guide provides end-to-end instructions for setting up your local environment, building smart contracts, running test suites, deploying to Stellar Soroban networks, and troubleshooting common issues.

---

## 1. Prerequisites & Installation

### Required Toolchain Components
Ensure you have the following installed on your host system:

1. **Rust Toolchain (1.75+ recommended)**:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   rustup update stable
   ```

2. **WebAssembly Compilation Target**:
   ```bash
   rustup target add wasm32-unknown-unknown
   ```

3. **Soroban CLI**:
   ```bash
   cargo install --locked soroban-cli
   ```

4. **Docker & Docker Compose** (for running local Soroban standalone node):
   - Install [Docker Desktop](https://www.docker.com/products/docker-desktop/).

---

## 2. Setting Up the Development Workspace

Clone the repository and verify your setup:

```bash
git clone https://github.com/Space-Nebula/stellar-nebula-nomad.git
cd stellar-nebula-nomad
cargo check --locked
```

---

## 3. Building, Testing, and Linting

### Compilation Commands
- **Check Compilation**:
  ```bash
  cargo check --locked
  ```

- **Build Release WebAssembly Binary**:
  ```bash
  cargo build --target wasm32-unknown-unknown --release
  ```

- **Optimize WASM Size**:
  ```bash
  soroban contract optimize --wasm target/wasm32-unknown-unknown/release/stellar_nebula_nomad.wasm
  ```

### Test Suite Execution
- **Run Unit Tests**:
  ```bash
  cargo test --locked
  ```

- **Run Specific Test Module**:
  ```bash
  cargo test --test contract_tests
  ```

### Code Formatting & Linting
- **Clippy Analysis**:
  ```bash
  cargo clippy --all-targets -- -D warnings
  ```

- **Rustfmt Verification**:
  ```bash
  cargo fmt --check
  ```

- **Documentation Build Verification**:
  ```bash
  cargo doc --no-deps
  ```

---

## 4. Deployment Instructions

### Deploying to Soroban Testnet

1. **Configure Soroban CLI Network**:
   ```bash
   soroban network add --rpc-url https://soroban-testnet.stellar.org:443 --network-passphrase "Test SDF Network ; September 2015" testnet
   ```

2. **Generate / Fund Identity**:
   ```bash
   soroban keys generate developer-identity
   soroban keys fund developer-identity --network testnet
   ```

3. **Deploy WASM Binary**:
   ```bash
   soroban contract deploy \
     --wasm target/wasm32-unknown-unknown/release/stellar_nebula_nomad.wasm \
     --source developer-identity \
     --network testnet
   ```

---

## 5. Code Examples for Common Developer Tasks

### Example 1: Defining Storage Keys & Contract Methods
```rust
use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env, Symbol};

#[contracttype]
pub enum DataKey {
    Admin,
    Counter,
}

#[contract]
pub struct NomadContract;

#[contractimpl]
impl NomadContract {
    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Counter, &0u32);
    }

    pub fn increment(env: Env) -> u32 {
        let mut count: u32 = env.storage().instance().get(&DataKey::Counter).unwrap_or(0);
        count += 1;
        env.storage().instance().set(&DataKey::Counter, &count);
        count
    }
}
```

### Example 2: Writing Soroban Unit Tests
```rust
#[test]
fn test_contract_initialization() {
    let env = Env::default();
    env.mock_all_signatures();

    let contract_id = env.register_contract(None, NomadContract);
    let client = NomadContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);

    client.initialize(&admin);
    let val = client.increment();
    assert_eq!(val, 1);
}
```

---

## 6. Common Troubleshooting & FAQs

| Issue / Error | Cause | Resolution |
| :--- | :--- | :--- |
| `error[E0463]: can't find crate for std` when building WASM | Missing `wasm32-unknown-unknown` target | Run `rustup target add wasm32-unknown-unknown`. |
| `soroban: command not found` | Cargo binary directory not in system `PATH` | Ensure `~/.cargo/bin` is added to your shell PATH variable. |
| Dependency mismatch on `soroban-sdk` | Unlocked dependencies pulling breaking versions | Always build and test using `cargo check --locked` and `cargo test --locked`. |
| `Error: Storage limit exceeded` during test execution | Soroban ledger storage footprint exceeded | Optimize storage keys using `symbol_short!` and cleanup unused persistent data. |
| Clippy warnings on unused returns | Result returned from contract function ignored | Wrap calls with `let _ = ...` or explicitly return `Result<(), Error>`. |
