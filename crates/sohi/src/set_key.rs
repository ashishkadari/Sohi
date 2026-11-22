//! `sohi.exe --set-key <name>`: stores an API key in Windows Credential
//! Manager (spec §5.7). The key is read without echo and is never logged.

use std::process::ExitCode;

use sohi_core::secret::KeyName;
use sohi_win::Prompt;

/// Runs the action and returns the process exit code.
pub fn run(name: &str) -> ExitCode {
    // Without a console or redirected input there is nowhere to ask.
    let Ok(prompt) = Prompt::open() else {
        return ExitCode::FAILURE;
    };
    let ok = store(&prompt, name);
    prompt.pause();
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn store(prompt: &Prompt, name: &str) -> bool {
    let Some(key) = KeyName::parse(name) else {
        prompt.say(&usage(name));
        return false;
    };
    prompt.say(&format!(
        "Paste your {} API key and press Enter. It will not be shown.\n> ",
        key.as_str()
    ));
    let secret = match prompt.read_secret() {
        Ok(Some(secret)) => secret,
        Ok(None) => {
            prompt.say("Nothing was entered, so the key was not changed.\n");
            return false;
        }
        Err(err) => {
            prompt.say(&format!("Could not read the key: {err}\n"));
            return false;
        }
    };
    match sohi_win::write_key(key.target(), &secret) {
        Ok(()) => {
            prompt.say(&format!(
                "Saved in Windows Credential Manager as {}. Sohi uses it from the next request.\n",
                key.target()
            ));
            true
        }
        Err(err) => {
            prompt.say(&format!("Could not save the key: {err}\n"));
            false
        }
    }
}

fn usage(name: &str) -> String {
    let names: Vec<_> = KeyName::ALL.iter().map(|k| k.as_str()).collect();
    let given = if name.is_empty() {
        "No key name was given".to_owned()
    } else {
        format!("\"{name}\" is not a key Sohi knows")
    };
    format!(
        "{given}. Use one of: {}.\nExample: sohi.exe --set-key mistral\n",
        names.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_lists_every_key() {
        let text = usage("openai");
        assert!(text.contains("\"openai\""));
        assert!(text.contains("mistral, anthropic, tts"));
        assert!(usage("").starts_with("No key name was given"));
    }
}
