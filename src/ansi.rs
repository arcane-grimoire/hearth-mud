pub const RESET: &str = "[/]";
pub const BOLD: &str = "[b]";
pub const DIM: &str = "[dim]";

pub const RED: &str = "[red]";
pub const GREEN: &str = "[green]";
pub const YELLOW: &str = "[yellow]";
pub const BLUE: &str = "[blue]";
pub const MAGENTA: &str = "[magenta]";
pub const CYAN: &str = "[cyan]";
pub const WHITE: &str = "[white]";

pub fn room_title(name: &str) -> String {
    format!("[b][cyan]{name}[/]")
}

pub fn exit_list(exits: &[&str]) -> String {
    let linked: Vec<String> = exits
        .iter()
        .map(|e| format!("[cmd={e}][green]{e}[/green][/cmd]"))
        .collect();
    format!("[dim][Exits: {}][/dim]", linked.join(", "))
}

pub fn player_name(name: &str) -> String {
    format!("[b][white]{name}[/]")
}

pub fn system_msg(msg: &str) -> String {
    format!("[yellow]{msg}[/]")
}

pub fn error_msg(msg: &str) -> String {
    format!("[red]{msg}[/]")
}

pub fn admin_msg(msg: &str) -> String {
    format!("[b][magenta]{msg}[/]")
}
