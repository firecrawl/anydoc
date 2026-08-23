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
    // The same archive budget that governs plaintext packages must bound the
    // decrypted one too, or a small encrypted file could inflate past every
    // limit before any part is read.
    let total: u64 = plain.len() as u64;
    if total > crate::package::limits::MAX_TOTAL_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_total_bytes",
            detail: format!(
                "decrypted OOXML package is {total} bytes, over the {} byte budget",
                crate::package::limits::MAX_TOTAL_BYTES
            ),
        });
    }
    // office-crypto does not check the EncryptionInfo password verifier, so a
    // wrong password still "succeeds" — into noise. The decrypted payload is
    // always an OOXML zip (the 8-byte size header is stripped), and the
    // signature alone is not proof, so validate it with the same archive
    // reader every package goes through next.
    if !plain.starts_with(b"PK") || zip_check_broken(&plain) {
        log::debug!("OOXML decryption produced a non-zip payload (wrong password?)");
        return Err(ConvertError::Encrypted);
    }
    Ok(plain)
}

/// Cheap structural probe: can the shared zip reader actually open this?
fn zip_check_broken(plain: &[u8]) -> bool {
    zip::ZipArchive::new(std::io::Cursor::new(plain)).map(|z| z.len()).is_err()
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
