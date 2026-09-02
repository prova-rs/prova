//! One certificate policy, shared by every network client
//! (docs/design/architecture.md#tls-everywhere).
//!
//! `http`/`graphql` (reqwest), `websocket` (tokio-tungstenite) and `grpc` (tonic) reach TLS through
//! three unrelated crates with three unrelated configuration types. This module is the single place
//! that turns a Lua options table into a decision, so the two options an author writes —
//! `insecure` and `ca_cert` — are spelled the same, mean the same, and are refused the same way on
//! all three. Without it each transport would grow its own dialect and they would drift: the
//! interesting drift is not a spelling difference but a *policy* difference, where one transport
//! keeps verifying after the author believed they had turned verification off (or the reverse).
//!
//! **Option parsing is compiled unconditionally; only the certificate machinery is gated.** A
//! build without the `tls` feature must still recognise `insecure`, `ca_cert` and a `wss://` URL
//! well enough to say *this build has no TLS* — the alternative is what v1 did, where
//! `http.get("https://…")` surfaced reqwest's "invalid URL, scheme is not http" and named neither
//! TLS nor the way out.

use std::path::PathBuf;

use mlua::Table;

#[cfg(feature = "tls")]
use std::sync::Arc;

/// The TLS keys every client constructor and per-call options table accepts.
///
/// Exported so each caller can splice them into its own closed option set
/// (docs/design/agent-ergonomics.md#module-opts-silently-ignored) — the alternative, a separate
/// nested `tls = { … }` table, would make the common case (`insecure = true`) two levels deep for
/// no gain in clarity.
pub(crate) const TLS_OPTS: &[&str] = &["ca_cert", "insecure"];

/// What the author asked for, resolved from Lua and independent of transport.
///
/// `Default` is verified TLS against both root sets — the state that needs no options at all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tls {
    /// Accept any certificate: no chain, no hostname, no expiry check. Supported, not smuggled —
    /// a service prova just booted with a self-signed certificate is the runner's most common TLS
    /// subject, and verified-only TLS would leave boot-then-probe untestable over TLS.
    pub(crate) insecure: bool,
    /// A PEM file whose certificates are **added** to the default trust anchors. Additive rather
    /// than replacing, because that is what makes one client able to reach a private endpoint and
    /// a public one; narrowing trust is not something an author can ask for by naming a CA.
    pub(crate) ca_cert: Option<PathBuf>,
}

impl Tls {
    /// Read `insecure`/`ca_cert` out of an options table. The caller has already rejected unknown
    /// keys, so this only interprets.
    ///
    /// **Both at once is an error.** They are contradictory statements of intent — *trust this one
    /// CA* against *trust anything* — and the failure mode of letting one quietly win is a proof
    /// that goes on passing after its certificate pinning has stopped meaning anything. tonic
    /// enforces exactly this at the type level (a custom verifier replaces the root store, so the
    /// two cannot be combined); lifting the refusal here is what stops one transport from being
    /// stricter than its neighbours.
    pub(crate) fn from_opts(opts: &Table, who: &str) -> mlua::Result<Self> {
        let insecure = opts.get::<Option<bool>>("insecure")?.unwrap_or(false);
        let ca_cert = opts.get::<Option<String>>("ca_cert")?.map(PathBuf::from);
        if insecure && ca_cert.is_some() {
            return Err(mlua::Error::RuntimeError(format!(
                "{who}: `insecure` and `ca_cert` say contradictory things — `ca_cert` adds a trust \
                 anchor, `insecure` stops verifying at all, so together the CA is not checked and \
                 the proof only looks pinned. Name exactly one."
            )));
        }
        // Refused where it is *written*, not where it would have had an effect: an ignored
        // `insecure` in a build without TLS is the silent-option failure this repo closes
        // everywhere else (docs/design/agent-ergonomics.md#module-opts-silently-ignored).
        #[cfg(not(feature = "tls"))]
        if insecure || ca_cert.is_some() {
            let key = if insecure { "insecure" } else { "ca_cert" };
            return Err(unavailable(who, &format!("the `{key}` option")));
        }
        Ok(Self { insecure, ca_cert })
    }

