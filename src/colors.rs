//! ANSI console colors — the same table as
//! droneengage_server_common/helpers/js_colors.js.

#![allow(dead_code)]

pub const RESET: &str = "\x1b[0m";
pub const BRIGHT: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const UNDERSCORE: &str = "\x1b[4m";
pub const BLINK: &str = "\x1b[5m";
pub const REVERSE: &str = "\x1b[7m";
pub const HIDDEN: &str = "\x1b[8m";

pub const FG_BLACK: &str = "\x1b[30m";
pub const ERROR: &str = "\x1b[31m";
pub const B_ERROR: &str = "\x1b[1;31m";
pub const SUCCESS: &str = "\x1b[32m";
pub const B_SUCCESS: &str = "\x1b[1;32m";
pub const FG_YELLOW: &str = "\x1b[33m";
pub const B_FG_YELLOW: &str = "\x1b[1;33m";
pub const LOG: &str = "\x1b[34m";
pub const FG_MAGENTA: &str = "\x1b[35m";
pub const FG_CYAN: &str = "\x1b[36m";
pub const FG_WHITE: &str = "\x1b[37m";
pub const INFO: &str = "\x1b[33m";
pub const BG_BLACK: &str = "\x1b[40m";
pub const BG_RED: &str = "\x1b[41m";
pub const BG_GREEN: &str = "\x1b[42m";
pub const BG_YELLOW: &str = "\x1b[43m";
pub const BG_BLUE: &str = "\x1b[44m";
pub const BG_MAGENTA: &str = "\x1b[45m";
pub const BG_CYAN: &str = "\x1b[46m";
pub const BG_WHITE: &str = "\x1b[47m";
