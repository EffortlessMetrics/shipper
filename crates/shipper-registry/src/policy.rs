//! Registry destination validation and trust-boundary types.
//!
//! Registry URLs are hostile configuration input. A registry client may attach
//! a publish token to owner requests, so validation must happen before a
//! request target is built and before credentials are added to a request.
//!
//! This module deliberately does not resolve DNS names or follow redirects.
//! Those network-time decisions belong to the follow-up enforcement lane. It
//! does validate all literal IP ranges and preserves the approved authorities
//! needed by that lane.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use anyhow::{Result, anyhow, bail};
use url::{Host, Url};

use shipper_types::{Registry, RegistryPolicyEvidence, RegistryPolicyPosture};

const METADATA_HOSTS: &[&str] = &[
    "instance-data",
    "metadata",
    "metadata.google.internal",
    "metadata.goog",
    "metadata.azure.com",
    "metadata.digitalocean.com",
    "metadata.hetzner.cloud",
    "metadata.ibm.com",
    "metadata.internal",
    "metadata.oraclecloud.com",
    "kubernetes.default.svc",
];

/// Explicit trust choices for a registry destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegistryPolicy {
    /// Permit literal RFC1918, carrier-grade NAT, and IPv6 unique-local
    /// destinations. HTTPS remains required.
    pub allow_private: bool,
    /// Permit loopback and plain HTTP for an explicit rehearsal/test client.
    /// This is intentionally separate from `allow_private`.
    pub allow_loopback: bool,
}

impl RegistryPolicy {
    /// Secure default for live registry traffic.
    pub const fn secure() -> Self {
        Self {
            allow_private: false,
            allow_loopback: false,
        }
    }

    /// Explicit rehearsal/test posture. It still rejects metadata and
    /// link-local destinations unconditionally.
    pub const fn rehearsal() -> Self {
        Self {
            allow_private: false,
            allow_loopback: true,
        }
    }

    /// Return a copy that permits private-network destinations.
    pub const fn with_private(self, allow_private: bool) -> Self {
        Self {
            allow_private,
            allow_loopback: self.allow_loopback,
        }
    }

    /// Return a copy that permits loopback HTTP for an explicit rehearsal or
    /// test client.
    pub const fn with_loopback(self, allow_loopback: bool) -> Self {
        Self {
            allow_private: self.allow_private,
            allow_loopback,
        }
    }
}

/// Parsed and approved registry network identity.
#[derive(Debug, Clone)]
pub struct ValidatedRegistry {
    display: Registry,
    api_base: Url,
    index_base: Url,
    credential_authority: RegistryAuthority,
    policy: RegistryPolicy,
}

/// Sanitized authority identity used for credential-destination checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryAuthority {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
}

/// Return whether an index destination belongs to the same explicitly
/// configured registry family as the credential-bearing API destination.
///
/// DNS names must either match exactly or use the conventional explicit
/// `index.<api-host>` relationship. This conservative rule avoids guessing a
/// registrable domain without a public-suffix list. The built-in
/// crates.io/index.crates.io pair is covered by that relationship. Literal IP
/// destinations must match exactly. Schemes and effective ports always match.
pub fn authorities_share_trusted_domain(
    credential: &RegistryAuthority,
    index: &RegistryAuthority,
) -> bool {
    if credential.scheme != index.scheme || credential.port != index.port {
        return false;
    }

    let credential_ip = credential.host.parse::<IpAddr>().ok();
    let index_ip = index.host.parse::<IpAddr>().ok();
    match (credential_ip, index_ip) {
        (Some(left), Some(right)) => left == right,
        (Some(_), None) | (None, Some(_)) => false,
        (None, None) => trusted_dns_pair(&credential.host, &index.host),
    }
}

fn trusted_dns_pair(credential: &str, index: &str) -> bool {
    credential == index || index == format!("index.{credential}")
}

impl ValidatedRegistry {
    /// Parse and validate a registry under explicit trust choices.
    pub fn new(registry: Registry, policy: RegistryPolicy) -> Result<Self> {
        let api_base = parse_and_validate_url(&registry.api_base, "api_base", policy)?;
        let index_raw = explicit_index_base(&registry, &api_base)?;
        let index_base = parse_and_validate_url(index_raw, "index_base", policy)?;
        let credential_authority = authority_for(&api_base)?;
        let index_authority = authority_for(&index_base)?;
        if !authorities_share_trusted_domain(&credential_authority, &index_authority) {
            bail!(
                "registry api_base and index_base must use the same scheme, port, and trusted host identity"
            )
        }

        Ok(Self {
            display: registry,
            api_base,
            index_base,
            credential_authority,
            policy,
        })
    }

