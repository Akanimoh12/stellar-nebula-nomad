use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Symbol, Vec};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum ThemeError {
    InvalidTheme = 1,
    Unauthorized = 2,
    ShipNotFound = 3,
}

impl crate::error_standard::StandardContractError for ThemeError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::InvalidTheme => (ErrorKind::Validation, false),
            Self::Unauthorized => (ErrorKind::Authorization, false),
            Self::ShipNotFound => (ErrorKind::NotFound, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "theme_customizer",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemePreview {
    pub name: Symbol,
    pub colors: Vec<Symbol>, // Hex color codes
    pub particles: Symbol,
}

pub fn generate_theme_preview(env: Env, theme_id: Symbol) -> Result<ThemePreview, ThemeError> {
    match theme_id {
        s if s == symbol_short!("nebula1") => Ok(ThemePreview {
            name: symbol_short!("Cosmic"),
            colors: Vec::from_array(&env, [symbol_short!("FF00FF"), symbol_short!("00FFFF")]),
            particles: symbol_short!("Stardust"),
        }),
        s if s == symbol_short!("nebula2") => Ok(ThemePreview {
            name: symbol_short!("Void"),
            colors: Vec::from_array(&env, [symbol_short!("000000"), symbol_short!("444444")]),
            particles: symbol_short!("dark_mtr"),
        }),
        s if s == symbol_short!("nebula3") => Ok(ThemePreview {
            name: symbol_short!("Nova"),
            colors: Vec::from_array(&env, [symbol_short!("FFA500"), symbol_short!("FF4500")]),
            particles: symbol_short!("Flare"),
        }),
        s if s == symbol_short!("nebula4") => Ok(ThemePreview {
            name: symbol_short!("Quasar"),
            colors: Vec::from_array(&env, [symbol_short!("0000FF"), symbol_short!("FFFFFF")]),
            particles: symbol_short!("Beams"),
        }),
        s if s == symbol_short!("nebula5") => Ok(ThemePreview {
            name: symbol_short!("Supernova"),
            colors: Vec::from_array(&env, [symbol_short!("FF0000"), symbol_short!("FFFF00")]),
            particles: symbol_short!("Shockwave"),
        }),
        s if s == symbol_short!("nebula6") => Ok(ThemePreview {
            name: symbol_short!("Wormhole"),
            colors: Vec::from_array(&env, [symbol_short!("A020F0"), symbol_short!("000000")]),
            particles: symbol_short!("Vortex"),
        }),
        s if s == symbol_short!("nebula7") => Ok(ThemePreview {
            name: symbol_short!("BlackHole"),
            colors: Vec::from_array(&env, [symbol_short!("000000"), symbol_short!("111111")]),
            particles: symbol_short!("snglrty"),
        }),
        s if s == symbol_short!("nebula8") => Ok(ThemePreview {
            name: symbol_short!("Aurora"),
            colors: Vec::from_array(&env, [symbol_short!("00FF00"), symbol_short!("B026FF")]),
            particles: symbol_short!("Borealis"),
        }),
        s if s == symbol_short!("nebula9") => Ok(ThemePreview {
            name: symbol_short!("Eclipse"),
            colors: Vec::from_array(&env, [symbol_short!("CCCCCC"), symbol_short!("000000")]),
            particles: symbol_short!("Corral"),
        }),
        s if s == symbol_short!("nebula10") => Ok(ThemePreview {
            name: symbol_short!("Meteor"),
            colors: Vec::from_array(&env, [symbol_short!("FFD700"), symbol_short!("8B4513")]),
            particles: symbol_short!("Trails"),
        }),
        _ => Err(ThemeError::InvalidTheme), // Only showing a few for brevity, but should have 10 presets
    }
}

pub fn apply_theme(env: Env, owner: Address, ship_id: u64, theme_id: Symbol) -> Result<(), ThemeError> {
    owner.require_auth();

    // In a real scenario, we'd check if the owner owns the ship using ship_nft module.
    // For this prototype, we'll assume the caller must be authorized and ship exists.
    
    // Validate theme first
    let _ = generate_theme_preview(env.clone(), theme_id.clone())?;

    // Store ship-to-theme association
    env.storage().persistent().set(&(symbol_short!("theme"), ship_id), &theme_id);

    env.events().publish(
        (symbol_short!("theme"), symbol_short!("applied")),
        (ship_id, theme_id),
    );

    Ok(())
}

pub fn get_theme(env: Env, ship_id: u64) -> Option<Symbol> {
    env.storage().persistent().get(&(symbol_short!("theme"), ship_id))
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
    fn test_generate_theme_preview_rejects_unknown_theme() {
        let env = Env::default();
        let result = generate_theme_preview(env.clone(), symbol_short!("bogus"));
        assert_eq!(result, Err(ThemeError::InvalidTheme));
    }

    #[test]
    fn test_generate_theme_preview_first_and_last_boundary() {
        let env = Env::default();
        let first = generate_theme_preview(env.clone(), symbol_short!("nebula1")).unwrap();
        assert_eq!(first.name, symbol_short!("Cosmic"));

        let last = generate_theme_preview(env.clone(), symbol_short!("nebula10")).unwrap();
        assert_eq!(last.name, symbol_short!("Meteor"));
    }

    #[test]
    fn test_get_theme_missing_ship_returns_none() {
        let (env, contract_id) = make_env();
        env.as_contract(&contract_id, || {
            assert!(get_theme(env.clone(), 12345).is_none());
        });
    }

    #[test]
    fn test_apply_theme_rejects_invalid_theme_and_persists_nothing() {
        let (env, contract_id) = make_env();
        let owner = Address::generate(&env);
        env.as_contract(&contract_id, || {
            let result = apply_theme(env.clone(), owner, 1, symbol_short!("bogus"));
            assert_eq!(result, Err(ThemeError::InvalidTheme));
            assert!(get_theme(env.clone(), 1).is_none());
        });
    }

    #[test]
    fn test_apply_theme_then_get_theme_roundtrip() {
        let (env, contract_id) = make_env();
        let owner = Address::generate(&env);
        env.as_contract(&contract_id, || {
            apply_theme(env.clone(), owner, 7, symbol_short!("nebula3")).unwrap();
            assert_eq!(get_theme(env.clone(), 7), Some(symbol_short!("nebula3")));
        });
    }
}
