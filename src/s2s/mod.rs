//! Server-to-server links: AUTH, the storage server, UDP relays and the
//! super-server mesh (src/comm_server/server_to_server/*, js_s2s_auth.js,
//! js_comm_server_manager_client.js).

pub mod auth_client;
pub mod child;
pub mod db_proxy_client;
pub mod parent;
pub mod udp_relay_client;

pub use droneengage_server_common::s2s_auth;
