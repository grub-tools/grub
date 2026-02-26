use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Returns the default TLS directory within the grub data directory.
pub fn tls_dir() -> Result<PathBuf> {
    let proj_dirs = directories::ProjectDirs::from("", "", "grub")
        .context("Could not determine home directory")?;
    let dir = proj_dirs.data_dir().join("tls");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create TLS directory: {}", dir.display()))?;
    Ok(dir)
}

/// Default certificate path.
pub fn default_cert_path() -> Result<PathBuf> {
    Ok(tls_dir()?.join("cert.pem"))
}

/// Default key path.
pub fn default_key_path() -> Result<PathBuf> {
    Ok(tls_dir()?.join("key.pem"))
}

/// Generate a self-signed certificate and private key, writing them to the given paths.
/// Returns the SHA-256 fingerprint of the certificate.
pub fn generate_self_signed_cert(cert_path: &Path, key_path: &Path) -> Result<String> {
    let mut params = rcgen::CertificateParams::new(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "0.0.0.0".to_string(),
    ])
    .context("failed to create certificate params")?;

    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "grub self-signed");
    params
        .distinguished_name
        .push(rcgen::DnType::OrganizationName, "grub");

    // Add IP SANs for local network access
    params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress(std::net::IpAddr::V4(
            std::net::Ipv4Addr::LOCALHOST,
        )));

    let key_pair = rcgen::KeyPair::generate().context("failed to generate key pair")?;
    let cert = params
        .self_signed(&key_pair)
        .context("failed to generate self-signed certificate")?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    // Compute fingerprint from DER bytes (more reliable than re-parsing PEM)
    let fingerprint = sha256_fingerprint(cert.der());

    std::fs::write(cert_path, &cert_pem)
        .with_context(|| format!("Failed to write certificate to {}", cert_path.display()))?;
    std::fs::write(key_path, &key_pem)
        .with_context(|| format!("Failed to write private key to {}", key_path.display()))?;

    Ok(fingerprint)
}

/// Compute the SHA-256 fingerprint of DER-encoded certificate bytes.
fn sha256_fingerprint(der: &[u8]) -> String {
    let hash = Sha256::digest(der);
    hash.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Compute the SHA-256 fingerprint from a PEM-encoded certificate file.
pub fn fingerprint_from_pem_file(cert_path: &Path) -> Result<String> {
    let pem_data = std::fs::read(cert_path)
        .with_context(|| format!("Failed to read certificate from {}", cert_path.display()))?;

    let mut reader = std::io::BufReader::new(pem_data.as_slice());
    let certs: Vec<_> =
        rustls_pemfile::certs(&mut reader).collect::<std::result::Result<_, _>>()?;

    let cert = certs.first().context("No certificate found in PEM file")?;

    Ok(sha256_fingerprint(cert.as_ref()))
}

/// Ensure a certificate and key exist (generate if missing).
/// Returns the SHA-256 fingerprint.
pub fn ensure_cert(cert_path: &Path, key_path: &Path) -> Result<String> {
    if cert_path.exists() && key_path.exists() {
        fingerprint_from_pem_file(cert_path)
    } else {
        eprintln!(
            "Generating self-signed TLS certificate at {}",
            cert_path.display()
        );
        generate_self_signed_cert(cert_path, key_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_generate_self_signed_cert() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cert_path = tmp.path().join("cert.pem");
        let key_path = tmp.path().join("key.pem");

        let fingerprint = generate_self_signed_cert(&cert_path, &key_path).unwrap();

        assert!(cert_path.exists());
        assert!(key_path.exists());

        let cert_contents = fs::read_to_string(&cert_path).unwrap();
        assert!(cert_contents.contains("BEGIN CERTIFICATE"));

        let key_contents = fs::read_to_string(&key_path).unwrap();
        assert!(key_contents.contains("BEGIN PRIVATE KEY"));

        // Fingerprint should be colon-separated hex (SHA-256 = 32 bytes = 95 chars with colons)
        assert_eq!(fingerprint.len(), 95);
        assert!(fingerprint.contains(':'));
    }

    #[test]
    fn test_ensure_cert_generates_when_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cert_path = tmp.path().join("cert.pem");
        let key_path = tmp.path().join("key.pem");

        let fp = ensure_cert(&cert_path, &key_path).unwrap();
        assert!(!fp.is_empty());
        assert!(cert_path.exists());
    }

    #[test]
    fn test_ensure_cert_reuses_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cert_path = tmp.path().join("cert.pem");
        let key_path = tmp.path().join("key.pem");

        let fp1 = ensure_cert(&cert_path, &key_path).unwrap();
        let fp2 = ensure_cert(&cert_path, &key_path).unwrap();
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_fingerprint_format() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cert_path = tmp.path().join("cert.pem");
        let key_path = tmp.path().join("key.pem");

        let fingerprint = generate_self_signed_cert(&cert_path, &key_path).unwrap();

        // Should be uppercase hex pairs separated by colons
        let parts: Vec<&str> = fingerprint.split(':').collect();
        assert_eq!(parts.len(), 32); // SHA-256 = 32 bytes
        for part in parts {
            assert_eq!(part.len(), 2);
            assert!(part.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn test_fingerprint_from_pem_matches_generate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cert_path = tmp.path().join("cert.pem");
        let key_path = tmp.path().join("key.pem");

        let fp_generate = generate_self_signed_cert(&cert_path, &key_path).unwrap();
        let fp_read = fingerprint_from_pem_file(&cert_path).unwrap();

        assert_eq!(fp_generate, fp_read);
    }
}
