use soroban_sdk::{testutils::Address as _, Address, Env};
use stellar_nebula_nomad::{
    generate_nebula_layout, harvest_resources, mint_ship, scan_nebula, NebulaLayout, ShipNft,
};

/// Benchmark nebula generation gas usage
#[test]
fn bench_nebula_generation() {
    let env = Env::default();
    let player = Address::generate(&env);
    let seed = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(&env, &[1u8; 32]));

    env.budget().reset_unlimited();
    let layout = generate_nebula_layout(env.clone(), seed.clone(), player.clone());

    let cpu_insns = env.budget().cpu_instruction_cost();
    let mem_bytes = env.budget().memory_bytes_cost();

    println!("Nebula Generation:");
    println!("  CPU instructions: {}", cpu_insns);
    println!("  Memory bytes: {}", mem_bytes);

    // Target: < 1M CPU instructions
    assert!(
        cpu_insns < 1_000_000,
        "Nebula generation exceeds CPU target"
    );
}

/// Benchmark scan operation gas usage
#[test]
fn bench_scan_nebula() {
    let env = Env::default();
    let player = Address::generate(&env);
    let seed = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(&env, &[1u8; 32]));

    env.budget().reset_unlimited();
    let _result = scan_nebula(env.clone(), seed, player);

    let cpu_insns = env.budget().cpu_instruction_cost();
    let mem_bytes = env.budget().memory_bytes_cost();

    println!("Scan Nebula:");
    println!("  CPU instructions: {}", cpu_insns);
    println!("  Memory bytes: {}", mem_bytes);

    // Target: < 2M CPU instructions
    assert!(cpu_insns < 2_000_000, "Scan exceeds CPU target");
}

/// Benchmark harvest operation gas usage
#[test]
fn bench_harvest_resources() {
    let env = Env::default();
    env.budget().reset_unlimited();

    let player = Address::generate(&env);
    let seed = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(&env, &[1u8; 32]));

    // Setup: mint ship and generate layout
    let ship = mint_ship(
        env.clone(),
        player.clone(),
        soroban_sdk::symbol_short!("fighter"),
        soroban_sdk::Bytes::new(&env),
    )
    .unwrap();

    let layout = generate_nebula_layout(env.clone(), seed, player.clone());

    env.budget().reset_unlimited();
    let _result = harvest_resources(&env, ship.id, &layout);

    let cpu_insns = env.budget().cpu_instruction_cost();
    let mem_bytes = env.budget().memory_bytes_cost();

    println!("Harvest Resources:");
    println!("  CPU instructions: {}", cpu_insns);
    println!("  Memory bytes: {}", mem_bytes);

    // Target: < 1.5M CPU instructions
    assert!(cpu_insns < 1_500_000, "Harvest exceeds CPU target");
}

/// Benchmark batch operations
#[test]
fn bench_batch_mint_ships() {
    let env = Env::default();
    env.budget().reset_unlimited();

    let player = Address::generate(&env);
    let ship_types = soroban_sdk::vec![
        &env,
        soroban_sdk::symbol_short!("fighter"),
        soroban_sdk::symbol_short!("miner"),
        soroban_sdk::symbol_short!("scout"),
    ];

    env.budget().reset_unlimited();
    let _result = stellar_nebula_nomad::batch_mint_ships(
        env.clone(),
        player,
        ship_types,
        soroban_sdk::Bytes::new(&env),
    );

    let cpu_insns = env.budget().cpu_instruction_cost();
    let mem_bytes = env.budget().memory_bytes_cost();

    println!("Batch Mint Ships (3):");
    println!("  CPU instructions: {}", cpu_insns);
    println!("  Memory bytes: {}", mem_bytes);
    println!("  Per ship: {} CPU", cpu_insns / 3);

    // Target: < 3M CPU instructions for 3 ships
    assert!(cpu_insns < 3_000_000, "Batch mint exceeds CPU target");
}

