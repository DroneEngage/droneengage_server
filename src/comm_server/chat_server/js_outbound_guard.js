"use strict";

/**
 * Outbound backpressure guard.
 *
 * Wraps socket.send() with a per-socket bufferedAmount check so that one
 * slow client cannot make the server queue unlimited data in the Node heap.
 * Messages are classified as:
 *   - 'latest_wins'  : stale telemetry is worthless; skipped when the target
 *                      socket is above the high-water mark.
 *   - 'must_deliver' : commands, acks, errors, system messages; always sent
 *                      until the hard cap.
 * A socket above the hard cap is terminated — the client reconnects fresh.
 *
 * ws gives no access to messages already queued, so the only tools are
 * send / skip / terminate.
 *
 * Config (server.config, "outbound_guard" block; defaults apply if missing):
 *   enabled                    master switch (false restores old behaviour)
 *   high_water_bytes           above this, latest_wins messages are skipped
 *   hard_cap_bytes             above this, the socket is terminated
 *   latest_wins_message_types  numeric mt values classified as latest_wins
 */

const CLASS_LATEST_WINS = 'latest_wins';
const CLASS_MUST_DELIVER = 'must_deliver';

const RESULT_SENT = 'sent';
const RESULT_SKIPPED = 'skipped';
const RESULT_TERMINATED = 'terminated';

let v_cachedConfig = null;


/**
 * Clears the cached config so the next call re-reads
 * global.m_serverconfig. Used by tests.
 */
function fn_reloadConfig() {
    v_cachedConfig = null;
}


/**
 * Reads outbound_guard config once and caches it, expanding
 * latest_wins_message_types into a Set for O(1) lookup in the hot path.
 */
function fn_getConfig() {
    if (v_cachedConfig !== null) return v_cachedConfig;

    const c_cfg = {
        enabled: true,
        high_water_bytes: 262144,
        hard_cap_bytes: 4194304,
        latest_wins_message_types: []
    };

    try {
        const c_user = (global.m_serverconfig != null
            && global.m_serverconfig.m_configuration != null)
            ? global.m_serverconfig.m_configuration.outbound_guard
            : null;
        if ((c_user != null) && (typeof c_user === 'object')) {
            Object.assign(c_cfg, c_user);
        }
    } catch (e) {
        // keep defaults
    }

    c_cfg._latestWinsSet = new Set(
        Array.isArray(c_cfg.latest_wins_message_types) ? c_cfg.latest_wins_message_types : []
    );

    v_cachedConfig = c_cfg;
    return v_cachedConfig;
}


/**
 * Returns the delivery class of a message type. Unknown types are
 * must_deliver — they are never skipped.
 * @param {number|string} p_mt the message 'mt' field
 * @returns {'latest_wins'|'must_deliver'}
 */
function fn_getMessageClass(p_mt) {
    return fn_getConfig()._latestWinsSet.has(p_mt) ? CLASS_LATEST_WINS : CLASS_MUST_DELIVER;
}


/**
 * Sends p_message to p_socket honoring the backpressure bands:
 *   buffered < high_water                 -> send
 *   high_water <= buffered < hard_cap     -> send must_deliver, skip latest_wins
 *   buffered >= hard_cap                  -> terminate socket
 * @param {*} p_socket ws socket (must expose bufferedAmount and terminate)
 * @param {*} p_message payload
 * @param {boolean} p_isBinary
 * @param {string} [p_class] message class; defaults to must_deliver
 * @returns {'sent'|'skipped'|'terminated'}
 */
function fn_safeSend(p_socket, p_message, p_isBinary, p_class) {
    const c_cfg = fn_getConfig();

    if (c_cfg.enabled !== true) {
        p_socket.send(p_message, { binary: p_isBinary });
        return RESULT_SENT;
    }

    const c_buffered = (typeof p_socket.bufferedAmount === 'number') ? p_socket.bufferedAmount : 0;

    if (c_buffered >= c_cfg.hard_cap_bytes) {
        if (p_socket.m_status != null) {
            p_socket.m_status.m_terminated_for_backlog = (p_socket.m_status.m_terminated_for_backlog || 0) + 1;
        }
        try {
            p_socket.terminate();
        } catch (e) {
            // socket may already be dead
        }
        console.log(`outbound_guard: terminated socket '${p_socket.name}' bufferedAmount=${c_buffered}`);
        return RESULT_TERMINATED;
    }

    if ((c_buffered >= c_cfg.high_water_bytes)
        && ((p_class == null ? CLASS_MUST_DELIVER : p_class) === CLASS_LATEST_WINS)) {
        if (p_socket.m_status != null) {
            p_socket.m_status.m_dropped = (p_socket.m_status.m_dropped || 0) + 1;
        }
        return RESULT_SKIPPED;
    }

    p_socket.send(p_message, { binary: p_isBinary });
    return RESULT_SENT;
}


module.exports = {
    CLASS_LATEST_WINS,
    CLASS_MUST_DELIVER,
    RESULT_SENT,
    RESULT_SKIPPED,
    RESULT_TERMINATED,
    fn_reloadConfig,
    fn_getMessageClass,
    fn_safeSend
};