    /// True when nothing was asked for, so a caller can keep its existing shared-client fast path
    /// instead of building a configured one per request.
    pub(crate) fn is_default(&self) -> bool {
        !self.insecure && self.ca_cert.is_none()
    }

    /// The scheme gate every transport calls before it connects.
    ///
    /// One function rather than three `starts_with` checks, because the *error* is the deliverable:
    /// each transport used to reject (or fail on) a TLS URL in its own words, and one of them —
    /// `http` — did not reject it at all but let reqwest report a URL problem for a build problem.
    pub(crate) fn require_for_url(who: &str, url: &str) -> mlua::Result<()> {
        let _ = (who, url);
        #[cfg(not(feature = "tls"))]
        if url.starts_with("https://") || url.starts_with("wss://") {
            let scheme = if url.starts_with("wss://") { "wss://" } else { "https://" };
            return Err(unavailable(who, &format!("a {scheme} URL")));
        }
        Ok(())
    }

    /// Read the `ca_cert` PEM off disk.
    ///
    /// The path is read here rather than handed to a crate's own `from_pem_file` so the error names
    /// the file and the option that pointed at it. A missing or unreadable CA otherwise surfaces as
    /// a handshake failure several layers down, which reads as "the server is misconfigured".
    #[cfg(feature = "tls")]
    fn ca_pem(path: &std::path::Path, who: &str) -> mlua::Result<Vec<u8>> {
        std::fs::read(path).map_err(|e| {
            mlua::Error::RuntimeError(format!(
                "{who}: reading `ca_cert` {}: {e}",
                path.display()
            ))
        })
    }