/// Benchmark storage operations
#[test]
fn bench_storage_operations() {
    let env = Env::default();
    env.budget().reset_unlimited();

    let player = Address::generate(&env);

    // Initialize profile
    let profile_id = stellar_nebula_nomad::initialize_profile(env.clone(), player.clone()).unwrap();

    env.budget().reset_unlimited();

    // Update progress multiple times
    for i in 0..5 {
        stellar_nebula_nomad::update_progress(
            env.clone(),
            player.clone(),
            profile_id,
            i + 1,
            (i as i128 + 1) * 100,
        )
        .unwrap();
    }

    let cpu_insns = env.budget().cpu_instruction_cost();
    let mem_bytes = env.budget().memory_bytes_cost();

    println!("Storage Operations (5 updates):");
    println!("  CPU instructions: {}", cpu_insns);
    println!("  Memory bytes: {}", mem_bytes);
    println!("  Per update: {} CPU", cpu_insns / 5);

    // Target: < 500K CPU per update
    assert!(cpu_insns / 5 < 500_000, "Storage update exceeds CPU target");
}

fn measure_operation<F>(env: &Env, name: &str, cpu_limit: u64, memory_limit: u64, operation: F)
where
    F: FnOnce(),
{
    env.budget().reset_unlimited();
    operation();
    let cpu_insns = env.budget().cpu_instruction_cost();
    let mem_bytes = env.budget().memory_bytes_cost();
    println!("{name}: cpu={cpu_insns} memory={mem_bytes}");
    assert!(
        cpu_insns <= cpu_limit,
        "{name} exceeded CPU budget: {cpu_insns} > {cpu_limit}"
    );
    assert!(
        mem_bytes <= memory_limit,
        "{name} exceeded memory budget: {mem_bytes} > {memory_limit}"
    );
}

/// Comprehensive gas budget matrix for the most exercised public entrypoints.
#[test]
fn bench_comprehensive_public_entrypoints() {
    let env = Env::default();
    env.mock_all_auths();
    let player = Address::generate(&env);
    let seed = env
        .crypto()
        .sha256(&soroban_sdk::Bytes::from_slice(&env, &[9u8; 32]));
    let metadata = soroban_sdk::Bytes::new(&env);

    measure_operation(&env, "generate_nebula_layout", 1_000_000, 250_000, || {
        let _ = generate_nebula_layout(env.clone(), seed.clone(), player.clone());
    });

    measure_operation(&env, "scan_nebula", 2_000_000, 350_000, || {
        let _ = scan_nebula(env.clone(), seed.clone(), player.clone());
    });

    let ship = mint_ship(
        env.clone(),
        player.clone(),
        soroban_sdk::symbol_short!("fighter"),
        metadata.clone(),
    )
    .unwrap();

    let layout = generate_nebula_layout(env.clone(), seed.clone(), player.clone());
    measure_operation(&env, "harvest_resources", 1_500_000, 350_000, || {
        let _ = harvest_resources(&env, ship.id, &layout);
    });

    let ship_types = soroban_sdk::vec![
        &env,
        soroban_sdk::symbol_short!("fighter"),
        soroban_sdk::symbol_short!("miner"),
        soroban_sdk::symbol_short!("scout"),
    ];
    measure_operation(&env, "batch_mint_ships_3", 3_000_000, 500_000, || {
        let _ = stellar_nebula_nomad::batch_mint_ships(
            env.clone(),
            player.clone(),
            ship_types,
            metadata.clone(),
        );
    });

    measure_operation(&env, "initialize_profile", 750_000, 250_000, || {
        let _ = stellar_nebula_nomad::initialize_profile(env.clone(), player.clone()).unwrap();
    });

    measure_operation(&env, "calculate_difficulty", 500_000, 150_000, || {
        let _ = stellar_nebula_nomad::calculate_difficulty(env.clone(), 50).unwrap();
    });
}
