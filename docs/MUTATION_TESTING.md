# Mutation Testing with `cargo-mutants`

## Overview

Mutation testing evaluates test suite quality by intentionally mutating program source code (replacing binary operations, replacing return values with defaults, omitting statements) and verifying whether automated tests catch and fail on the mutation ("kill the mutant").

Surviving mutants indicate gaps in boundary checks, error conditions, or unasserted return states.

## Priority Modules

Mutation analysis focuses on core smart contract systems:
1. `src/access_control.rs`: Role hierarchy, emergency privileges, delegations, and time-lock mechanisms.
2. `src/resource_minter.rs`: Anti-whale protections, rate-limiting, and supply conservation.
3. `src/nebula_gen.rs`: Procedural determinism, coordinate bounds, and anomaly generations.
4. `src/ship_registry.rs`: Ship minting, stats calculation, and registry lookups.
5. `src/nomad_bonding.rs`: Curve pricing, bonding collateral, and slip protections.

## Running Locally

To run mutation testing on specific modules:

```bash
cargo install cargo-mutants
cargo mutants --file src/access_control.rs
```

## Target Mutation Score

- **Target**: > 80% mutants killed across prioritized modules.
- **Justified Surviving Mutants**:
  - Non-essential debug assertions and log format strings.
  - Redundant saturating math operations on values strictly bounded by earlier checks.
