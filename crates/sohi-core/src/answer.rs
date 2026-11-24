//! Understanding a spoken or typed yes or no (spec §5.6): a fixed list,
//! never a model.

const YES: &str = "yes|yeah|yep|yup|ya|sure|ok|okay|go ahead|do it|allow|allow it|confirm|\
    affirmative|of course|please do|approve|absolutely";
const NO: &str = "no|nope|nah|cancel|stop|dont|do not|dont do it|do not do it|deny|negative|\
    never|never mind|nevermind|no way|absolutely not";
/// Words that may surround an answer without changing it.
const FILLER: [&str; 5] = ["please", "sohi", "thanks", "thank you", "um"];

/// Whether `text` is a yes or a no; `None` if it is neither for sure. The
/// whole utterance must be an answer: "yes, but first…" is not one.
#[must_use]
pub fn parse_answer(text: &str) -> Option<bool> {
    let plain: String = text
        .chars()
        .filter(|c| !matches!(c, '\'' | '’'))
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .flat_map(char::to_lowercase)
        .collect();
    let mut plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        if let Some(said) = answer(&plain).or_else(|| agreeing(&plain)) {
            return Some(said);
        }
        let shorter = FILLER.iter().find_map(|word| {
            let without = |part: Option<&str>| part.map(str::to_owned);
            without(plain.strip_prefix(word).and_then(|r| r.strip_prefix(' ')))
                .or_else(|| without(plain.strip_suffix(word).and_then(|r| r.strip_suffix(' '))))
        });
        plain = shorter?;
    }
}

fn answer(text: &str) -> Option<bool> {
    let is_one_of = |list: &str| list.split('|').any(|word| word == text);
    if is_one_of(YES) {
        Some(true)
    } else if is_one_of(NO) {
        Some(false)
    } else {
        None
    }
}

/// Whether `text` is two answers in a row that say the same: "yes yes",
/// "no, stop".
fn agreeing(text: &str) -> Option<bool> {
    text.match_indices(' ').find_map(|(i, _)| {
        let (first, second) = text.split_at(i);
        let first = answer(first)?;
        (answer(second.trim_start())? == first).then_some(first)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers() {
        for text in [
            "Yes.",
            "yeah",
            " YEP! ",
            "Yes, please.",
            "Sure, go ahead.",
            "yes yes",
            "Okay, Sohi.",
            "do it",
            "Um, yes.",
        ] {
            assert_eq!(parse_answer(text), Some(true), "{text}");
        }
        for text in [
            "No.",
            "nope",
            "No, thanks.",
            "Don't!",
            "don’t do it",
            "No, stop.",
            "cancel",
            "no no",
            "Absolutely not.",
            "never mind",
            "No thank you, Sohi.",
        ] {
            assert_eq!(parse_answer(text), Some(false), "{text}");
        }
    }

    #[test]
    fn anything_else_is_not_an_answer() {
        for text in [
            "",
            "...",
            "yes no",
            "no, yes",
            "yes but not now",
            "I don't know",
            "what?",
            "yesterday",
            "not yes",
            "know",
            "do it not",
            "sure not",
            "okay then no",
            "please",
        ] {
            assert_eq!(parse_answer(text), None, "{text}");
        }
    }
}
