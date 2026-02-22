# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.2.x   | :white_check_mark: |
| < 0.2.0 | :x:                |

We provide security updates for the current stable release series.

---

## Reporting a Vulnerability

We appreciate responsible disclosure of security vulnerabilities.

### How to Report

**Preferred Method:**
1. Go to [GitHub Security Advisories](https://github.com/cmrigney/shipper/security/advisories)
2. Click "Report a vulnerability"
3. Provide details about the vulnerability

**Alternative:**
If GitHub is not an option, email the maintainer directly. Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

### What to Expect

- **Acknowledgment**: Within 48 hours
- **Initial Assessment**: Within 7 days
- **Status Updates**: Weekly until resolved
- **Disclosure**: After fix is released (coordinated disclosure)

### Please Do Not

- Open public issues for security vulnerabilities
- Disclose vulnerabilities publicly before a fix is available
- Access or modify data that isn't yours

---

## Security Considerations

### Token Handling

Shipper handles registry tokens with care:

| Aspect | Implementation |
|--------|----------------|
| **Storage** | Tokens are never stored by Shipper. They are read from environment variables or cargo's credential store. |
| **Logging** | Tokens are never logged or included in debug output. |
| **State Files** | State files (`.shipper/state.json`) do not contain tokens. |
| **Receipts** | Receipts and event logs do not contain tokens. |

### Token Sources (in priority order)

1. `CARGO_REGISTRY_TOKEN` environment variable
2. `CARGO_TOKEN` environment variable (fallback)
3. Cargo credential store (`cargo login` configuration)

### Recommendations

- **Use environment variables** in CI/CD pipelines
- **Use cargo credential store** for local development
- **Rotate tokens** if you suspect compromise
- **Use scoped tokens** when possible (crates.io supports this)

---

### State File Security

State files contain:

- Crate names and versions (public information)
- Publish progress (not sensitive)
- Timestamps (not sensitive)

State files do **not** contain:

- Registry tokens
- Private keys
- User credentials

### File Permissions

Shipper creates files with appropriate permissions:

- State files: User read/write only (0600 on Unix)
- Lock files: User read/write only (0600 on Unix)

---

### Supply Chain Security

Shipper itself is published to crates.io. To verify authenticity:

1. **Verify the source**:
   ```bash
   cargo download shipper-cli
   # Compare with source at github.com/cmrigney/shipper
   ```

2. **Verify the checksum**:
   ```bash
   sha256sum ~/.cargo/registry/cache/*/shipper-*.crate
   ```

3. **Build from source** for maximum trust:
   ```bash
   git clone https://github.com/cmrigney/shipper.git
   cd shipper
   cargo build --release -p shipper-cli
   ```

---

### Known Security Considerations

#### 1. Token Exposure in Process List

On multi-user systems, environment variables may be visible in the process list. Mitigation:
- Use cargo credential store instead of environment variables
- Use single-user systems or containers for publishing

#### 2. State File Tampering

State files could theoretically be modified to skip crates. Mitigation:
- Run Shipper in trusted environments only
- Verify receipts after publishing
- Use file integrity monitoring in sensitive environments

#### 3. Network Interception

Registry traffic could be intercepted. Mitigation:
- crates.io uses HTTPS (TLS)
- Verify certificates are valid
- Use trusted networks for publishing

---

## Security Updates

Security updates will be:
- Announced in [GitHub Releases](https://github.com/cmrigney/shipper/releases)
- Documented in [CHANGELOG.md](CHANGELOG.md)
- Tagged with `security` label

---

## Contact

For security concerns:
- **GitHub Security**: [Report a vulnerability](https://github.com/cmrigney/shipper/security/advisories)
- **Issues**: For non-sensitive security questions, open a [GitHub Issue](https://github.com/cmrigney/shipper/issues)

---

Thank you for helping keep Shipper secure!