    /// The trust anchors from `ca_cert`, parsed as DER.
    ///
    /// An empty PEM is refused rather than accepted as "no extra anchors": a file that parses to
    /// nothing means the author pointed at the wrong thing (a key, a CSR, a truncated download),
    /// and silently trusting only the defaults would let a proof that believes it is pinned pass
    /// against a public CA.
    #[cfg(all(feature = "tls", any(feature = "grpc", feature = "http", feature = "graphql")))]
    fn ca_ders(
        path: &std::path::Path,
        who: &str,
    ) -> mlua::Result<Vec<rustls_pki_types::CertificateDer<'static>>> {
        use rustls_pki_types::pem::PemObject;
        let pem = Self::ca_pem(path, who)?;
        let mut ders = Vec::new();
        for cert in rustls_pki_types::CertificateDer::pem_slice_iter(&pem) {
            ders.push(cert.map_err(|e| {
                mlua::Error::RuntimeError(format!(
                    "{who}: parsing `ca_cert` {}: {e}",
                    path.display()
                ))
            })?);
        }
        if ders.is_empty() {
            return Err(mlua::Error::RuntimeError(format!(
                "{who}: `ca_cert` {} contains no CERTIFICATE block — a PEM that parses to no \
                 anchors would leave this connection trusting only the default roots while the \
                 proof reads as pinned",
                path.display()
            )));
        }
        Ok(ders)
    }

    /// A `rustls::ClientConfig` for the transports whose crates expose no policy of their own
    /// (`websocket`, and `grpc`'s `insecure` path, which needs a verifier object).
    ///
    /// The crypto provider is named rather than taken from `CryptoProvider::get_default()`, which
    /// panics when a dep graph carries two providers — a crate elsewhere enabling `aws-lc-rs` would
    /// otherwise turn a working build into a startup crash with nothing in this tree to explain it.
    #[cfg(feature = "tls")]
    pub(crate) fn rustls_config(&self, who: &str) -> mlua::Result<rustls::ClientConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| {
                mlua::Error::RuntimeError(format!("{who}: configuring TLS: {e}"))
            })?;
        let config = if self.insecure {
            builder
                .dangerous()
                .with_custom_certificate_verifier(insecure_verifier(provider))
                .with_no_client_auth()
        } else {
            builder
                .with_root_certificates(Arc::new(self.root_store(who)?))
                .with_no_client_auth()
        };
        Ok(config)
    }

    /// Both root sets, plus `ca_cert`'s anchors when given.
    ///
    /// Native roots are added best-effort: a scratch container legitimately has no platform trust
    /// store, and failing there would make the bundled roots — the whole reason they are compiled
    /// in — unreachable. `webpki-roots` is unconditional, so the store is never empty.
    #[cfg(feature = "tls")]
    fn root_store(&self, who: &str) -> mlua::Result<rustls::RootCertStore> {
        let mut store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let native = rustls_native_certs::load_native_certs();
        store.add_parsable_certificates(native.certs);
        if let Some(path) = &self.ca_cert {
            use rustls_pki_types::pem::PemObject;
            let pem = Self::ca_pem(path, who)?;
            let mut added = 0usize;
            for cert in rustls_pki_types::CertificateDer::pem_slice_iter(&pem) {
                let cert = cert.map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "{who}: parsing `ca_cert` {}: {e}",
                        path.display()
                    ))
                })?;
                store.add(cert).map_err(|e| {
                    mlua::Error::RuntimeError(format!(
                        "{who}: `ca_cert` {} is not a usable trust anchor: {e}",
                        path.display()
                    ))
                })?;
                added += 1;
            }
            if added == 0 {
                return Err(mlua::Error::RuntimeError(format!(
                    "{who}: `ca_cert` {} contains no CERTIFICATE block — a PEM that parses to no \
                     anchors would leave this connection trusting only the default roots while the \
                     proof reads as pinned",
                    path.display()
                )));
            }
        }
        Ok(store)
    }

    /// Apply the policy to a reqwest builder (`http`, `graphql`).
    ///
    /// reqwest owns both root sets already (both features are on, and it enables both stores by
    /// default), so only the two deltas are expressed here — which keeps the common path on
    /// reqwest's own verifier rather than a hand-built one.
    #[cfg(all(feature = "tls", any(feature = "http", feature = "graphql")))]
    pub(crate) fn apply_reqwest(
        &self,
        builder: reqwest::ClientBuilder,
        who: &str,
    ) -> mlua::Result<reqwest::ClientBuilder> {
        let mut builder = builder;
        if self.insecure {
            builder = builder.danger_accept_invalid_certs(true);
        }
        if let Some(path) = &self.ca_cert {
            // One `Certificate` per block, not `from_pem_bundle`: the DER round-trip is already
            // done by `ca_ders`, whose emptiness check is the part worth keeping.
            for der in Self::ca_ders(path, who)? {
                let cert = reqwest::Certificate::from_der(&der).map_err(|e| {
                    mlua::Error::RuntimeError(format!("{who}: `ca_cert` is not usable: {e}"))
                })?;
                builder = builder.add_root_certificate(cert);
            }
        }
        Ok(builder)
    }

    /// Apply the policy to a tonic endpoint (`grpc`).
    ///
    /// The two branches are genuinely different tonic calls rather than one config with a flag:
    /// a custom verifier *replaces* the default one, so tonic errors if a root-store method was
    /// also set. `from_opts` has already refused the combination, which is what lets this be a
    /// clean either/or.
    #[cfg(all(feature = "tls", feature = "grpc"))]
    pub(crate) fn apply_tonic(
        &self,
        endpoint: tonic::transport::Endpoint,
        who: &str,
    ) -> mlua::Result<tonic::transport::Endpoint> {
        let err = |e: tonic::transport::Error| {
            mlua::Error::RuntimeError(format!("{who}: configuring TLS: {e}"))
        };
        if self.insecure {
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            return endpoint
                .tls_config_with_verifier(
                    tonic::transport::ClientTlsConfig::new(),
                    insecure_verifier(provider),
                )
                .map_err(err);
        }
        // `with_enabled_roots` is both compiled root sets — the same policy `root_store` builds by
        // hand for the transports that have no equivalent.
        let mut cfg = tonic::transport::ClientTlsConfig::new().with_enabled_roots();
        if let Some(path) = &self.ca_cert {
            for der in Self::ca_ders(path, who)? {
                cfg = cfg.ca_certificate(tonic::transport::Certificate::from_pem(
                    pem_encode(&der),
                ));
            }
        }
        endpoint.tls_config(cfg).map_err(err)
    }
}

