use anyhow::{Context, Result};
use axum_server::tls_rustls::RustlsConfig;
use std::path::Path;

pub async fn load_or_generate_tls(
    data_dir: &Path,
    cert_path: Option<&str>,
    key_path: Option<&str>,
) -> Result<RustlsConfig> {
    if let (Some(c), Some(k)) = (cert_path, key_path)
        && !c.is_empty()
        && !k.is_empty()
    {
        return RustlsConfig::from_pem_file(c, k)
            .await
            .context("load configured TLS cert/key");
    }

    let tls_dir = data_dir.join("tls");
    std::fs::create_dir_all(&tls_dir).context("create tls dir")?;
    let auto_cert = tls_dir.join("cert.pem");
    let auto_key = tls_dir.join("key.pem");

    if !auto_cert.exists() || !auto_key.exists() {
        let subject_alt_names = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
        ];
        let cert = rcgen::generate_simple_self_signed(subject_alt_names)
            .context("generate self-signed TLS cert")?;
        std::fs::write(&auto_cert, cert.cert.pem()).context("write self-signed cert")?;
        std::fs::write(&auto_key, cert.key_pair.serialize_pem())
            .context("write self-signed key")?;
    }

    RustlsConfig::from_pem_file(&auto_cert, &auto_key)
        .await
        .context("load self-signed TLS config")
}
