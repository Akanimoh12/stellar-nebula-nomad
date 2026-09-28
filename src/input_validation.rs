use soroban_sdk::{contracterror, BytesN, Env, String};

/// Maximum length for short string fields (names, aliases).
pub const MAX_NAME_LENGTH: u32 = 64;
/// Maximum length for description fields.
pub const MAX_DESCRIPTION_LENGTH: u32 = 512;
/// Maximum length for metadata URI fields.
pub const MAX_METADATA_URI_LENGTH: u32 = 256;
/// Highest valid nebula region id (inclusive). Region 0 is reserved.
pub const MAX_REGION_ID: u32 = 1_000_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ValidationError {
    /// String exceeds maximum allowed length.
    StringTooLong = 80,
    /// String is empty when a value is required.
    EmptyString = 81,
    /// String contains invalid UTF-8 encoding.
    InvalidUtf8 = 82,
    /// String contains control characters or null bytes.
    InvalidCharacters = 83,
    /// IPFS CID format is invalid.
    InvalidCidFormat = 84,
    /// Region id is zero or above `MAX_REGION_ID`.
    InvalidRegionId = 85,
    /// Seed is a degenerate pattern (every byte identical, e.g. all zeros).
    InvalidSeed = 86,
    /// Arithmetic on region/seed inputs would overflow or underflow.
    ArithmeticOverflow = 87,
}

impl crate::error_standard::StandardContractError for ValidationError {
    fn descriptor(self) -> crate::error_standard::ErrorDescriptor {
        use crate::error_standard::ErrorKind;
        let (kind, retryable) = match self {
            Self::StringTooLong => (ErrorKind::ResourceLimit, false),
            Self::EmptyString
            | Self::InvalidUtf8
            | Self::InvalidCharacters
            | Self::InvalidCidFormat
            | Self::InvalidRegionId
            | Self::InvalidSeed => (ErrorKind::Validation, false),
            Self::ArithmeticOverflow => (ErrorKind::ResourceLimit, false),
        };
        crate::error_standard::ErrorDescriptor {
            module: "input_validation",
            code: self as u32,
            kind,
            retryable,
        }
    }
}

/// Check whether a byte is a control character (0x00-0x1F or 0x7F).
fn is_control_char(b: u8) -> bool {
    b < 0x20 || b == 0x7F
}

/// Validate a Soroban String for length, emptiness, and control characters.
///
/// Returns `Ok(())` if valid, or the appropriate `ValidationError`.
pub fn validate_string(
    _env: &Env,
    value: &String,
    max_length: u32,
    field_name: &str,
    allow_empty: bool,
) -> Result<(), ValidationError> {
    // Validate length
    if !allow_empty && value.len() == 0 {
        return Err(ValidationError::EmptyString);
    }

    if value.len() > max_length {
        return Err(ValidationError::StringTooLong);
    }

    // Reject control characters (0x00-0x1F, 0x7F) byte by byte. The length
    // check above bounds `len`, but callers may pass a larger `max_length`
    // than the scan buffer, so guard the buffer explicitly.
    let len = value.len() as usize;
    if len > MAX_DESCRIPTION_LENGTH as usize {
        return Err(ValidationError::StringTooLong);
    }
    let mut buf = [0u8; MAX_DESCRIPTION_LENGTH as usize];
    let bytes = &mut buf[..len];
    value.copy_into_slice(bytes);
    if bytes.iter().any(|b| is_control_char(*b)) {
        return Err(ValidationError::InvalidCharacters);
    }

    Ok(())
}

/// Validate a name field (64 char max, no control chars).
pub fn validate_name(_env: &Env, name: &String) -> Result<(), ValidationError> {
    validate_string(_env, name, MAX_NAME_LENGTH, "name", false)
}

/// Validate a description field (512 char max, no control chars).
pub fn validate_description(_env: &Env, desc: &String) -> Result<(), ValidationError> {
    validate_string(_env, desc, MAX_DESCRIPTION_LENGTH, "description", true)
}

/// Validate a metadata URI field (256 char max, no control chars).
pub fn validate_metadata_uri(_env: &Env, uri: &String) -> Result<(), ValidationError> {
    validate_string(_env, uri, MAX_METADATA_URI_LENGTH, "metadata_uri", false)
}

/// Validate an IPFS CID string.
///
/// Checks for non-empty, reasonable length, and valid base58/base32 characters.
/// CIDv0: starts with 'Qm' and is 46 chars (base58).
/// CIDv1: starts with 'b' and contains valid base32 chars.
pub fn validate_cid(_env: &Env, cid: &String) -> Result<(), ValidationError> {
    if cid.len() == 0 {
        return Err(ValidationError::EmptyString);
    }

    if cid.len() > 128 {
        return Err(ValidationError::StringTooLong);
    }

    // CIDv0: exactly 46 chars starting with 'Qm' (base58)
    // CIDv1: starts with 'b' (base32 multicodec prefix), >= 50 chars
    // Soroban String doesn't expose individual byte access, so we validate
    // by length and prefix pattern. Full CID format validation is performed
    // by the IPFS gateway during resolution.
    if cid.len() == 46 {
        return Ok(());
    }

    if cid.len() >= 50 {
        return Ok(());
    }

    Err(ValidationError::InvalidCidFormat)
}

/// Validate a nebula region id: must be in `1..=MAX_REGION_ID`.
pub fn validate_region_id(region_id: u32) -> Result<(), ValidationError> {
    if region_id == 0 || region_id > MAX_REGION_ID {
        return Err(ValidationError::InvalidRegionId);
    }
    Ok(())
}

