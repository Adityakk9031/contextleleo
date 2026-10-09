//! `contextleleo setup`: the optional step that turns on the two Jev-powered
//! features. Everything else in contextleleo works without an account, so this
//! asks first, says what leaves the machine, and stores the key privately.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::process::ExitCode;

use contextleleo::jev_config::{self, KeySource};

const KEY_URL: &str = "https://docs.typesafe.ai/introduction/quickstart";

/// Whether a person can answer a prompt: stdin and stderr are both terminals.
pub(crate) fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// `setup`, `setup --status`, `setup --remove`.
pub(crate) fn cmd_setup(status: bool, remove: bool) -> Result<ExitCode, String> {
    let file = jev_config::path()
        .ok_or("no home directory found; export JEV_API_KEY in your shell instead")?;
    if status {
        print_status(&file);
        return Ok(ExitCode::SUCCESS);
    }
    if remove {
        let removed = jev_config::remove_key_from(&file)
            .map_err(|e| format!("updating {}: {e}", file.display()))?;
        println!(
            "{}",
            if removed {
                format!("Removed the stored Jev key from {}.", file.display())
            } else {
                "No stored Jev key to remove.".to_string()
            }
        );
        return Ok(ExitCode::SUCCESS);
    }
    if interactive() {
        explain();
        if !confirm("Turn these on now?")? {
            eprintln!(
                "\nOK — contextleleo keeps working as a session converter and viewer. \
                 Run `contextleleo setup` any time to enable retrieval."
            );
            return Ok(ExitCode::SUCCESS);
        }
    }
    store_key(&file)?;
    Ok(ExitCode::SUCCESS)
}

/// Called when a Jev command finds no key. On a terminal, offers to set one
/// up right there; returns whether a key is now stored. Off a terminal it
/// does nothing, so scripts get the plain configuration error.
pub(crate) fn offer() -> Result<bool, String> {
    if !interactive() {
        return Ok(false);
    }
    let Some(file) = jev_config::path() else {
        return Ok(false);
    };
    explain();
    if !confirm("Set up a Jev API key now?")? {
        return Ok(false);
    }
    store_key(&file)?;
    Ok(true)
}

/// A one-time pointer for a person who has never been told about `setup`:
/// printed to stderr on the first interactive run when no key exists.
pub(crate) fn first_run_hint() {
    if !interactive() || jev_config::key_source() != KeySource::None {
        return;
    }
    let Some(marker) = jev_config::path().and_then(|p| p.parent().map(|d| d.join(".setup-hint")))
    else {
        return;
    };
    if marker.exists() {
        return;
    }
    eprintln!(
        "note: session listing, viewing, searching and converting need no account. \
         Retrieval and smart trimming are optional and need a Jev API key — \
         run `contextleleo setup` to turn them on (shown once)."
    );
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(marker, "");
}

fn explain() {
    eprintln!(
        "\ncontextleleo works without any account: list, view, search, export, and carry \
         sessions between agents.\n\n\
         Two optional features use the Jev API (TypeSafe) and need a key:\n  \
         • retrieval       — find the relevant history across all your past sessions\n  \
         • smart trimming  — Jev scores each message to keep, shrink, or drop it\n\n\
         They send short excerpts (credentials removed, up to 800 characters each) to \
         api.typesafe.ai. Full sessions are never uploaded.\n"
    );
}

fn confirm(question: &str) -> Result<bool, String> {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| format!("reading your answer: {e}"))?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn store_key(file: &std::path::Path) -> Result<(), String> {
    let key = read_key()?;
    jev_config::save_key_to(file, &key).map_err(|e| format!("saving the key: {e}"))?;
    eprintln!(
        "Saved to {} (readable by you only). Retrieval and smart trimming are on.\n\
         Remove it any time with `contextleleo setup --remove`.",
        file.display()
    );
    Ok(())
}

/// Read the key from the terminal with echo off, or from stdin when piped
/// (`echo $KEY | contextleleo setup`), so it never lands in shell history.
fn read_key() -> Result<String, String> {
    let on_terminal = std::io::stdin().is_terminal();
    if on_terminal {
        eprintln!("Paste your Jev API key (input is hidden; get one at {KEY_URL}):");
        eprint!("> ");
        let _ = std::io::stderr().flush();
    }
    let hidden = on_terminal && set_echo(false);
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    if hidden {
        set_echo(true);
        eprintln!();
    }
    read.map_err(|e| format!("reading the key: {e}"))?;
    let key = line.trim().to_string();
    if key.is_empty() {
        return Err(format!("no key entered (get one at {KEY_URL})"));
    }
    Ok(key)
}

/// Turn terminal echo on or off through `stty`; false when that is not
/// possible (the key is then visible as typed).
fn set_echo(on: bool) -> bool {
    cfg!(unix)
        && std::process::Command::new("stty")
            .arg(if on { "echo" } else { "-echo" })
            .stdin(std::process::Stdio::inherit())
            .status()
            .is_ok_and(|status| status.success())
}

fn print_status(file: &std::path::Path) {
    match jev_config::key_source_in(Some(file)) {
        KeySource::Env(name) => println!(
            "Jev key: set in the environment ({name}). Retrieval and smart trimming are on."
        ),
        KeySource::File(path) => println!(
            "Jev key: stored in {}. Retrieval and smart trimming are on.",
            path.display()
        ),
        KeySource::None => println!(
            "Jev key: not set. Session listing, viewing, searching and converting work; \
             retrieval and smart trimming are off. Run `contextleleo setup`."
        ),
    }
}
