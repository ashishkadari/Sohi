//! What may run, and what the user is asked about first (spec §5.6).
//!
//! The executor only takes a [`Permit`], and only this module makes one:
//! whatever a model says, nothing runs that was not allowed here.

use sohi_core::chat::ToolSpec;
use sohi_core::config::ToolsConfig;

use crate::{Call, Outcome, Tool};

/// The user's tool settings.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    confirm: Vec<Tool>,
    disabled: Vec<Tool>,
}

impl Policy {
    /// The policy of `[tools]`. Names of tools that do not exist (or not
    /// in this build) are ignored.
    #[must_use]
    pub fn new(config: &ToolsConfig) -> Self {
        let tools = |names: &[String]| names.iter().filter_map(|n| Tool::named(n.trim())).collect();
        Self {
            confirm: tools(&config.confirm),
            disabled: tools(&config.disabled),
        }
    }

    /// The tools models are told about.
    #[must_use]
    pub fn specs(&self) -> Vec<ToolSpec> {
        Tool::ALL
            .iter()
            .filter(|tool| !self.disabled.contains(tool))
            .map(|tool| tool.spec())
            .collect()
    }

    /// Starts judging one turn's calls.
    #[must_use]
    pub fn gate(&self) -> Gate<'_> {
        Gate {
            policy: self,
            tainted: false,
        }
    }
}

/// Judges the calls of one turn.
#[derive(Debug)]
pub struct Gate<'a> {
    policy: &'a Policy,
    /// Whether the turn has seen text from outside. From then on the model
    /// may be following instructions hidden in it, so every call is asked
    /// about, whatever the settings say.
    tainted: bool,
}

/// Permission to run one call.
#[derive(Debug)]
pub struct Permit(Call);

impl Permit {
    /// What may run.
    #[must_use]
    pub fn call(&self) -> &Call {
        &self.0
    }
}

/// A call the user must be asked about.
#[derive(Debug)]
pub struct Question(Call);

impl Question {
    /// What would run, in a few words.
    #[must_use]
    pub fn summary(&self) -> String {
        self.0.summary()
    }

    /// Takes the user's answer; no answer is a no.
    ///
    /// # Errors
    /// Returns the denied outcome if the answer is no.
    pub fn answer(self, yes: bool) -> Result<Permit, Outcome> {
        if yes {
            Ok(Permit(self.0))
        } else {
            Err(Outcome::denied())
        }
    }
}

/// What is to happen with a call.
#[derive(Debug)]
pub enum Verdict {
    /// It may run.
    Allow(Permit),
    /// The user must be asked.
    Ask(Question),
    /// It may not run.
    Refuse(Outcome),
}

impl Gate<'_> {
    /// Judges `call`.
    #[must_use]
    pub fn judge(&self, call: Call) -> Verdict {
        let tool = call.tool();
        if self.policy.disabled.contains(&tool) {
            return Verdict::Refuse(Outcome::failed("this tool is switched off"));
        }
        if self.tainted || always_asks(tool) || self.policy.confirm.contains(&tool) {
            return Verdict::Ask(Question(call));
        }
        Verdict::Allow(Permit(call))
    }

    /// Whether the turn has seen text from outside.
    #[must_use]
    pub fn is_tainted(&self) -> bool {
        self.tainted
    }

    /// Takes note of what a call returned.
    pub fn saw(&mut self, outcome: &Outcome) {
        self.tainted |= outcome.untrusted.is_some();
    }
}

/// Tools no setting can run unasked.
#[cfg(feature = "shell")]
fn always_asks(tool: Tool) -> bool {
    tool == Tool::ShellExec
}

#[cfg(not(feature = "shell"))]
fn always_asks(_tool: Tool) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(confirm: &[&str], disabled: &[&str]) -> ToolsConfig {
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect();
        ToolsConfig {
            confirm: names(confirm),
            disabled: names(disabled),
        }
    }

    fn open_url() -> Call {
        Call::OpenUrl {
            url: "https://a.com".to_owned(),
        }
    }

    #[test]
    fn by_default_only_reading_the_clipboard_asks() {
        let policy = Policy::new(&ToolsConfig::default());
        let gate = policy.gate();
        assert!(matches!(gate.judge(Call::ClipboardGet), Verdict::Ask(_)));
        assert!(matches!(gate.judge(open_url()), Verdict::Allow(_)));
        assert_eq!(policy.specs().len(), Tool::ALL.len());
    }

    #[test]
    fn settings_add_questions_and_remove_tools() {
        let policy = Policy::new(&config(&["open_url", "nonsense"], &["clipboard_get "]));
        let gate = policy.gate();
        assert!(matches!(gate.judge(open_url()), Verdict::Ask(_)));
        assert!(matches!(gate.judge(Call::ClipboardGet), Verdict::Refuse(_)));
        assert!(policy.specs().iter().all(|s| s.name != "clipboard_get"));
    }

    #[test]
    fn after_text_from_outside_everything_asks() {
        let policy = Policy::new(&config(&[], &[]));
        let mut gate = policy.gate();
        gate.saw(&Outcome::done("Volume 30%."));
        assert!(matches!(gate.judge(open_url()), Verdict::Allow(_)));
        gate.saw(&Outcome {
            untrusted: Some(String::new()),
            ..Outcome::done("Read.")
        });
        assert!(matches!(gate.judge(open_url()), Verdict::Ask(_)));
        gate.saw(&Outcome::done("Fine."));
        assert!(matches!(gate.judge(open_url()), Verdict::Ask(_)));
        // The next turn starts afresh.
        assert!(matches!(policy.gate().judge(open_url()), Verdict::Allow(_)));
    }

    #[test]
    #[cfg(feature = "shell")]
    fn commands_always_ask() {
        let policy = Policy::new(&config(&[], &[]));
        let command = "dir".to_owned();
        let Verdict::Ask(question) = policy.gate().judge(Call::ShellExec { command }) else {
            panic!("not asked");
        };
        assert_eq!(question.summary(), "command dir");
        let policy = Policy::new(&config(&[], &["shell_exec"]));
        let command = "dir".to_owned();
        let verdict = policy.gate().judge(Call::ShellExec { command });
        assert!(matches!(verdict, Verdict::Refuse(_)));
    }

    #[test]
    fn a_no_is_a_denied_outcome() {
        let policy = Policy::new(&ToolsConfig::default());
        let ask = || match policy.gate().judge(Call::ClipboardGet) {
            Verdict::Ask(question) => question,
            other => panic!("{other:?}"),
        };
        assert_eq!(ask().summary(), "read your clipboard");
        assert_eq!(ask().answer(true).unwrap().call(), &Call::ClipboardGet);
        assert_eq!(ask().answer(false).unwrap_err(), Outcome::denied());
    }
}
