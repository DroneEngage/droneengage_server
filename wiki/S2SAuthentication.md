# S2S Authentication Guide

This guide explains how to configure and use Server-to-Server (S2S) authentication for Andruav. S2S authentication secures WebSocket connections between servers using Ed25519 public/private key pairs.

## What is S2S Authentication?

S2S authentication secures the following WebSocket channels:

- **Auth Server ⟷ Communication Server** - The control channel for authentication
- **Parent (Super) Comm Server ⟷ Child Comm Server** - The relay channel for message forwarding

Without authentication, any server could connect to these channels. With S2S authentication enabled, only servers possessing the correct private key can establish connections.

## Per-Server Key Model

Each server has its own unique Ed25519 key pair. This provides better security than a shared key:

- **Compromise isolation**: If one server is compromised, the attacker cannot impersonate other servers
- **Server identity**: The accepting side can identify which server is connecting
- **Revocation**: Individual server keys can be revoked without affecting others

## How It Works

The authentication uses a challenge-response handshake:

1. When a server connects, the accepting side sends a random challenge (nonce)
2. The connecting server signs the challenge with its private key and includes its server ID
3. The accepting side looks up the public key for that server ID and verifies the signature
4. If valid, the connection proceeds; otherwise, it is terminated

## Quick Start

### 1. Generate Keys for Each Server

For each server, generate a unique key pair using the server ID:

```bash
cd /path/to/andruav_authenticator
./scripts/gen_s2s_keys.sh AndruavLap
./scripts/gen_s2s_keys.sh SuperServer
./scripts/gen_s2s_keys.sh DronCommServer
```

This creates files in the current directory:
- `<server_id>_private.pem` - Private key (keep secret)
- `<server_id>_public.pem` - Public key

### 2. Distribute Keys

**For each Communication Server:**
```bash
# Copy the private key to the server's ssl directory
cp AndruavLap_private.pem /path/to/andruav_server/server/ssl/s2s_ed25519_private.pem
```

**For the Auth Server:**
```bash
# Copy all public keys to the auth server's ssl directory
cp AndruavLap_public.pem /path/to/andruav_authenticator/ssl/
cp SuperServer_public.pem /path/to/andruav_authenticator/ssl/
cp DronCommServer_public.pem /path/to/andruav_authenticator/ssl/
```

**For the Parent (Super) Server:**
```bash
# Copy the private key for the parent server itself
cp SuperServer_private.pem /path/to/andruav_server/server/ssl/s2s_ed25519_private.pem
# Copy child server public keys
cp DronCommServer_public.pem /path/to/andruav_server/server/ssl/
```

### 3. Configure Accepting Servers

**Auth Server (`server.config`):**
```json
"s2s_auth_enabled": true,
"s2s_trusted_server_keys": {
    "AndruavLap": "./ssl/AndruavLap_public.pem",
    "SuperServer": "./ssl/SuperServer_public.pem",
    "DronCommServer": "./ssl/DronCommServer_public.pem"
}
```

**Parent Super Server (`deployment/server.s2s.super.config`):**
```json
"s2s_auth_enabled": true,
"s2s_my_private_key": "./ssl/SuperServer_private.pem",
"s2s_trusted_server_keys": {
    "DronCommServer": "./ssl/DronCommServer_public.pem"
}
```

### 4. Configure Connecting Servers

**Communication Server (`server.config`):**
```json
"s2s_auth_enabled": true,
"s2s_my_private_key": "./ssl/s2s_ed25519_private.pem"
```

**Child/Drone Server (`deployment/server.s2s.drone.config`):**
```json
"s2s_auth_enabled": true,
"s2s_my_private_key": "./ssl/s2s_ed25519_private.pem"
```

**Note:** Connecting servers only need the private key file since they don't accept connections.

### 5. Restart Servers

Restart all servers to apply the configuration changes.

## Configuration Reference

| Setting | Description | Required |
|---------|-------------|----------|
| `s2s_auth_enabled` | Enable/disable S2S authentication | Yes |
| `s2s_my_private_key` | Path to private key PEM file | Connecting servers only |
| `s2s_trusted_server_keys` | Object mapping server_id to public key file paths | Accepting servers only |

## Key File Locations

| Server | Private Key | Public Keys |
|--------|-------------|-------------|
| Auth Server | Not needed | `./ssl/<server_id>_public.pem` for each comm server |
| Comm Server (connecting only) | `./server/ssl/s2s_ed25519_private.pem` | Not needed |
| Parent Super Server | `./server/ssl/s2s_ed25519_private.pem` | `./server/ssl/<child_id>_public.pem` for each child |
| Child Server | `./server/ssl/s2s_ed25519_private.pem` | Not needed |

## Security Best Practices

- **Never commit private keys** to version control (add `*.pem` to `.gitignore`)
- **Restrict file permissions** - Private keys are automatically set to mode 0600
- **Use unique keys** - Each server must have its own key pair
- **Distribute keys securely** - Use out-of-band methods when possible
- **Regenerate keys periodically** - Re-run the script with the server ID to generate new keys
- **Keep backups** - Store private keys in a secure location
- **Revoke compromised keys** - Remove the public key from the accepting server's config

## Troubleshooting

### Connection Fails

**Check:**
- Key files exist at the configured paths
- File permissions are correct (private key should be 0600)
- `s2s_auth_enabled` is set to `true` in the config
- The server ID in the config matches the key filename
- The public key on the accepting side matches the private key on the connecting side

### "Missing private key" Error

The connecting server cannot find its private key file. Verify the path in `s2s_my_private_key` is correct.

### "Invalid signature" Error

The signature verification failed. This usually means:
- The public key on the accepting side does not match the private key on the connecting side
- Keys were regenerated but not redistributed to all servers
- The server ID sent during handshake doesn't match any configured public key

### "Unknown server_id" Error

The accepting server received a server ID that is not in its `s2s_trusted_server_keys` mapping. Add the server's public key to the config.

### Authentication Timeout

If the connecting server does not respond to the challenge within the timeout period, the connection remains unauthenticated. Check that:
- The connecting server has the private key
- The server is not experiencing network delays

## Disabling Authentication

To disable S2S authentication (not recommended for production):

```json
"s2s_auth_enabled": false
```

Restart the servers after changing this setting.
