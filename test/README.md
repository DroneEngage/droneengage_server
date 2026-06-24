# Andruav Comm Server — Test Suite

Automated tests for the communication server. Uses Node's **built-in** test runner
(`node:test` + `node:assert`) — no extra dependencies.

## Running

```bash
npm test            # run all unit tests once
npm run test:watch  # re-run on file changes
```

Requires Node >= 18 (the project already targets Node 18+ via `ws`).

## Layout

```
test/
  helpers/
    test_globals.js     # installs fake global.* state, captures relay output,
                        # builds fake WebSocket clients and binary frames
  unit/
    relay.test.js       # js_chat_relay.js  (mesh forwarding, _path/_gid/_aid)
    tasks.test.js       # js_chat_tasks.js  (de-duplicated task helpers + guards)
  test_connect_as_unit.js              # legacy MANUAL integration script
  test_fully_connect_to_comm_server.js # legacy MANUAL integration script
```

The two `test_*.js` files in the root of `test/` are **manual** scripts that
connect to a live server. They are not part of `npm test` (which only runs
`test/unit/**/*.test.js`).

## Why globals are faked

The server reads runtime state from `global.*` (`m_serverconfig`, the relay
channel singletons, `m_logger`). `test/helpers/test_globals.js` installs fakes
before each test and restores them afterwards, so pure logic can be exercised
without a live server or database. As the codebase moves to dependency
injection (Phase 2 of the refactor), these helpers can shrink or be replaced by
direct constructor/config injection.

## Conventions

- One `*.test.js` file per source module under `test/unit/`.
- Always `restoreGlobals()` in `test.afterEach()` when you call
  `installFakeGlobals()`.
- Prefix tests that lock in a known/legacy behavior (rather than desired
  behavior) with `[characterization]` and reference the follow-up.

## Coverage status & next targets

| Module | Status |
|--------|--------|
| `js_chat_relay.js`   | ✅ covered (forwarding, loop trail, payload preservation) |
| `js_chat_tasks.js`   | ✅ de-dup helpers + handler guards covered |
| `js_chat_routing.js` | ⬜ TODO — `fn_parseExternalMessage` loop-drop, broadcast scoping, one-to-one no-forward |
| `js_chat_system_commands.js` | ⬜ TODO — ping reply, logout, udp-proxy dispatch (stub `js_udp_proxy`) |
| `js_chat_connection.js` | ⬜ TODO — key validation accept/reject paths |
| End-to-end (WS handshake → routing) | ⬜ TODO — integration test booting the server on an ephemeral port |

### Extending to `routing` / `system-command` / `connection`

These modules `require()` other project modules at load time
(`js_andruav_chat_account_rooms`, `pluginManager`, `js_udp_proxy`, etc.). To
unit-test them in isolation, inject seams during Phase 2 (DI), or stub the
required modules via the Node `require` cache before requiring the module under
test. Prefer adding the DI seam over cache hacks where practical.
