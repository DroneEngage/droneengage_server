"use strict";

/**
 * Server-to-Server (S2S) authentication helper.
 *
 * Secures S2S WebSocket channels using an Ed25519 public/private key pair with a
 * simple challenge-response handshake:
 *   1- The ACCEPTING side (holds the PUBLIC key) sends a random nonce challenge.
 *   2- The CONNECTING side (holds the PRIVATE key) signs the nonce and replies.
 *   3- The ACCEPTING side verifies the signature before trusting the peer.
 *
 * No third-party dependency is used (Node built-in 'crypto').
 *
 * Config keys (in server config file):
 *   s2s_auth_enabled       : true/false   - enable the handshake on this node (Auth Server only).
 *   s2s_my_private_key     : PEM path     - used when this node CONNECTS out.
 *   s2s_trusted_server_keys: object       - mapping of server_id to public key file paths (when ACCEPTING peers).
 *                                   Example: { "child1": "./ssl/child1_public.pem", "child2": "./ssl/child2_public.pem" }
 *
 * Key file paths are resolved relative to the project root (one level up from comm_server/ directory).
 */

const c_crypto = require('crypto');
const c_path = require('path');
const c_fs = require('fs');

const CONST_S2S_AUTH_CHALLENGE = 'challenge';
const CONST_S2S_AUTH_RESPONSE = 'response';
const CONST_S2S_AUTH_HANDSHAKE_TIMEOUT = 8000; // ms to complete handshake before drop.

// Auth methods carried in the challenge envelope 'method' field.
const CONST_S2S_AUTH_METHOD_ED25519 = 'ed25519';
const CONST_S2S_AUTH_METHOD_HMAC = 'hmac-sha256';

// Env var alternative to s2s_shared_secret_file.
const CONST_S2S_SHARED_SECRET_ENV = 'de_s2s_shared_secret';
const CONST_S2S_SHARED_SECRET_MIN_LENGTH = 16;

let m_privateKey = null;
let m_publicKeys = null;
let m_sharedSecret = null;


function fn_resolvePath(p_path) {
    // Resolve relative to project root, not this module's directory
    const c_projectRoot = c_path.resolve(__dirname, '..', '..');
    return c_path.resolve(c_projectRoot, p_path);
}


/**
 * Lazy-loads and caches the Ed25519 PRIVATE key used to sign challenges.
 */
function fn_getPrivateKey() {
    if (m_privateKey != null) return m_privateKey;

    const c_file = global.m_serverconfig.m_configuration.s2s_my_private_key;
    if (c_file == null) {
        throw new Error("s2s_my_private_key is not configured");
    }

    const c_pem = c_fs.readFileSync(fn_resolvePath(c_file));
    m_privateKey = c_crypto.createPrivateKey(c_pem);

    return m_privateKey;
}


/**
 * Lazy-loads and caches the Ed25519 PUBLIC keys used to verify responses.
 * Returns an object mapping server_id to crypto PublicKey objects.
 */
function fn_getPublicKeys() {
    if (m_publicKeys != null) return m_publicKeys;

    const c_keys = global.m_serverconfig.m_configuration.s2s_trusted_server_keys;
    if (c_keys == null) {
        throw new Error("s2s_trusted_server_keys is not configured");
    }

    m_publicKeys = {};
    for (const v_serverId in c_keys) {
        const c_file = c_keys[v_serverId];
        const c_pem = c_fs.readFileSync(fn_resolvePath(c_file));
        m_publicKeys[v_serverId] = c_crypto.createPublicKey(c_pem);
    }

    return m_publicKeys;
}

/**
 * Resets the lazy-loaded key/secret caches. Used by unit tests to avoid
 * cross-test pollution when the global config changes.
 */
function fn_resetKeyCache() {
    m_privateKey = null;
    m_publicKeys = null;
    m_sharedSecret = null;
}


/**
 * True when the string looks like a shipped placeholder rather than a real
 * secret. A placeholder value in config must fail closed — a shipped default
 * is a public credential.
 */
