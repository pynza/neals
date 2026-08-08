use comfy_table::presets::UTF8_FULL;
use comfy_table::{Attribute, Cell, Color as TableColor, ContentArrangement, Table};
use owo_colors::OwoColorize;
use std::io::IsTerminal;

pub fn use_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if matches!(
        std::env::var("CLICOLOR").as_deref(),
        Ok("0")
    ) {
        return false;
    }
    std::io::stdout().is_terminal() || std::io::stderr().is_terminal()
}

pub fn new_table() -> Table {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table
}

pub fn header_cell(text: &str) -> Cell {
    let cell = Cell::new(text);
    if use_color() {
        cell.add_attribute(Attribute::Bold)
            .fg(TableColor::Cyan)
    } else {
        cell
    }
}

pub fn status_ok(text: &str) -> Cell {
    colored_cell(text, TableColor::Green)
}

pub fn status_warn(text: &str) -> Cell {
    colored_cell(text, TableColor::Yellow)
}

fn colored_cell(text: &str, color: TableColor) -> Cell {
    let cell = Cell::new(text);
    if use_color() {
        cell.fg(color)
    } else {
        cell
    }
}

pub fn print_ok(msg: &str) {
    if use_color() {
        println!("{} {msg}", "✔".green());
    } else {
        println!("ok: {msg}");
    }
}

pub fn print_warn(msg: &str) {
    if use_color() {
        eprintln!("{} {msg}", "⚠".yellow());
    } else {
        eprintln!("warning: {msg}");
    }
}

pub fn print_err(msg: &str) {
    if use_color() {
        eprintln!("{} {msg}", "✖".red());
    } else {
        eprintln!("error: {msg}");
    }
}

pub fn print_dim(msg: &str) {
    if use_color() {
        println!("{}", msg.dimmed());
    } else {
        println!("{msg}");
    }
}

pub fn eprint_dim(msg: &str) {
    if use_color() {
        eprintln!("{}", msg.dimmed());
    } else {
        eprintln!("{msg}");
    }
}

pub fn accent(msg: &str) -> String {
    if use_color() {
        msg.cyan().to_string()
    } else {
        msg.to_string()
    }
}

pub fn format_mark(ok: bool, required: bool) -> String {
    if ok {
        if use_color() {
            "ok".green().to_string()
        } else {
            "ok".into()
        }
    } else if required {
        if use_color() {
            "FAIL".red().bold().to_string()
        } else {
            "FAIL".into()
        }
    } else if use_color() {
        "WARN".yellow().to_string()
    } else {
        "WARN".into()
    }
}