    pub fn display(&self) -> &Registry {
        &self.display
    }

    pub fn api_base(&self) -> &Url {
        &self.api_base
    }

    pub fn index_base(&self) -> &Url {
        &self.index_base
    }

    pub fn credential_authority(&self) -> &RegistryAuthority {
        &self.credential_authority
    }

    pub fn policy(&self) -> RegistryPolicy {
        self.policy
    }

    /// Evidence contains only policy posture and authorities; it never
    /// contains tokens, URL userinfo, query strings, or fragments.
    pub fn sanitized_evidence(&self) -> RegistryPolicyEvidence {
        RegistryPolicyEvidence {
            posture: if self.policy.allow_loopback {
                RegistryPolicyPosture::RehearsalOrTest
            } else if self.policy.allow_private {
                RegistryPolicyPosture::PrivateOptIn
            } else {
                RegistryPolicyPosture::PublicDefault
            },
            allow_private: self.policy.allow_private,
            allow_loopback: self.policy.allow_loopback,
            credential_authority: authority_string(&self.credential_authority),
            index_authority: authority_for(&self.index_base)
                .map(|authority| authority_string(&authority))
                .unwrap_or_else(|_| "unknown".to_string()),
        }
    }
}

fn explicit_index_base<'a>(registry: &'a Registry, api_base: &Url) -> Result<&'a str> {
    if let Some(index_base) = registry.index_base.as_deref() {
        let index_base = index_base.strip_prefix("sparse+").unwrap_or(index_base);
        if index_base.trim().is_empty() {
            bail!("registry index_base must not be blank")
        }
        return Ok(index_base);
    }

    if registry.name == "crates-io" && api_base.host_str() == Some("crates.io") {
        return Ok("https://index.crates.io");
    }

    bail!(
        "registry '{}' requires an explicit index_base; refusing to guess an index host from api_base",
        registry.name
    )
}

fn parse_and_validate_url(value: &str, field: &str, policy: RegistryPolicy) -> Result<Url> {
    let url = Url::parse(value).map_err(|err| anyhow!("invalid registry {field} URL: {err}"))?;

    if !url.username().is_empty() || url.password().is_some() {
        bail!("registry {field} URL must not contain userinfo")
    }
    if url.query().is_some() || url.fragment().is_some() {
        bail!("registry {field} URL must not contain a query or fragment")
    }

    let host = url
        .host()
        .ok_or_else(|| anyhow!("registry {field} URL has no host"))?;
    let host_is_loopback = host_is_loopback(&host);

    match url.scheme() {
        "https" => {}
        "http" if policy.allow_loopback && host_is_loopback => {}
        "http" => bail!(
            "registry {field} URL must use https; plain http is reserved for an explicit loopback rehearsal/test posture"
        ),
        scheme => bail!(
            "registry {field} URL uses unsupported scheme {scheme:?}; registry URLs must use https"
        ),
    }

    match host {
        Host::Domain(domain) => validate_domain(domain, field, policy)?,
        Host::Ipv4(address) => validate_ip(IpAddr::V4(address), field, policy)?,
        Host::Ipv6(address) => validate_ip(IpAddr::V6(address), field, policy)?,
    }

    Ok(url)
}

fn validate_domain(domain: &str, field: &str, policy: RegistryPolicy) -> Result<()> {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if METADATA_HOSTS.contains(&domain.as_str()) || domain.starts_with("metadata.") {
        bail!("registry {field} host is a cloud metadata endpoint")
    }

    let local = domain == "localhost" || domain.ends_with(".localhost");
    if local && !policy.allow_loopback {
        bail!("registry {field} host is local; explicit rehearsal/test posture is required")
    }

    Ok(())
}

fn validate_ip(address: IpAddr, field: &str, policy: RegistryPolicy) -> Result<()> {
    let address = match address {
        IpAddr::V6(v6) => embedded_ipv4(v6).map_or(IpAddr::V6(v6), IpAddr::V4),
        address => address,
    };

    if is_link_local(address) {
        bail!("registry {field} host is link-local or metadata-routed")
    }
    if address.is_unspecified() {
        bail!("registry {field} host is the unspecified address")
    }
    if address.is_loopback() && !policy.allow_loopback {
        bail!("registry {field} host is loopback; explicit rehearsal/test posture is required")
    }
    if is_private(address) && !address.is_loopback() && !policy.allow_private {
        bail!("registry {field} host is private; set allow_private = true explicitly")
    }

    Ok(())
}