function fn_isPlaceholderSecret(p_value) {
    if (p_value == null) return true;
    const c_v = String(p_value).trim().toLowerCase();
    if (c_v.length === 0) return true;
    const c_placeholders = [
        'change_me', 'changeme', 'change-me', 'change this',
        'placeholder', 'your-secret', 'your_secret', 'todo', 'secret', 'xxx'
    ];
    for (let i = 0; i < c_placeholders.length; ++i) {
        if (c_v === c_placeholders[i]) return true;
    }
    return false;
}


/**
 * True when a shared-secret source is configured (file path or env var),
 * without loading or validating it.
 */
function fn_hasSharedSecret() {
    if (process.env[CONST_S2S_SHARED_SECRET_ENV] != null) return true;
    return global.m_serverconfig.m_configuration.s2s_shared_secret_file != null;
}


/**
 * Lazy-loads and caches the shared secret used in HMAC mode
 * (s2s_cert_enabled == false on the accepting side). The secret is never
 * stored in server.config: it comes from the de_s2s_shared_secret env var or
 * from the file named by s2s_shared_secret_file (chmod 600). When
 * p_autoGenerate is true a missing secret file is created with a random
 * 32-byte hex secret on first boot. Throws on missing/placeholder/weak
 * secrets — callers must fail closed.
 * @param {boolean} p_autoGenerate generate+write the secret file when absent
 * @returns {string} the shared secret
 */
function fn_getSharedSecret(p_autoGenerate) {
    if (m_sharedSecret != null) return m_sharedSecret;

    let c_secret = null;

    const c_envSecret = process.env[CONST_S2S_SHARED_SECRET_ENV];
    if (c_envSecret != null) {
        c_secret = String(c_envSecret).trim();
    }
    else {
        const c_file = global.m_serverconfig.m_configuration.s2s_shared_secret_file;
        if (c_file == null) {
            throw new Error(
                `S2S shared-secret mode requested but no secret is configured. ` +
                `Set ${CONST_S2S_SHARED_SECRET_ENV} or s2s_shared_secret_file.`);
        }
        const c_path = fn_resolvePath(c_file);
        if (c_fs.existsSync(c_path)) {
            c_secret = c_fs.readFileSync(c_path, 'utf8').trim();
        }
        else if (p_autoGenerate === true) {
            c_secret = c_crypto.randomBytes(32).toString('hex');
            c_fs.writeFileSync(c_path, c_secret + '\n', { mode: 0o600 });
            console.log("S2S shared secret auto-generated at " + c_path);
        }
        else {
            throw new Error("S2S shared secret file not found: " + c_path);
        }
    }

    if (fn_isPlaceholderSecret(c_secret)) {
        throw new Error("S2S shared secret is missing or a placeholder value — refusing to run insecurely (fail closed)");
    }
    if (c_secret.length < CONST_S2S_SHARED_SECRET_MIN_LENGTH) {
        throw new Error("S2S shared secret too short (min " + CONST_S2S_SHARED_SECRET_MIN_LENGTH + " chars) — refusing to run insecurely (fail closed)");
    }

    m_sharedSecret = c_secret;
    return m_sharedSecret;
}


/**
 * Answers a nonce challenge with HMAC-SHA256(nonce, shared_secret).
 * The secret itself is never sent on the wire.
 * @param {string} p_nonce
 * @returns {string} base64 HMAC
 */
function fn_hmacSign(p_nonce) {
    return c_crypto.createHmac('sha256', fn_getSharedSecret(false)).update(String(p_nonce)).digest('base64');
}


/**
 * Verifies a base64 HMAC-SHA256 response against the nonce in constant time.
 * @param {string} p_nonce
 * @param {string} p_hmac base64 HMAC-SHA256 of the nonce
 * @returns {boolean}
 */
function fn_verifyHmac(p_nonce, p_hmac) {
    try {
        if ((p_nonce == null) || (p_hmac == null)) return false;
        const c_expected = Buffer.from(fn_hmacSign(p_nonce), 'utf8');
        const c_actual = Buffer.from(String(p_hmac), 'utf8');
        if (c_expected.length !== c_actual.length) return false;
        return c_crypto.timingSafeEqual(c_expected, c_actual);
    }
    catch (ex) {
        console.log(global.Colors.Error + "S2S HMAC verify error: " + ex + global.Colors.Reset);
        return false;
    }
}


/**
 * Generates a random nonce (challenge).
 */
function fn_generateNonce() {
    return c_crypto.randomBytes(32).toString('hex');
}


