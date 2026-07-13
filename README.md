# Andruav Server

Andruav Server is the communication server responsible for exchanging messages between different units and GCSs running [Andruav or Drone-Engage](https://cloud.ardupilot.org).

## Architecture Overview

The communication server provides:
- WebSocket-based real-time messaging between units and GCS
- Server-to-server mesh relay for scalable message propagation
- Message routing with group and individual targeting
- System commands and task management
- Server-to-Server (S2S) authentication for secure relay connections

### Server Roles

The server can operate in different modes:
- **Standalone**: Independent server for local communication
- **Child Server**: Connects to a parent server for relay
- **Parent (Super) Server**: Accepts child connections and forwards messages

## Installation

### Prerequisites

- Node.js >= 18
- MySQL (for database-backed storage)
- OpenSSL (for SSL certificates)

### Setup

```bash
# Clone the repository
git clone https://github.com/HefnySco/andruav_server.git
cd andruav_server

# Install dependencies
npm install

# Copy configuration
cp server.config server.config.local
# Edit server.config.local with your settings

# Generate SSL certificates (if needed)
mkdir -p ssl_local/ssl_airgap
openssl req -x509 -newkey rsa:4096 -keyout ssl_local/ssl_airgap/domain.key -out ssl_local/ssl_airgap/domain.crt -days 365 -nodes
```

## Configuration

The server is configured via `server.config` (JSON format). Key settings:

### Server Settings

```json
{
    "server_id": "MyServer",
    "server_ip": "0.0.0.0",
    "server_port": 9966,
    "public_host": "127.0.0.1"
}
```

### Server Roles

**Parent (Super) Server Mode:**
```json
{
    "enable_super_server": true,
    "s2s_super_server_ip": "127.0.0.1",
    "s2s_super_server_port": 9866
}
```

**Child Server Mode:**
```json
{
    "enable_persistant_relay": true,
    "s2s_relay_to_super_server_ip": "127.0.0.1",
    "s2s_relay_to_super_server_port": 9866
}
```

### S2S Authentication

Server-to-Server authentication using Ed25519 keys:

```json
{
    "s2s_my_private_key": "./ssl_local/<server_id>_private.pem",
    "s2s_trusted_server_keys": {
        "ParentServer": "./ssl_local/ParentServer_public.pem"
    }
}
```

**Note:** Comm servers always attempt authentication when challenged. The accepting server (Auth Server or parent server) controls whether authentication is required via its `s2s_auth_enabled` configuration.

See [wiki/S2SAuthentication.md](wiki/S2SAuthentication.md) for detailed setup.

### SSL/TLS

```json
{
    "enable_SSL": true,
    "ssl_key_file": "./ssl/privkey.pem",
    "ssl_cert_file": "./ssl/fullchain.pem",
    "allow_fake_SSL": true
}
```

### Database

```json
{
    "dbIP": "localhost",
    "dbuser": "USERNAME",
    "dbpassword": "PASSWORD",
    "dbdatabase": "andruav"
}
```

## Running the Server

### Development

```bash
npm start
```

### Production

```bash
# Using deployment scripts
./deployment/run_parent.sh    # For parent server
./deployment/run_slave.sh     # For child server

# Or directly
NODE_ENV=production npm start
```

## Message Propagation

The server implements a mesh relay system for message propagation:

- **Local messages**: From WebSocket clients, forwarded to relay servers
- **External messages**: From relay servers, delivered locally only (no re-forwarding)
- **Loop prevention**: Uses `_path` array to track message traversal

See [wiki/MessagePropagation.md](wiki/MessagePropagation.md) for detailed architecture.

## Message Routing

Messages are routed based on the `ty` (type) and `tg` (target) fields:

### Routing Types

| Type | Constant | Behavior |
|------|----------|----------|
| `'g'` | `CONST_WS_MSG_ROUTING_GROUP` | Broadcast to group |
| `'i'` | `CONST_WS_MSG_ROUTING_INDIVIDUAL` | Targeted delivery |
| `'s'` | `CONST_WS_MSG_ROUTING_SYSTEM` | System commands (local only) |

### Target Values

| Value | Constant | Meaning |
|-------|----------|---------|
| `'_GCS_'` | `CONST_WS_SENDER_ALL_GCS` | All GCS units |
| `'_GD_'` | `CONST_WS_SENDER_ALL` | All units (GCS + Agents) |
| `'_AGN_'` | `CONST_WS_SENDER_ALL_AGENTS` | All drone agents |
| *(other)* | - | Specific unit ID (one-to-one) |

## Testing

```bash
# Run all unit tests
npm test

# Run tests in watch mode
npm run test:watch

# Run specific test file
node --test test/unit/relay.test.js
```

Test files are located in `test/unit/` and use Node's built-in test runner.

## S2S Key Generation

Generate Ed25519 keys for server-to-server authentication:

```bash
cd ../andruav_authenticator/scripts
./gen_s2s_keys.sh <server_id>
```

This creates:
- `<server_id>_private.pem` - Private key (keep secret)
- `<server_id>_public.pem` - Public key (share with accepting servers)

See [wiki/S2SAuthentication.md](wiki/S2SAuthentication.md) for complete guide.

## Deployment Configurations

Example configuration files are provided in the `deployment/` directory:

- `server.s2s.super.config` - Parent server configuration
- `server.s2s.drone.config` - Child/drone server configuration

## Security Best Practices

- Enable SSL/TLS for all WebSocket connections
- Use S2S authentication for server-to-server connections
- Restrict file permissions on private keys (0600)
- Use environment variables for sensitive data
- Keep dependencies updated
- Monitor server logs for suspicious activity
- Implement rate limiting for WebSocket connections

## Troubleshooting

### WebSocket Connection Fails
- Check server is running: `npm start`
- Verify port is not in use
- Check SSL certificate validity
- Verify firewall allows WebSocket connections

### S2S Relay Not Working
- Verify `enable_persistant_relay` or `enable_super_server` is set correctly
- Check parent server IP and port configuration
- Ensure S2S authentication is configured on both ends
- Verify public/private key pairs match

### Messages Not Propagating
- Check `_path` array for loop prevention issues
- Verify routing type (`ty`) and target (`tg`) fields
- Review message propagation logs
- Ensure units are in correct groups

## Documentation

- [Message Propagation](wiki/MessagePropagation.md) - Message routing and relay architecture
- [S2S Authentication](wiki/S2SAuthentication.md) - Server-to-server authentication setup

## Project Structure

```
andruav_server/
├── server/
│   ├── chat_server/          # Message routing logic
│   └── server_to_server/     # S2S communication
├── ssl_local/                # SSL certificates and S2S keys
├── deployment/               # Deployment configurations
├── test/
│   ├── unit/                 # Unit tests
│   └── helpers/              # Test utilities
├── wiki/                     # Documentation
└── server.js                 # Main entry point
```

## Contributing

Contributions are welcome! Please read the contributing guidelines before submitting PRs.

## License

[License information here]

## Support

For support and documentation, please refer to [Cloud.Ardupilot.org](https://cloud.ardupilot.org).

[![Ardupilot Cloud EcoSystem](https://cloud.ardupilot.org/_static/ardupilot_logo.png)](https://cloud.ardupilot.org) **Drone Engage** is part of Ardupilot Cloud Eco System




