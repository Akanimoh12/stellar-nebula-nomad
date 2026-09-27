//! Shared error metadata and hierarchy for contract modules.
//!
//! Soroban contract errors remain small `#[repr(u32)]` enums. This module adds
//! a uniform, off-chain-friendly description without changing their ABI.

/// Broad error classes that callers can use for consistent handling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorKind {
    Validation,
    Authorization,
    NotFound,
    Conflict,
    ResourceLimit,
    Internal,
}

/// Stable metadata exposed by every standardized module error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorDescriptor {
    /// Module namespace, used with `code` to form a globally unique identity.
    pub module: &'static str,
    /// Stable numeric code encoded in the contract ABI.
    pub code: u32,
    /// Semantic class for clients that should not match individual variants.
    pub kind: ErrorKind,
    /// Whether retrying later may succeed without changing the request.
    pub retryable: bool,
}

/// Common interface implemented by contract error enums.
pub trait StandardContractError {
    fn descriptor(self) -> ErrorDescriptor;
}

#[cfg(test)]
mod tests {
    use super::{ErrorKind, StandardContractError};
    use crate::{
        access_control::AccessControlError, analytics::AnalyticsError,
        batch_processor::BatchError,
    };

    #[test]
    fn descriptors_namespace_otherwise_overlapping_codes() {
        let access = AccessControlError::AdminRequired.descriptor();
        let analytics = AnalyticsError::InvalidTopN.descriptor();

        assert_eq!(access.code, analytics.code);
        assert_ne!(access.module, analytics.module);
        assert_eq!(access.kind, ErrorKind::Authorization);
        assert_eq!(analytics.kind, ErrorKind::Validation);
    }

    #[test]
    fn resource_limits_are_consistently_classified() {
        let descriptor = BatchError::GasLimitExceeded.descriptor();

        assert_eq!(descriptor.module, "batch");
        assert_eq!(descriptor.kind, ErrorKind::ResourceLimit);
        assert!(descriptor.retryable);
    }

    #[test]
    fn rate_limit_errors_are_retryable_resource_limits() {
        use crate::{
            nebula_gen::NebulaError, rate_limiter::RateLimitError, ship_upgrade::ShipUpgradeError,
        };

        for descriptor in [
            NebulaError::RateLimitExceeded.descriptor(),
            RateLimitError::RateLimitExceeded.descriptor(),
            ShipUpgradeError::RateLimitExceeded.descriptor(),
        ] {
            assert_eq!(descriptor.kind, ErrorKind::ResourceLimit);
            assert!(descriptor.retryable);
        }
    }

    #[test]
    fn descriptors_preserve_abi_codes_and_module_namespace() {
        use crate::{migration_framework::MigrationError, ship_upgrade::ShipUpgradeError};

        let invalid_ship = ShipUpgradeError::InvalidShipId.descriptor();
        assert_eq!(invalid_ship.module, "ship_upgrade");
        assert_eq!(invalid_ship.code, 206);
        assert_eq!(invalid_ship.kind, ErrorKind::Validation);

        let unauthorized = MigrationError::Unauthorized.descriptor();
        assert_eq!(unauthorized.module, "migration_framework");
        assert_eq!(unauthorized.kind, ErrorKind::Authorization);
        assert!(!unauthorized.retryable);
    }
}
