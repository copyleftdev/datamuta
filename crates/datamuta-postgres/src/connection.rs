//! Explicit lab or verified transport. Configuration contains references, never passwords.
use super::*;
use std::{io::Read, path::PathBuf, sync::Arc};
use tokio_postgres::config::{ChannelBinding, SslMode};

// Preserve actionable categories without relaying server-controlled messages.
fn connection_error(e: tokio_postgres::Error) -> Error {
    if let Some(code) = e.code() {
        return invalid(&format!(
            "verified PostgreSQL connection refused (SQLSTATE {})",
            code.code()
        ));
    }
    let mut source = std::error::Error::source(&e);
    for _ in 0..8 {
        let Some(cause) = source else {
            break;
        };
        for known in [
            "server does not support TLS",
            "server did not use channel binding",
        ] {
            if cause.to_string() == known {
                return invalid(&format!("verified PostgreSQL connection refused ({known})"));
            }
        }
        if let Some(tls) = cause.downcast_ref::<rustls::Error>() {
            let category = match tls {
                rustls::Error::InvalidCertificate(_) => "certificate verification",
                _ => "TLS protocol",
            };
            return invalid(&format!(
                "verified PostgreSQL connection refused ({category})"
            ));
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return invalid(&format!(
                "verified PostgreSQL connection refused (I/O {:?})",
                io.kind()
            ));
        }
        source = cause.source();
    }
    invalid("verified PostgreSQL connection refused (TLS or authentication policy)")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verified {
    pub host: String,
    pub database: String,
    pub user: String,
    /// One DER-encoded trust anchor, at most 64 KiB. No implicit system roots.
    pub ca_der: PathBuf,
    pub ca_sha256: kernel::Fingerprint,
    /// Read at connection time. Rotation takes effect on the next connection.
    pub password_env: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub port: u16,
    pub namespace: String,
    pub lab_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified: Option<Verified>,
}
fn identifier(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
impl Endpoint {
    pub fn validate(&self) -> Result<()> {
        require(
            self.port > 0
                && self.namespace.starts_with("dm_")
                && self.namespace.len() <= 48
                && self
                    .namespace
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "invalid endpoint namespace or port",
        )?;
        match (&self.verified, self.lab_only) {
            (None, true) => Ok(()),
            (Some(v), false) => {
                require(
                    !v.host.is_empty()
                        && v.host.len() <= 253
                        && !v.host.contains(['/', '\\', '%'])
                        && rustls::pki_types::ServerName::try_from(v.host.as_str()).is_ok(),
                    "invalid TLS server identity",
                )?;
                require(
                    identifier(&v.database) && identifier(&v.user),
                    "invalid database or role",
                )?;
                require(v.ca_der.is_absolute(), "CA path must be absolute")?;
                require(
                    v.password_env.starts_with("DATAMUTA_")
                        && v.password_env.len() <= 128
                        && v.password_env
                            .bytes()
                            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                    "invalid credential environment reference",
                )
            }
            _ => Err(invalid(
                "choose exactly one of lab-only or verified transport",
            )),
        }
    }
    /// Stable identity includes trust and principal, excluding secret values and file location.
    pub fn receipt_domain(&self) -> Result<String> {
        self.validate()?;
        if let Some(v) = &self.verified {
            let identity = kernel::canonical_hash(&(
                "postgres/verified-v1",
                &v.host,
                self.port,
                &v.database,
                &v.user,
                &self.namespace,
                &v.ca_sha256,
            ))?;
            Ok(format!("postgres/verified/{}", identity.as_str()))
        } else {
            Ok(format!("127.0.0.1:{}/{}", self.port, self.namespace))
        }
    }
}

pub struct Session {
    client: Client,
    task: tokio::task::JoinHandle<()>,
}
impl Deref for Session {
    type Target = Client;
    fn deref(&self) -> &Client {
        &self.client
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn tls_config(v: &Verified) -> Result<rustls::ClientConfig> {
    let file = std::fs::File::open(&v.ca_der).map_err(|_| invalid("CA file unavailable"))?;
    require(
        file.metadata()
            .map_err(|_| invalid("CA metadata unavailable"))?
            .is_file(),
        "CA must be a regular file",
    )?;
    let mut bytes = Vec::new();
    file.take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid("CA read failed"))?;
    require(!bytes.is_empty() && bytes.len() <= 65_536, "CA size bound")?;
    require(
        kernel::Fingerprint::of(&bytes) == v.ca_sha256,
        "CA fingerprint mismatch",
    )?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(bytes))
        .map_err(|_| invalid("invalid CA certificate"))?;
    Ok(rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| invalid("TLS protocol policy"))?
    .with_root_certificates(roots)
    .with_no_client_auth())
}

pub async fn connect(endpoint: &Endpoint) -> Result<Session> {
    endpoint.validate()?;
    // Bounds DNS, TCP, TLS and authentication together, not just TCP establishment.
    tokio::time::timeout(Duration::from_secs(10), connect_inner(endpoint))
        .await
        .map_err(|_| invalid("PostgreSQL connection deadline"))?
}
async fn connect_inner(endpoint: &Endpoint) -> Result<Session> {
    let mut config = tokio_postgres::Config::new();
    config
        .port(endpoint.port)
        .connect_timeout(Duration::from_secs(5))
        .application_name("datamuta")
        .options("-csearch_path=pg_catalog -cstatement_timeout=10000 -clock_timeout=8000");
    let session = if let Some(v) = &endpoint.verified {
        let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config(v)?);
        let password =
            std::env::var(&v.password_env).map_err(|_| invalid("credential unavailable"))?;
        require(
            !password.is_empty() && password.len() <= 4096 && !password.contains('\0'),
            "credential size or encoding",
        )?;
        config
            .host(&v.host)
            .user(&v.user)
            .dbname(&v.database)
            .password(password)
            .ssl_mode(SslMode::Require)
            .channel_binding(ChannelBinding::Require);
        let (client, connection) = config.connect(tls).await.map_err(connection_error)?;
        Session {
            client,
            task: tokio::spawn(async move {
                let _ = connection.await;
            }),
        }
    } else {
        config
            .host("127.0.0.1")
            .user("datamuta")
            .dbname("datamuta")
            .ssl_mode(SslMode::Disable);
        let (client, connection) = config.connect(tokio_postgres::NoTls).await.map_err(err)?;
        Session {
            client,
            task: tokio::spawn(async move {
                let _ = connection.await;
            }),
        }
    };
    session
        .batch_execute(&format!(
            "SET search_path TO pg_catalog, {}, pg_temp; SET synchronous_commit=on",
            q(&endpoint.namespace)
        ))
        .await
        .map_err(|_| invalid("PostgreSQL session setup refused"))?;
    if endpoint.verified.is_some() {
        verified_session(&session).await?;
    }
    Ok(session)
}