/// The teaching error for a TLS surface reached in a build compiled without the feature.
///
/// It names the missing feature and offers the `requires` escape, matching `absent_stub`'s wording
/// for an absent namespace — a proof that cannot run here should be able to *skip* here, and an
/// author who has to guess the feature's name reaches for curl instead.
#[cfg_attr(feature = "tls", allow(dead_code))]
pub(crate) fn unavailable(who: &str, what: &str) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "{who}: {what} needs TLS, which is not compiled into this build — rebuild with the `tls` \
         feature (it is on by default), or gate the test with requires = {{ \"tls\" }} to skip \
         instead"
    ))
}

/// Re-encode a DER certificate as PEM, for tonic's PEM-only `Certificate::from_pem`.
///
/// Round-tripping through DER first is deliberate: it means `ca_ders`' parse and emptiness checks
/// run for tonic exactly as they do for reqwest, so a bad `ca_cert` fails with the same message on
/// both rather than as a tonic handshake error on one.
#[cfg(all(feature = "tls", feature = "grpc"))]
fn pem_encode(der: &rustls_pki_types::CertificateDer<'_>) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der.as_ref());
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    // Sliced by byte offset rather than chunked over `as_bytes()`: base64 is ASCII, so every
    // 64-byte boundary is a char boundary, and indexing the `String` says so without asserting it.
    let mut at = 0;
    while at < b64.len() {
        let end = (at + 64).min(b64.len());
        out.push_str(&b64[at..end]);
        out.push('\n');
        at = end;
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}

/// A verifier that accepts every certificate — what `insecure = true` installs.
///
/// Written out rather than pulled from a crate because every "danger" helper crate is one more
/// supply-chain edge for forty lines that must be read to be trusted anyway. The signature
/// algorithms come from the real provider, so `insecure` weakens *identity* checking only: the
/// handshake still negotiates and encrypts, which is what keeps this useful for a self-signed
/// localhost service rather than a way to talk to a plaintext port.
#[cfg(feature = "tls")]
fn insecure_verifier(
    provider: Arc<rustls::crypto::CryptoProvider>,
) -> Arc<dyn rustls::client::danger::ServerCertVerifier> {
    #[derive(Debug)]
    struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);

    impl rustls::client::danger::ServerCertVerifier for AcceptAny {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls_pki_types::CertificateDer<'_>,
            _intermediates: &[rustls_pki_types::CertificateDer<'_>],
            _server_name: &rustls_pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls_pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &rustls_pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls12_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &rustls_pki_types::CertificateDer<'_>,
            dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            rustls::crypto::verify_tls13_signature(
                message,
                cert,
                dss,
                &self.0.signature_verification_algorithms,
            )
        }

        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            self.0
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    Arc::new(AcceptAny(provider))
}

#[cfg(all(test, feature = "tls"))]
mod tests {
    use super::*;

    fn opts(lua: &mlua::Lua, pairs: &[(&str, mlua::Value)]) -> Table {
        let t = lua.create_table().unwrap();
        for (k, v) in pairs {
            t.set(*k, v.clone()).unwrap();
        }
        t
    }

    /// The no-options case is verified TLS — the property the refusals below are only meaningful
    /// against, since a parser that rejected everything would satisfy them all.
    #[test]
    fn absent_options_are_verified_tls() {
        let lua = mlua::Lua::new();
        let tls = Tls::from_opts(&opts(&lua, &[]), "http.get").unwrap();
        assert_eq!(tls, Tls::default());
        assert!(tls.is_default());
    }

    #[test]
    fn insecure_is_read() {
        let lua = mlua::Lua::new();
        let tls =
            Tls::from_opts(&opts(&lua, &[("insecure", mlua::Value::Boolean(true))]), "http.get")
                .unwrap();
        assert!(tls.insecure);
        assert!(!tls.is_default());
    }

    #[test]
    fn insecure_false_is_still_the_default_path() {
        let lua = mlua::Lua::new();
        let tls = Tls::from_opts(
            &opts(&lua, &[("insecure", mlua::Value::Boolean(false))]),
            "http.get",
        )
        .unwrap();
        assert!(tls.is_default(), "insecure = false must not build a configured client");
    }

    #[test]
    fn ca_cert_is_read_as_a_path() {
        let lua = mlua::Lua::new();
        let ca = lua.create_string("/tmp/ca.pem").unwrap();
        let tls =
            Tls::from_opts(&opts(&lua, &[("ca_cert", mlua::Value::String(ca))]), "http.get")
                .unwrap();
        assert_eq!(tls.ca_cert, Some(PathBuf::from("/tmp/ca.pem")));
        assert!(!tls.is_default());
    }

    /// The refusal that keeps a proof from reading as pinned while trusting anything.
    #[test]
    fn insecure_with_ca_cert_is_refused() {
        let lua = mlua::Lua::new();
        let ca = lua.create_string("/tmp/ca.pem").unwrap();
        let err = Tls::from_opts(
            &opts(
                &lua,
                &[
                    ("insecure", mlua::Value::Boolean(true)),
                    ("ca_cert", mlua::Value::String(ca)),
                ],
            ),
            "websocket.connect",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("websocket.connect"), "names the call site: {err}");
        assert!(err.contains("contradictory"), "says why: {err}");
        assert!(err.contains("Name exactly one"), "says what to do: {err}");
    }

    /// `insecure = false` alongside a `ca_cert` is not the contradiction — only an explicit
    /// `true` is, and a parser that keyed off presence rather than value would reject valid code.
    #[test]
    fn ca_cert_with_insecure_false_is_accepted() {
        let lua = mlua::Lua::new();
        let ca = lua.create_string("/tmp/ca.pem").unwrap();
        let tls = Tls::from_opts(
            &opts(
                &lua,
                &[
                    ("insecure", mlua::Value::Boolean(false)),
                    ("ca_cert", mlua::Value::String(ca)),
                ],
            ),
            "http.get",
        )
        .unwrap();
        assert_eq!(tls.ca_cert, Some(PathBuf::from("/tmp/ca.pem")));
    }

    /// A missing CA names the file and the option, not a handshake several layers down.
    #[test]
    fn a_missing_ca_cert_names_the_path() {
        let lua = mlua::Lua::new();
        let ca = lua.create_string("/nonexistent/ca.pem").unwrap();
        let tls =
            Tls::from_opts(&opts(&lua, &[("ca_cert", mlua::Value::String(ca))]), "http.get")
                .unwrap();
        let err = tls.rustls_config("http.get").unwrap_err().to_string();
        assert!(err.contains("ca_cert"), "names the option: {err}");
        assert!(err.contains("/nonexistent/ca.pem"), "names the path: {err}");
    }

    /// A PEM with no CERTIFICATE block is a wrong-file mistake, not "no extra anchors".
    #[test]
    fn an_empty_ca_cert_is_refused() {
        let lua = mlua::Lua::new();
        let dir = std::env::temp_dir().join("prova-tls-empty-ca");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-a-cert.pem");
        std::fs::write(&path, b"-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n")
            .unwrap();
        let ca = lua.create_string(path.to_str().unwrap()).unwrap();
        let tls =
            Tls::from_opts(&opts(&lua, &[("ca_cert", mlua::Value::String(ca))]), "http.get")
                .unwrap();
        let err = tls.rustls_config("http.get").unwrap_err().to_string();
        assert!(
            err.contains("no CERTIFICATE block"),
            "says what is wrong with the file: {err}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The default store is never empty — a machine with no platform trust store still has the
    /// bundled roots, which is the whole reason both sets are compiled in.
    #[test]
    fn the_default_root_store_is_not_empty() {
        let store = Tls::default().root_store("http.get").unwrap();
        assert!(
            store.roots.len() > 50,
            "expected Mozilla's bundle, got {} anchors",
            store.roots.len()
        );
    }

    /// Verified and insecure produce genuinely different configs — the negative control for
    /// `insecure`, which would otherwise be satisfied by a flag that changed nothing.
    #[test]
    fn insecure_installs_a_different_verifier() {
        let verified = Tls::default().rustls_config("http.get").unwrap();
        let insecure = Tls {
            insecure: true,
            ca_cert: None,
        }
        .rustls_config("http.get")
        .unwrap();
        // `ClientConfig` exposes no verifier accessor, so compare the observable consequence:
        // a verified config carries the webpki root anchors, a custom-verifier config carries none.
        assert!(
            format!("{verified:?}").len() != format!("{insecure:?}").len(),
            "insecure must not build the same config as verified"
        );
    }
}
