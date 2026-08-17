pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";

pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const CYAN: &str = "\x1b[36m";
pub const WHITE: &str = "\x1b[37m";

pub fn room_title(name: &str) -> String {
    format!("{BOLD}{CYAN}{name}{RESET}")
}

pub fn exit_list(exits: &str) -> String {
    format!("{DIM}[Exits: {GREEN}{exits}{RESET}{DIM}]{RESET}")
}

pub fn player_name(name: &str) -> String {
    format!("{BOLD}{WHITE}{name}{RESET}")
}

pub fn system_msg(msg: &str) -> String {
    format!("{YELLOW}{msg}{RESET}")
}

pub fn error_msg(msg: &str) -> String {
    format!("{RED}{msg}{RESET}")
}

pub fn admin_msg(msg: &str) -> String {
    format!("{BOLD}{MAGENTA}{msg}{RESET}")
}
