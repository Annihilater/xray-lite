# v0.2.8 Release Notes

## 🌟 Reality Protocol Support (TLS 1.3)

This release introduces comprehensive support for the **Reality Protocol**, designed to bypass advanced censorship by masquerading as legitimate TLS traffic.

### Key Features:
- **Sniff-and-Dispatch Fallback**: The server intelligently distinguishes between legitimate Reality clients and random scanners/browsers.
  - **Reality Clients**: Authenticated and proxied via VLESS protocol.
  - **Scanners**: Transparently forwarded to the fallback destination (e.g., `www.microsoft.com`), rendering the proxy invisible to active probing.
- **Client Verification**: Implements robust cryptographic verification (X25519 ECDH + HKDF + AES-GCM) to authenticate clients during the TLS handshake.
- **ShortId Support**: Automatically generates unique ShortIDs to secure the connection further.

### Technical Upgrades:
- **Rustls 0.22**: Upgraded backend TLS engine for enhanced security and performance.
- **Dependencies**: Integrated `ring`, `x25519-dalek`, and `aes-gcm`.

### How to Update:
Run the installation script again (it will update the existing installation):
```bash
wget -qO- https://raw.githubusercontent.com/undead-undead/xray-lite/main/install.sh | bash
```
