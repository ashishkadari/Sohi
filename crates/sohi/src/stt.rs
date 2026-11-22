//! Speech-to-text checks against the live service (spec §5.5, §10.6 P4).
//! The voice turn's glue lands here with the wiring.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sohi_core::config::SttConfig;
    use sohi_core::secret::KeyName;
    use sohi_providers::voxtral::TranscribeRequest;
    use sohi_providers::{HttpClient, Mistral, Voxtral, mistral};

    use crate::chat::read_key;

    /// The fixture phrases and what was said.
    const PHRASES: [(&str, &str); 5] = [
        ("open_browser", "Open the browser."),
        ("weather", "What's the weather like in Paris this weekend?"),
        (
            "timer",
            "Set a timer for ten minutes, then remind me to call my mother.",
        ),
        ("music", "Play some quiet music while I work."),
        ("question", "How far is the moon from the earth?"),
    ];

    /// Lowercase words without punctuation; small numbers as words.
    fn words(text: &str) -> Vec<String> {
        const NUMBERS: [&str; 11] = [
            "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ];
        text.split_whitespace()
            .map(|w| {
                let w: String = w
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '\'')
                    .flat_map(char::to_lowercase)
                    .map(|c| if c == '\u{2019}' { '\'' } else { c })
                    .collect();
                match w.parse::<usize>() {
                    Ok(n) if n < NUMBERS.len() => NUMBERS[n].to_owned(),
                    _ => w,
                }
            })
            .filter(|w| !w.is_empty())
            .collect()
    }

    /// Word-level edit distance.
    fn word_errors(reference: &[String], heard: &[String]) -> usize {
        let mut row: Vec<usize> = (0..=heard.len()).collect();
        for (i, r) in reference.iter().enumerate() {
            let mut diagonal = row[0];
            row[0] = i + 1;
            for (j, h) in heard.iter().enumerate() {
                let substitute = diagonal + usize::from(r != h);
                diagonal = row[j + 1];
                row[j + 1] = substitute.min(row[j] + 1).min(row[j + 1] + 1);
            }
        }
        row[heard.len()]
    }

    #[test]
    fn counts_word_errors() {
        let a = words("Set a timer for ten minutes.");
        let b = words("set the timer for 10 minutes");
        assert_eq!(a, ["set", "a", "timer", "for", "ten", "minutes"]);
        assert_eq!(word_errors(&a, &b), 1);
        assert_eq!(word_errors(&a, &[]), 6);
        assert_eq!(word_errors(&[], &b), 6);
    }

    /// Lists the Voxtral models the stored key can use, then transcribes
    /// the five fixtures with the configured model (or `SOHI_STT_MODEL`)
    /// and checks the word error rate is at most 15 %.
    #[tokio::test]
    #[ignore = "needs the internet and the stored Mistral key"]
    async fn voxtral_transcribes_the_fixtures() {
        let key = read_key(KeyName::Mistral).unwrap();
        let http = HttpClient::new().unwrap();
        let ids = Mistral::new(http.clone(), mistral::BASE_URL)
            .models(&key, Duration::from_secs(10))
            .await
            .unwrap();
        let voxtral_ids: Vec<_> = ids.iter().filter(|id| id.contains("voxtral")).collect();
        println!("voxtral models: {voxtral_ids:?}");

        let model = std::env::var("SOHI_STT_MODEL").unwrap_or_else(|_| SttConfig::default().model);
        let client = Voxtral::public(http);
        let (mut errors, mut total) = (0, 0);
        for (name, said) in PHRASES {
            let path = format!(
                "{}/../sohi-audio/tests/fixtures/{name}.wav",
                env!("CARGO_MANIFEST_DIR")
            );
            let wav = std::fs::read(&path).unwrap();
            let request = TranscribeRequest {
                model: &model,
                language: "auto",
                wav: &wav,
            };
            let started = std::time::Instant::now();
            let heard = client.transcribe(&key, &request).await.unwrap();
            let reference = words(said);
            let wrong = word_errors(&reference, &words(&heard.text));
            println!(
                "{name}: {:?} ({:?}) {} ms, {wrong}/{} wrong",
                heard.text,
                heard.language,
                started.elapsed().as_millis(),
                reference.len()
            );
            errors += wrong;
            total += reference.len();
        }
        println!("{model}: {errors}/{total} words wrong");
        assert!(errors * 100 <= total * 15, "WER {errors}/{total} over 15 %");
    }
}
