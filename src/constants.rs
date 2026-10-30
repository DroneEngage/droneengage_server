//! Message and protocol constants (js_constants.js, js_andruavMessages.js).

#![allow(dead_code)]

// HTTP header fields
pub const CONST_ACCESSCODE_PARAMETER: &str = "ac";
pub const CONST_DEVICE_FINGERPRINT: &str = "fp";
pub const CONST_SESSION_ID_PARAMETER: &str = "s";
pub const CONST_PARTY_ID_PARAMETER: &str = "p";
pub const CONST_APK_FINGERPRINT_PARAMETER: &str = "ak";
pub const CONST_ACTOR_TYPE: &str = "at";
pub const CONST_PERMISSION2: &str = "prm";

// AUTH <--> COMM server commands
pub const CONST_CS_CMD_INFO: &str = "a";
pub const CONST_CS_CMD_LOGIN_REQUEST: &str = "b";
pub const CONST_CS_CMD_LOGOUT_REQUEST: &str = "c";
pub const CONST_CS_CMD_STORAGE_STATUS: &str = "d";
pub const CONST_CS_CMD_QUERY_UDP_PROXIES: &str = "e";
pub const CONST_CS_CMD_REPORT_UDP_PROXIES: &str = "f";
pub const CONST_CS_CMD_SET_UDP_PROXY_STATE: &str = "g";
pub const CONST_CS_CMD_SET_UNIT_TRAFFIC_STATE: &str = "h";
pub const CONST_CS_CMD_REPORT_BLOCKED_UNITS: &str = "i";
pub const CONST_CS_CMD_UDP_RELAY_STATUS: &str = "j";

pub const CONST_CS_ACCOUNT_ID: &str = "a";
pub const CONST_CS_GROUP_ID: &str = "b";
pub const CONST_CS_SENDER_ID: &str = "s";
pub const CONST_CS_LOGIN_TEMP_KEY: &str = "f";
pub const CONST_CS_ERROR: &str = "e";
pub const CONST_CS_SERVER_PUBLIC_HOST: &str = "g";
pub const CONST_CS_SERVER_PORT: &str = "h";
pub const CONST_CS_REQUEST_ID: &str = "r";

pub const CONST_WS_SENDER_ID: &str = "sd";
pub const CONST_WS_PARTY_ID: &str = "pd";
pub const CONST_WS_TARGET_ID: &str = "tg";
pub const CONST_WS_PAYLOAD: &str = "ms";
pub const CONST_WS_MESSAGE_ID: &str = "mt";
pub const CONST_WS_MSG_ROUTING: &str = "ty";
pub const CONST_WS_MSG_ROUTING_GROUP: &str = "g";
pub const CONST_WS_MSG_ROUTING_INDIVIDUAL: &str = "i";
pub const CONST_WS_MSG_ROUTING_SYSTEM: &str = "s";

pub const CONST_ACTOR_TYPE_GCS: &str = "g";
pub const CONST_ACTOR_TYPE_DRONE: &str = "d";

pub const CONST_WS_SENDER_COMM_SERVER: &str = "_SYS_";
pub const CONST_WS_SENDER_ALL_GCS: &str = "_GCS_";
pub const CONST_WS_SENDER_ALL: &str = "_GD_";
pub const CONST_WS_SENDER_ALL_AGENTS: &str = "_AGN_";

// error numbers
pub const CONST_ERROR_NON: i64 = 0;
pub const CONST_ERROR_INVALID_DATA: i64 = 1;
pub const CONST_ERROR_DATA_NOT_FOUND: i64 = 2;
pub const CONST_ERROR_DATA_DATABASE_ERROR: i64 = 3;
pub const CONST_ERROR_SERVER_NOT_AVAILABLE: i64 = 4;
pub const CONST_ERROR_DATA_UNKNOWN_ERROR: i64 = 999;