/// Narrow admission envelope: PostgreSQL 17, primary, durable settings, TLS,
/// and a non-superuser/non-BYPASSRLS role. This cannot certify storage hardware.
async fn verified_session(c: &Client) -> Result<()> {
    let r = c.query_one("SELECT current_setting('server_version_num')::int4 BETWEEN 170000 AND 179999
        AND NOT pg_is_in_recovery() AND current_setting('transaction_read_only')='off'
        AND current_setting('fsync')='on' AND current_setting('full_page_writes')='on'
        AND current_setting('synchronous_commit')='on'
        AND EXISTS(SELECT 1 FROM pg_stat_ssl WHERE pid=pg_backend_pid() AND ssl AND version IN ('TLSv1.2','TLSv1.3'))
        AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=current_user AND NOT rolsuper AND NOT rolbypassrls)", &[])
        .await.map_err(|_| invalid("PostgreSQL capability probe failed"))?;
    require(
        r.try_get::<_, bool>(0).map_err(err)?,
        "PostgreSQL session capabilities refused",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_modes_and_identity() -> Result<()> {
        let mut e: Endpoint =
            serde_json::from_str(r#"{"port":5432,"namespace":"dm_test","lab_only":true}"#)
                .map_err(err)?;
        e.validate()?;
        e.lab_only = false;
        assert!(e.validate().is_err());
        e.verified = Some(Verified {
            host: "localhost".into(),
            database: "data".into(),
            user: "worker".into(),
            ca_der: "/tmp/ca.der".into(),
            ca_sha256: kernel::Fingerprint::of(b"ca"),
            password_env: "DATAMUTA_PASSWORD".into(),
        });
        let id = e.receipt_domain()?;
        let v = e.verified.as_mut().ok_or_else(|| invalid("fixture"))?;
        v.ca_der = "/tmp/rotated-location.der".into();
        v.password_env = "DATAMUTA_NEW_PASSWORD".into();
        assert_eq!(id, e.receipt_domain()?);
        e.verified.as_mut().ok_or_else(|| invalid("fixture"))?.user = "other".into();
        assert_ne!(id, e.receipt_domain()?);
        e.lab_only = true;
        assert!(e.validate().is_err());
        assert!(
            serde_json::from_str::<Endpoint>(
                r#"{"port":5432,"namespace":"dm_test","lab_only":true,"password":"secret"}"#
            )
            .is_err()
        );
        Ok(())
    }
}
