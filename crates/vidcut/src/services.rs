use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use serde_json::{Value, json};
use vidcut_core::preferences::Services;
use vidcut_core::request::{SpeechModel, SpeechService};
use vidcut_core::{Error, Result};

const KEYCHAIN: &str = "vidcut";

static COMMANDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn variable(service: SpeechService) -> &'static str {
    match service {
        SpeechService::Xai => "XAI_API_KEY",
        SpeechService::Google => "GEMINI_API_KEY",
    }
}

fn account(service: SpeechService) -> &'static str {
    match service {
        SpeechService::Xai => "xai",
        SpeechService::Google => "google",
    }
}

fn command(service: SpeechService, s: &Services) -> &str {
    match service {
        SpeechService::Xai => &s.xai_key_command,
        SpeechService::Google => &s.google_key_command,
    }
}

pub fn of(model: SpeechModel) -> Option<SpeechService> {
    match model {
        SpeechModel::Xai => Some(SpeechService::Xai),
        SpeechModel::Google => Some(SpeechService::Google),
        _ => None,
    }
}

fn entry(service: SpeechService) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN, account(service))
        .map_err(|e| Error::internal(format!("the keychain: {e}")))
}

fn from_environment(service: SpeechService) -> Option<String> {
    std::env::var(variable(service))
        .ok()
        .filter(|k| !k.trim().is_empty())
}

fn from_keychain(service: SpeechService) -> Option<String> {
    entry(service).ok()?.get_password().ok()
}

fn run(cmd: &str) -> Result<String> {
    if let Some(k) = COMMANDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|m| m.get(cmd))
    {
        return Ok(k.clone());
    }
    let out = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", cmd]).output()
    } else {
        std::process::Command::new("sh").args(["-c", cmd]).output()
    }
    .map_err(|e| Error::internal(format!("the key command: {e}")))?;
    let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || key.is_empty() {
        return Err(Error::invalid(format!(
            "the key command gave no key ({})",
            out.status
        )));
    }
    COMMANDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(cmd.to_string(), key.clone());
    Ok(key)
}

pub fn key(model: SpeechModel) -> Result<Option<String>> {
    let Some(service) = of(model) else {
        return Ok(None);
    };
    if let Some(k) = from_environment(service) {
        return Ok(Some(k));
    }
    let prefs = crate::preferences::load().unwrap_or_default();
    let cmd = command(service, &prefs.services);
    if !cmd.is_empty() {
        return run(cmd).map(Some);
    }
    Ok(from_keychain(service))
}

pub fn source(service: SpeechService, s: &Services) -> Option<&'static str> {
    if from_environment(service).is_some() {
        Some("environment")
    } else if !command(service, s).is_empty() {
        Some("command")
    } else if from_keychain(service).is_some() {
        Some("keychain")
    } else {
        None
    }
}

pub fn describe(s: &Services) -> Value {
    let one = |service: SpeechService| {
        json!({
            "key": source(service, s),
            "variable": variable(service),
            "keychain": keyring::Entry::store_status().is_ok(),
        })
    };
    json!({ "xai": one(SpeechService::Xai), "google": one(SpeechService::Google) })
}

pub fn set_key(service: SpeechService, key: Option<&str>, dry_run: bool) -> Result<Value> {
    let key = key.map(str::trim).unwrap_or_default();
    if key.is_empty() {
        return Err(Error::usage(
            "give the key (the CLI reads it from standard input)",
        ));
    }
    if !dry_run {
        entry(service)?.set_password(key).map_err(|e| {
            Error::invalid(format!(
                "the keychain didn't keep the key ({e}): set {} or a key command instead",
                variable(service)
            ))
        })?;
    }
    Ok(json!({ "service": service, "key": "keychain" }))
}

pub fn remove_key(service: SpeechService, dry_run: bool) -> Result<Value> {
    if !dry_run {
        match entry(service)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(Error::invalid(format!("the keychain: {e}"))),
        }
    }
    Ok(json!({ "service": service, "key": Value::Null }))
}