// system messages
pub const CONST_TYPE_ANDRUAV_SYSTEM_LOAD_TASKS: i64 = 9001;
pub const CONST_TYPE_ANDRUAV_SYSTEM_SAVE_TASKS: i64 = 9002;
pub const CONST_TYPE_ANDRUAV_SYSTEM_DELETE_TASKS: i64 = 9003;
pub const CONST_TYPE_ANDRUAV_SYSTEM_DISABLE_TASKS: i64 = 9004;
pub const CONST_TYPE_ANDRUAV_SYSTEM_PING: i64 = 9005;
pub const CONST_TYPE_ANDRUAV_SYSTEM_LOGOUT_COMM_SERVER: i64 = 9006;
pub const CONST_TYPE_ANDRUAV_SYSTEM_CONNECTED_COMM_SERVER: i64 = 9007;
pub const CONST_TYPE_ANDRUAV_SYSTEM_UDP_PROXY: i64 = 9008;
pub const CONST_TYPE_ANDRUAV_SYSTEM_UNIT_ONLINE: i64 = 9009;
pub const CONST_TYPE_ANDRUAV_SYSTEM_LOAD_MISSION: i64 = 9010;
pub const CONST_TYPE_ANDRUAV_SYSTEM_SAVE_MISSION: i64 = 9011;
pub const CONST_TYPE_ANDRUAV_SYSTEM_DELETE_MISSION: i64 = 9012;
pub const CONST_TYPE_ANDRUAV_SYSTEM_QUERY_SERVER: i64 = 9013;
pub const CONST_TYPE_ANDRUAV_SYSTEM_STATE_SERVER: i64 = 9014;
pub const CONST_TYPE_ANDRUAV_SYSTEM_LOAD_NEWS: i64 = 9015;
pub const CONST_TYPE_ANDRUAV_SYSTEM_SAVE_NEWS: i64 = 9016;
pub const CONST_TYPE_ANDRUAV_SYSTEM_DELETE_NEWS: i64 = 9017;
pub const CONST_TYPE_ANDRUAV_SYSTEM_NEWS_PUSH: i64 = 9018;
pub const CONST_TYPE_ANDRUAV_SYSTEM_AIS_SUB: i64 = 9019;
pub const CONST_TYPE_ANDRUAV_SYSTEM_AIS_PUSH: i64 = 9020;
pub const CONST_TYPE_ANDRUAV_SYSTEM_UDP_PROXY_PEER: i64 = 9021;

pub const CONST_NEWS_SCOPE_GLOBAL: &str = "global";
pub const CONST_NEWS_SCOPE_ACCOUNT: &str = "account";

pub const CONST_TYPE_ANDRUAV_SYSTEM_QUERY_SERVER_SUB_CMD_IS_STORAGE_SERVER_CONNECTED: i64 = 1;

// storage / relay link state codes
pub const CONST_STORAGE_STATE_DISCONNECTED: &str = "dc";
pub const CONST_STORAGE_STATE_CONNECTING: &str = "cg";
pub const CONST_STORAGE_STATE_CONNECTED: &str = "cd";
pub const CONST_STORAGE_STATE_UNHEALTHY: &str = "uh";
pub const CONST_STORAGE_STATE_ERROR: &str = "er";

/// de_mavlink requires all of these for its UDP-proxy commands
pub const CONST_PERMISSION_ALLOW_GCS_FULL_CONTROL: i64 = 0x0000_0f00;

pub const CONST_ACCESSCODE_MAX_LENGTH: usize = 200;
pub const CONST_FINGERPRINT_MAX_LENGTH: usize = 200;
pub const CONST_SESSION_MAX_LENGTH: usize = 200;
pub const CONST_SENDERID_MAX_LENGTH: usize = 200;

// Andruav message types (js_andruavMessages.js) used by the server
pub const CONST_TYPE_ANDRUAV_MESSAGE_ID: i64 = 1004;
pub const CONST_TYPE_ANDRUAV_MESSAGE_REMOTE_EXECUTE: i64 = 1005;
pub const CONST_TYPE_ANDRUAV_MESSAGE_IMG: i64 = 1006;
pub const CONST_TYPE_ANDRUAV_MESSAGE_GPS: i64 = 1002;
pub const CONST_TYPE_ANDRUAV_BINARY_MESSAGE_MAVLINK: i64 = 6502;