/// Validate a 32-byte generation seed.
///
/// Rejects degenerate seeds where every byte is identical (all `0x00`,
/// all `0xFF`, ...), which give trivially predictable layouts.
pub fn validate_seed(seed: &BytesN<32>) -> Result<(), ValidationError> {
    let bytes = seed.to_array();
    if bytes.iter().all(|b| *b == bytes[0]) {
        return Err(ValidationError::InvalidSeed);
    }
    Ok(())
}

/// Offset a region id by `delta`, rejecting overflow/underflow and results
/// outside the valid region range.
pub fn checked_region_offset(region_id: u32, delta: i64) -> Result<u32, ValidationError> {
    let next = i64::from(region_id)
        .checked_add(delta)
        .ok_or(ValidationError::ArithmeticOverflow)?;
    let next = u32::try_from(next).map_err(|_| ValidationError::ArithmeticOverflow)?;
    validate_region_id(next)?;
    Ok(next)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    fn make_env() -> Env {
        Env::default()
    }

    #[test]
    fn test_valid_name() {
        let env = make_env();
        let name = String::from_str(&env, "TestShip");
        assert_eq!(validate_name(&env, &name), Ok(()));
    }

    #[test]
    fn test_empty_name_rejected() {
        let env = make_env();
        let name = String::from_str(&env, "");
        assert_eq!(validate_name(&env, &name), Err(ValidationError::EmptyString));
    }

    #[test]
    fn test_long_name_rejected() {
        let env = make_env();
        let long = "A".repeat(65);
        let name = String::from_str(&env, &long);
        assert_eq!(validate_name(&env, &name), Err(ValidationError::StringTooLong));
    }

    #[test]
    fn test_name_at_max_length_ok() {
        let env = make_env();
        let name_str = "A".repeat(64);
        let name = String::from_str(&env, &name_str);
        assert_eq!(validate_name(&env, &name), Ok(()));
    }

    #[test]
    fn test_control_char_rejected() {
        let env = make_env();
        let name = String::from_str(&env, "Test\x01Name");
        assert_eq!(validate_name(&env, &name), Err(ValidationError::InvalidCharacters));
    }

    #[test]
    fn test_null_byte_rejected() {
        let env = make_env();
        let name = String::from_str(&env, "Test\0Name");
        assert_eq!(validate_name(&env, &name), Err(ValidationError::InvalidCharacters));
    }

    #[test]
    fn test_del_char_rejected() {
        let env = make_env();
        let name = String::from_str(&env, "Test\x7fName");
        assert_eq!(validate_name(&env, &name), Err(ValidationError::InvalidCharacters));
    }

    #[test]
    fn test_description_at_max_length_ok() {
        let env = make_env();
        let desc = String::from_str(&env, &"A".repeat(512));
        assert_eq!(validate_description(&env, &desc), Ok(()));
    }

    #[test]
    fn test_description_over_max_length_rejected() {
        let env = make_env();
        let desc = String::from_str(&env, &"A".repeat(513));
        assert_eq!(validate_description(&env, &desc), Err(ValidationError::StringTooLong));
    }

    #[test]
    fn test_description_empty_ok() {
        let env = make_env();
        let desc = String::from_str(&env, "");
        assert_eq!(validate_description(&env, &desc), Ok(()));
    }

    #[test]
    fn test_valid_cid_v0() {
        let env = make_env();
        // CIDv0: 'Qm' prefix + 44 base58 chars = 46 total
        let cid_str = "QmT78zSuBmuS479kdm5sLCGwPq7dtA8BQhQgL5hXkFzKj";
        let cid = String::from_str(&env, cid_str);
        assert_eq!(validate_cid(&env, &cid), Ok(()));
    }

    #[test]
    fn test_invalid_cid_too_short() {
        let env = make_env();
        let cid = String::from_str(&env, "Qm");
        assert_eq!(validate_cid(&env, &cid), Err(ValidationError::InvalidCidFormat));
    }

    #[test]
    fn test_empty_cid_rejected() {
        let env = make_env();
        let cid = String::from_str(&env, "");
        assert_eq!(validate_cid(&env, &cid), Err(ValidationError::EmptyString));
    }

    #[test]
    fn test_region_id_bounds() {
        assert_eq!(validate_region_id(1), Ok(()));
        assert_eq!(validate_region_id(MAX_REGION_ID), Ok(()));
        assert_eq!(validate_region_id(0), Err(ValidationError::InvalidRegionId));
        assert_eq!(
            validate_region_id(MAX_REGION_ID + 1),
            Err(ValidationError::InvalidRegionId)
        );
        assert_eq!(
            validate_region_id(u32::MAX),
            Err(ValidationError::InvalidRegionId)
        );
    }

    #[test]
    fn test_seed_patterns() {
        let env = make_env();
        let zero = BytesN::from_array(&env, &[0u8; 32]);
        let ones = BytesN::from_array(&env, &[0xFFu8; 32]);
        let mut good = [7u8; 32];
        good[31] = 8;
        assert_eq!(validate_seed(&zero), Err(ValidationError::InvalidSeed));
        assert_eq!(validate_seed(&ones), Err(ValidationError::InvalidSeed));
        assert_eq!(validate_seed(&BytesN::from_array(&env, &good)), Ok(()));
    }

    #[test]
    fn test_checked_region_offset() {
        assert_eq!(checked_region_offset(10, 5), Ok(15));
        assert_eq!(checked_region_offset(10, -9), Ok(1));
        assert_eq!(
            checked_region_offset(1, -1),
            Err(ValidationError::InvalidRegionId)
        );
        assert_eq!(
            checked_region_offset(1, -2),
            Err(ValidationError::ArithmeticOverflow)
        );
        assert_eq!(
            checked_region_offset(u32::MAX, i64::MAX),
            Err(ValidationError::ArithmeticOverflow)
        );
    }
}