/**
 * Signs a nonce with the private key. Ed25519 uses a null algorithm.
 * @param {string} p_nonce
 * @returns {string} base64 signature
 */
function fn_sign(p_nonce) {
    return c_crypto.sign(null, Buffer.from(p_nonce), fn_getPrivateKey()).toString('base64');
}


/**
 * Verifies a base64 signature against a nonce using the server-specific public key.
 * @param {string} p_nonce
 * @param {string} p_signature base64
 * @param {string} p_serverId the server ID to look up the public key for
 * @returns {boolean}
 */
function fn_verify(p_nonce, p_signature, p_serverId) {
    try {
        if ((p_nonce == null) || (p_signature == null) || (p_serverId == null)) return false;
        const c_keys = fn_getPublicKeys();
        const c_publicKey = c_keys[p_serverId];
        if (c_publicKey == null) {
            console.log(global.Colors.Error + "S2S verify error: unknown server_id " + p_serverId + global.Colors.Reset);
            return false;
        }
        return c_crypto.verify(null, Buffer.from(p_nonce), c_publicKey, Buffer.from(p_signature, 'base64'));
    }
    catch (ex) {
        console.log(global.Colors.Error + "S2S verify error: " + ex + global.Colors.Reset);
        return false;
    }
}


/**
 * Builds the challenge envelope (sent by the accepting side).
 * @param {string} p_nonce
 * @param {string} p_method CONST_S2S_AUTH_METHOD_* — tells the peer which
 *        credential to answer with (Ed25519 signature or shared-secret HMAC).
 */
function fn_buildChallenge(p_nonce, p_method) {
    const v_obj = { s2s_auth: CONST_S2S_AUTH_CHALLENGE, nonce: p_nonce };
    if (p_method != null) v_obj.method = p_method;
    return JSON.stringify(v_obj);
}


/**
 * Builds the signed response envelope (sent by the connecting side).
 * @param {string} p_nonce nonce received in the challenge
 * @param {string} p_id this server's id (informational)
 */
function fn_buildResponse(p_nonce, p_id) {
    return JSON.stringify({ s2s_auth: CONST_S2S_AUTH_RESPONSE, sig: fn_sign(p_nonce), id: p_id });
}


/**
 * Builds the HMAC response envelope for shared-secret mode
 * (sent by the connecting side).
 * @param {string} p_nonce nonce received in the challenge
 * @param {string} p_id this server's id (informational)
 */
function fn_buildHmacResponse(p_nonce, p_id) {
    return JSON.stringify({ s2s_auth: CONST_S2S_AUTH_RESPONSE, sig: fn_hmacSign(p_nonce), id: p_id });
}


/**
 * Parses a raw WS message (string or Buffer) into an S2S auth envelope.
 * Returns null if the message is not an S2S auth message (e.g. normal traffic).
 * @param {string|Buffer} p_msg
 */
function fn_parseEnvelope(p_msg) {
    try {
        if (Buffer.isBuffer(p_msg)) {
            // binary chat payloads are never auth envelopes.
            if ((p_msg.length === 0) || (p_msg[0] !== 0x7b /* '{' */)) return null;
            p_msg = p_msg.toString('utf8');
        }
        if (typeof p_msg !== 'string') return null;
        const v_obj = JSON.parse(p_msg);
        if ((v_obj == null) || (v_obj.s2s_auth == null)) return null;
        return v_obj;
    }
    catch {
        return null;
    }
}


module.exports = {
    CONST_S2S_AUTH_CHALLENGE,
    CONST_S2S_AUTH_RESPONSE,
    CONST_S2S_AUTH_HANDSHAKE_TIMEOUT,
    CONST_S2S_AUTH_METHOD_ED25519,
    CONST_S2S_AUTH_METHOD_HMAC,
    CONST_S2S_SHARED_SECRET_ENV,
    fn_generateNonce,
    fn_getPrivateKey,
    fn_getPublicKeys,
    fn_getSharedSecret,
    fn_hasSharedSecret,
    fn_resetKeyCache,
    fn_sign,
    fn_verify,
    fn_hmacSign,
    fn_verifyHmac,
    fn_buildChallenge,
    fn_buildResponse,
    fn_buildHmacResponse,
    fn_parseEnvelope
};
