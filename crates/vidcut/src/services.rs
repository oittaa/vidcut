use serde_json::{Value, json};
use vidcut_core::request::{SpeechModel, SpeechService};
use vidcut_core::{Error, Result};

const KEYCHAIN: &str = "vidcut";

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

pub fn key(model: SpeechModel) -> Result<Option<String>> {
    let Some(service) = of(model) else {
        return Ok(None);
    };
    Ok(from_environment(service).or_else(|| from_keychain(service)))
}

pub fn source(service: SpeechService) -> Option<&'static str> {
    if from_environment(service).is_some() {
        Some("environment")
    } else if from_keychain(service).is_some() {
        Some("keychain")
    } else {
        None
    }
}

pub fn describe() -> Value {
    let one = |service: SpeechService| {
        json!({
            "key": source(service),
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
                "the keychain didn't keep the key ({e}): set {} instead",
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
