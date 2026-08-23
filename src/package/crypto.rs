//! Password-protected OOXML packages.
//!
//! An encrypted OOXML file is an OLE compound container whose payload is a
//! zip; [`crate::package::archive::probe_ole`] rejects it as
//! [`ConvertError::Encrypted`]. With a password in hand the container can be
//! decrypted back to that zip and converted like any plaintext package, so
//! all existing resource limits apply to the decrypted bytes unchanged.

use crate::error::ConvertError;

/// Decrypt a password-protected OOXML package into its plaintext zip bytes.
///
/// Every failure — malformed `EncryptionInfo`, unsupported scheme, wrong
/// password — maps to [`ConvertError::Encrypted`]: without a usable
/// plaintext there is nothing else useful to say, and that is the error
/// callers already handle.
pub fn decrypt_ooxml(bytes: Vec<u8>, password: &str) -> Result<Vec<u8>, ConvertError> {
    let plain = office_crypto::decrypt_from_bytes(bytes, password).map_err(|e| {
        log::debug!("OOXML decryption failed: {e}");
        ConvertError::Encrypted
    })?;
    // office-crypto does not check the EncryptionInfo password verifier, so a
    // wrong password still "succeeds" — into noise. The decrypted payload is
    // always the OOXML zip itself (the 8-byte size header is stripped), so its
    // signature is the cheapest reliable wrong-password test.
    if !plain.starts_with(b"PK") {
        log::debug!("OOXML decryption produced a non-zip payload (wrong password?)");
        return Err(ConvertError::Encrypted);
    }
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_container_maps_to_encrypted() {
        let err = decrypt_ooxml(vec![0xD0, 0xCF, 0x11, 0xE0], "nope").unwrap_err();
        assert!(matches!(err, ConvertError::Encrypted));
    }

    #[test]
    fn non_zip_payload_maps_to_encrypted() {
        // office-crypto does not verify the password, so a wrong one yields
        // noise with an Ok status; the zip-signature gate must catch it.
        let err = decrypt_ooxml(vec![0u8; 64], "wrong").unwrap_err();
        assert!(matches!(err, ConvertError::Encrypted));
    }
}
