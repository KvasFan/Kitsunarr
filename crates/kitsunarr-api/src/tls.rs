//! Sonarr's certificate validation (`X509CertificateValidationService`): HTTPS certificates are
//! checked by the platform verifier, and a failure is accepted only when the setting says so —
//! always, or for local addresses (loopback, private and link-local IPs, `localhost` and
//! single-label or `.local`/`.lan`/`.internal`/`.home.arpa` names). Saving the setting applies
//! at once: every client built from [`CertificatePolicy::rustls_config`] consults it.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use kitsunarr_domain::api::settings::CertificateValidation;
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};

#[derive(Debug, Clone, Default)]
pub struct CertificatePolicy(Arc<AtomicU8>);

impl CertificatePolicy {
    pub fn set(&self, mode: CertificateValidation) {
        let v = match mode {
            CertificateValidation::Enabled => 0,
            CertificateValidation::DisabledForLocalAddresses => 1,
            CertificateValidation::Disabled => 2,
        };
        self.0.store(v, Ordering::Relaxed);
    }

    pub fn mode(&self) -> CertificateValidation {
        match self.0.load(Ordering::Relaxed) {
            1 => CertificateValidation::DisabledForLocalAddresses,
            2 => CertificateValidation::Disabled,
            _ => CertificateValidation::Enabled,
        }
    }

    /// A rustls configuration whose verifier follows this policy (HTTP/2 and HTTP/1.1 ALPN).
    pub fn rustls_config(&self) -> Option<rustls::ClientConfig> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let inner = rustls_platform_verifier::Verifier::new(provider.clone()).ok()?;
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .ok()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PolicyVerifier {
                inner: Arc::new(inner),
                policy: self.clone(),
            }))
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Some(config)
    }
}

/// `IsLocalAddress` for the names TLS sees.
pub fn is_local(name: &ServerName<'_>) -> bool {
    match name {
        ServerName::IpAddress(ip) => local_ip(IpAddr::from(*ip)),
        ServerName::DnsName(dns) => {
            let host = dns.as_ref().trim_end_matches('.').to_ascii_lowercase();
            host == "localhost"
                || !host.contains('.')
                || [".local", ".lan", ".internal", ".home.arpa", ".localhost"]
                    .iter()
                    .any(|s| host.ends_with(s))
        }
        _ => false,
    }
}

fn local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| local_ip(IpAddr::V4(v4)))
        }
    }
}

#[derive(Debug)]
struct PolicyVerifier {
    inner: Arc<dyn ServerCertVerifier>,
    policy: CertificatePolicy,
}

impl ServerCertVerifier for PolicyVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.inner.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        ) {
            Ok(v) => Ok(v),
            Err(e) => match self.policy.mode() {
                CertificateValidation::Disabled => Ok(ServerCertVerified::assertion()),
                CertificateValidation::DisabledForLocalAddresses if is_local(server_name) => {
                    Ok(ServerCertVerified::assertion())
                }
                _ => Err(e),
            },
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_addresses() {
        let name = |s: &str| ServerName::try_from(s.to_string()).unwrap();
        for local in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.20",
            "172.16.3.4",
            "169.254.1.1",
            "::1",
            "fd00::1",
            "fe80::1",
            "localhost",
            "nas",
            "diskstation.local",
            "router.lan",
        ] {
            assert!(is_local(&name(local)), "{local}");
        }
        for remote in [
            "8.8.8.8",
            "2001:4860::1",
            "example.com",
            "indexer.example.org",
        ] {
            assert!(!is_local(&name(remote)), "{remote}");
        }
    }

    #[test]
    fn the_policy_builds_a_client_configuration() {
        let p = CertificatePolicy::default();
        assert_eq!(p.mode(), CertificateValidation::Enabled);
        p.set(CertificateValidation::DisabledForLocalAddresses);
        assert_eq!(p.mode(), CertificateValidation::DisabledForLocalAddresses);
        let config = p.rustls_config().expect("platform verifier");
        assert_eq!(config.alpn_protocols.len(), 2);
        reqwest::Client::builder()
            .tls_backend_preconfigured(config)
            .build()
            .expect("reqwest takes it");
    }
}