/// Return IPv4 addresses embedded in IPv4-compatible or IPv4-mapped IPv6
/// forms so they receive the same private, loopback, and metadata checks.
fn embedded_ipv4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    if address.is_loopback() || address.is_unspecified() {
        return None;
    }

    let segments = address.segments();
    if segments[..6].iter().all(|segment| *segment == 0) {
        return Some(Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        ));
    }

    address.to_ipv4_mapped()
}

fn host_is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            domain == "localhost" || domain.ends_with(".localhost")
        }
        Host::Ipv4(address) => address.is_loopback(),
        Host::Ipv6(address) => {
            address.is_loopback()
                || embedded_ipv4(*address).is_some_and(|address| address.is_loopback())
        }
    }
}

fn is_link_local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_link_local(),
        IpAddr::V6(address) => (address.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn is_private(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private() || is_shared_v4(address) || address.octets()[0] == 0
        }
        IpAddr::V6(address) => (address.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn is_shared_v4(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    first == 100 && (64..128).contains(&second)
}

fn authority_for(url: &Url) -> Result<RegistryAuthority> {
    Ok(RegistryAuthority {
        scheme: url.scheme().to_ascii_lowercase(),
        host: url
            .host_str()
            .ok_or_else(|| anyhow!("validated URL has no host"))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_end_matches('.')
            .to_ascii_lowercase(),
        port: url.port_or_known_default(),
    })
}

fn authority_string(authority: &RegistryAuthority) -> String {
    let host = if authority.host.contains(':') {
        format!("[{}]", authority.host)
    } else {
        authority.host.clone()
    };
    match authority.port {
        Some(port) => format!("{}://{host}:{port}", authority.scheme),
        None => format!("{}://{host}", authority.scheme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(api_base: &str, index_base: Option<&str>) -> Registry {
        Registry {
            name: "custom".to_string(),
            api_base: api_base.to_string(),
            index_base: index_base.map(str::to_string),
        }
    }

    #[test]
    fn secure_defaults_require_https_and_explicit_index() {
        let err = ValidatedRegistry::new(
            registry("http://registry.example", Some("https://index.example")),
            RegistryPolicy::secure(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("must use https"));

        let err = ValidatedRegistry::new(
            registry("https://registry.example", None),
            RegistryPolicy::secure(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("explicit index_base"));
    }

    #[test]
    fn rejects_userinfo_queries_metadata_and_link_local() {
        for api in [
            "https://user:pass@registry.example",
            "https://registry.example/?token=secret",
            "https://169.254.169.254",
            "https://[fe80::1]",
        ] {
            let err = ValidatedRegistry::new(
                registry(api, Some("https://index.example")),
                RegistryPolicy::secure(),
            )
            .expect_err(api);
            assert!(!err.to_string().contains("secret"));
        }
    }

    #[test]
    fn rejects_metadata_hostnames_and_shared_private_addresses() {
        for host in [
            "instance-data",
            "metadata",
            "metadata.google.internal",
            "metadata.goog",
            "metadata.azure.com",
            "metadata.internal",
            "kubernetes.default.svc",
        ] {
            let url = format!("https://{host}");
            let err = ValidatedRegistry::new(registry(&url, Some(&url)), RegistryPolicy::secure())
                .expect_err(host);
            assert!(err.to_string().contains("metadata"));
        }

        for host in ["100.64.0.1", "0.1.2.3"] {
            let url = format!("https://{host}");
            ValidatedRegistry::new(registry(&url, Some(&url)), RegistryPolicy::secure())
                .expect_err(host);
            ValidatedRegistry::new(
                registry(&url, Some(&url)),
                RegistryPolicy::secure().with_private(true),
            )
            .expect("explicit private opt-in");
        }
    }

    #[test]
    fn private_and_loopback_postures_are_separate() {
        let private = registry("https://10.0.0.5", Some("https://10.0.0.5"));
        ValidatedRegistry::new(private.clone(), RegistryPolicy::secure()).expect_err("private");
        let validated =
            ValidatedRegistry::new(private, RegistryPolicy::secure().with_private(true))
                .expect("private opt-in");
        assert_eq!(
            validated.sanitized_evidence().posture,
            RegistryPolicyPosture::PrivateOptIn
        );

        let local = registry("http://127.0.0.1:8080", Some("http://127.0.0.1:8080"));
        ValidatedRegistry::new(local.clone(), RegistryPolicy::secure()).expect_err("loopback");
        let validated =
            ValidatedRegistry::new(local, RegistryPolicy::rehearsal()).expect("rehearsal");
        assert_eq!(
            validated.sanitized_evidence().posture,
            RegistryPolicyPosture::RehearsalOrTest
        );
    }

    #[test]
    fn evidence_contains_authorities_but_not_url_secrets() {
        let validated = ValidatedRegistry::new(
            registry(
                "https://registry.example.com/api",
                Some("https://index.registry.example.com"),
            ),
            RegistryPolicy::secure(),
        )
        .expect("valid");
        let evidence = validated.sanitized_evidence();
        assert_eq!(
            evidence.credential_authority,
            "https://registry.example.com:443"
        );
        assert_eq!(
            evidence.index_authority,
            "https://index.registry.example.com:443"
        );
        let json = serde_json::to_string(&evidence).expect("json");
        assert!(!json.contains("token"));
    }

    #[test]
    fn shared_two_label_public_suffix_is_not_a_registry_identity() {
        let err = ValidatedRegistry::new(
            registry(
                "https://registry.example.co.uk",
                Some("https://index.attacker.co.uk"),
            ),
            RegistryPolicy::secure(),
        )
        .expect_err("unrelated hosts must not share co.uk trust");
        assert!(err.to_string().contains("trusted host identity"));

        ValidatedRegistry::new(
            registry("https://crates.io", Some("https://index.crates.io")),
            RegistryPolicy::secure(),
        )
        .expect("the documented crates.io pair remains valid");

        ValidatedRegistry::new(
            registry(
                "https://registry.example.co.uk",
                Some("https://index.registry.example.co.uk"),
            ),
            RegistryPolicy::secure(),
        )
        .expect("the explicit index subdomain remains valid");
    }

    #[test]
    fn evidence_brackets_ipv6_authorities() {
        let validated = ValidatedRegistry::new(
            registry("https://[2001:db8::10]", Some("https://[2001:db8::10]")),
            RegistryPolicy::secure(),
        )
        .expect("valid IPv6 registry");
        let evidence = validated.sanitized_evidence();
        assert_eq!(evidence.credential_authority, "https://[2001:db8::10]:443");
        assert_eq!(evidence.index_authority, "https://[2001:db8::10]:443");
    }

    #[test]
    fn ipv6_unique_local_requires_private_opt_in() {
        let private = registry("https://[fd00::10]", Some("https://[fd00::10]"));
        ValidatedRegistry::new(private.clone(), RegistryPolicy::secure())
            .expect_err("private IPv6 destination must be opted in");
        ValidatedRegistry::new(private, RegistryPolicy::secure().with_private(true))
            .expect("private IPv6 opt-in");
    }

    #[test]
    fn ipv4_compatible_ipv6_requires_private_opt_in() {
        for host in ["[::10.0.0.1]", "[::192.168.1.1]"] {
            let private = registry(&format!("https://{host}"), Some(&format!("https://{host}")));
            ValidatedRegistry::new(private, RegistryPolicy::secure())
                .expect_err("embedded private IPv4 must be rejected");
        }

        let private = registry("https://[::10.0.0.1]", Some("https://[::10.0.0.1]"));
        ValidatedRegistry::new(private, RegistryPolicy::secure().with_private(true))
            .expect("private opt-in should cover embedded IPv4");
    }

    #[test]
    fn ipv6_loopback_forms_require_rehearsal_posture() {
        for host in ["[::1]", "[::ffff:127.0.0.1]", "[::127.0.0.1]"] {
            let https = format!("https://{host}");
            let error =
                ValidatedRegistry::new(registry(&https, Some(&https)), RegistryPolicy::secure())
                    .expect_err(host);
            assert!(error.to_string().contains("loopback"), "{host}: {error}");

            let http = format!("http://{host}:8080");
            ValidatedRegistry::new(registry(&http, Some(&http)), RegistryPolicy::rehearsal())
                .expect("explicit rehearsal posture should permit IPv6 loopback");
        }
    }
}
