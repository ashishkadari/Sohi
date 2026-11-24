//! The index of installed apps, and finding one by what the user calls it.
//!
//! A name is only ever looked up in the index; it is never run itself.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// An index older than this is rebuilt (spec §5.6).
pub const MAX_AGE: Duration = Duration::from_mins(10);
/// Start Menu folders are not deeper than this.
const MAX_DEPTH: usize = 6;

/// An installed app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// What it is called.
    pub name: String,
    /// What to open.
    pub target: String,
    /// Only found by its exact name. `App Paths` is full of helper
    /// programs nobody means.
    pub exact_only: bool,
}

/// The installed apps, as last read.
#[derive(Debug, Default)]
pub struct AppIndex {
    apps: Vec<App>,
    built: Option<Instant>,
}

impl AppIndex {
    /// Whether it must be (re)built before use.
    #[must_use]
    pub fn is_stale(&self, now: Instant) -> bool {
        self.built
            .is_none_or(|built| now.saturating_duration_since(built) > MAX_AGE)
    }

    /// Replaces the index. Uninstallers are left out: a misheard name must
    /// never start one. Earlier apps win over later ones of that name.
    pub fn rebuild(&mut self, apps: Vec<App>, now: Instant) {
        self.apps.clear();
        for app in apps {
            let words = words(&app.name);
            let unwanted = words.is_empty() || words.iter().any(|w| w.starts_with("uninstall"));
            if !unwanted
                && !self
                    .apps
                    .iter()
                    .any(|a| a.name.eq_ignore_ascii_case(&app.name))
            {
                self.apps.push(app);
            }
        }
        self.built = Some(now);
    }

    /// How many apps are known.
    #[must_use]
    pub fn len(&self) -> usize {
        self.apps.len()
    }

    /// Whether no apps are known.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.apps.is_empty()
    }

    /// The app the user means by `query`, if one fits well enough.
    #[must_use]
    pub fn find(&self, query: &str) -> Option<&App> {
        let query = Query::new(query)?;
        self.apps
            .iter()
            .filter_map(|app| {
                let score = query.score(&app.name)?;
                (score == EXACT || !app.exact_only).then_some((score, app))
            })
            // The best score; of those, the shortest name.
            .max_by_key(|(score, app)| (*score, std::cmp::Reverse(app.name.len())))
            .map(|(_, app)| app)
    }
}

/// The score of a name that is the query.
const EXACT: u32 = 100;

/// What the user asked for, in the forms names are compared in.
struct Query {
    words: Vec<String>,
    joined: String,
}

impl Query {
    fn new(text: &str) -> Option<Self> {
        let mut words = words(text);
        // "the calculator app" means "calculator".
        if words.len() > 1 && words.first().is_some_and(|w| w == "the" || w == "my") {
            words.remove(0);
        }
        if words.len() > 1 && words.last().is_some_and(|w| w == "app") {
            words.pop();
        }
        let joined = words.concat();
        (!joined.is_empty()).then_some(Self { words, joined })
    }

    /// How well `name` fits; `None` if it does not.
    fn score(&self, name: &str) -> Option<u32> {
        let name = words(name);
        let joined = name.concat();
        if joined == self.joined {
            return Some(EXACT);
        }
        // "chrome" for "Google Chrome", "word" for "Microsoft Word".
        let plain = match name.split_first() {
            Some((maker, rest)) if !rest.is_empty() && MAKERS.contains(&maker.as_str()) => rest,
            _ => &name[..],
        };
        if plain.concat() == self.joined {
            return Some(95);
        }
        if abbreviations(&name).any(|a| a == self.joined) {
            return Some(90);
        }
        if starts_words(plain, &self.words) {
            return Some(80);
        }
        if name
            .windows(self.words.len())
            .any(|w| starts_words(w, &self.words))
        {
            return Some(70);
        }
        // A typo, or a name misheard by a letter or two.
        let allowed = match self.joined.chars().count() {
            0..=3 => return None,
            4..=6 => 1,
            _ => 2,
        };
        let off = distance(&plain.concat(), &self.joined);
        (off <= allowed).then(|| 60 - u32::try_from(off).unwrap_or(60))
    }
}

/// Names that go before what people call an app.
const MAKERS: [&str; 5] = ["microsoft", "google", "windows", "mozilla", "adobe"];

/// Lower-case words of letters and digits.
fn words(text: &str) -> Vec<String> {
    let mut text = text.to_lowercase();
    if let Some(stem) = text.strip_suffix(".exe") {
        text.truncate(stem.len());
    }
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// "vscode" and "vsc" for "Visual Studio Code": the first words by their
/// initials, the others whole.
fn abbreviations(name: &[String]) -> impl Iterator<Item = String> + '_ {
    (2..=name.len()).map(move |n| {
        let (initials, rest) = name.split_at(n);
        initials
            .iter()
            .filter_map(|w| w.chars().next())
            .chain(rest.iter().flat_map(|w| w.chars()))
            .collect()
    })
}

/// Whether `name` begins with `query`, whose last word may be cut short
/// after three letters.
fn starts_words(name: &[String], query: &[String]) -> bool {
    let Some((last, whole)) = query.split_last() else {
        return false;
    };
    name.len() >= query.len()
        && name.starts_with(whole)
        && name.get(whole.len()).is_some_and(|w| {
            w == last || (last.chars().count() >= 3 && w.starts_with(last.as_str()))
        })
}

/// The edit distance (insertions, deletions and substitutions).
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = i;
        let mut left = i + 1;
        for (cell, cb) in row.iter_mut().skip(1).zip(&b) {
            let above = *cell;
            let best = (diagonal + usize::from(ca != *cb))
                .min(above + 1)
                .min(left + 1);
            diagonal = above;
            left = best;
            *cell = best;
        }
        if let Some(first) = row.first_mut() {
            *first = i + 1;
        }
    }
    row.last().copied().unwrap_or(0)
}

/// The `.lnk` files below the Start Menu folders of this user and of
/// everyone.
#[must_use]
pub fn start_menu() -> Vec<App> {
    let folders = ["APPDATA", "ProgramData"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|base| PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs"));
    let mut apps = Vec::new();
    for folder in folders {
        collect_links(&folder, MAX_DEPTH, &mut apps);
    }
    apps
}

fn collect_links(folder: &Path, depth: usize, apps: &mut Vec<App>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            // Symbolic links and junctions are not followed.
            Ok(kind) if kind.is_dir() && depth > 0 => collect_links(&path, depth - 1, apps),
            Ok(kind) if kind.is_file() => apps.extend(link(&path)),
            _ => {}
        }
    }
}

fn link(path: &Path) -> Option<App> {
    if !path.extension()?.eq_ignore_ascii_case("lnk") {
        return None;
    }
    Some(App {
        name: path.file_stem()?.to_str()?.to_owned(),
        target: path.to_str()?.to_owned(),
        exact_only: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(names: &[&str]) -> AppIndex {
        let mut index = AppIndex::default();
        let apps = names.iter().map(|name| App {
            name: (*name).to_owned(),
            target: format!("{name}.lnk"),
            exact_only: false,
        });
        index.rebuild(apps.collect(), Instant::now());
        index
    }

    fn find<'a>(index: &'a AppIndex, query: &str) -> &'a str {
        index.find(query).map_or("", |app| app.name.as_str())
    }

    #[test]
    fn finds_what_people_call_things() {
        let index = index(&[
            "Notepad",
            "Notepad++",
            "Google Chrome",
            "Chrome Remote Desktop",
            "Microsoft Edge",
            "Word",
            "WordPad",
            "PowerPoint",
            "Visual Studio Code",
            "Visual Studio 2022",
            "Task Manager",
            "Spotify",
            "Steam",
            "Steam Support Center",
            "Settings",
            "Uninstall Spotify",
        ]);
        for (query, want) in [
            ("notepad", "Notepad"),
            ("Notepad.exe", "Notepad"),
            ("chrome", "Google Chrome"),
            ("edge", "Microsoft Edge"),
            ("word", "Word"),
            ("power point", "PowerPoint"),
            ("vs code", "Visual Studio Code"),
            ("VSCode", "Visual Studio Code"),
            ("code", "Visual Studio Code"),
            ("visual studio", "Visual Studio 2022"),
            ("task manager", "Task Manager"),
            ("the settings app", "Settings"),
            ("Spotifi", "Spotify"),
            ("spotify", "Spotify"),
            ("steam", "Steam"),
            ("remote desktop", "Chrome Remote Desktop"),
            ("uninstall spotify", ""),
            ("photoshop", ""),
            ("a", ""),
            ("", ""),
            ("calc.exe /c format", ""),
            (r"C:\Windows\System32\cmd.exe", ""),
        ] {
            assert_eq!(find(&index, query), want, "{query}");
        }
    }

    #[test]
    fn helper_programs_need_their_exact_name() {
        let mut index = AppIndex::default();
        let app = App {
            name: "setup.exe".to_owned(),
            target: "setup.exe".to_owned(),
            exact_only: true,
        };
        index.rebuild(vec![app], Instant::now());
        assert_eq!(find(&index, "set"), "");
        assert_eq!(find(&index, "setub"), "");
        assert_eq!(find(&index, "setup"), "setup.exe");
    }

    #[test]
    fn the_first_app_of_a_name_wins() {
        let mut index = AppIndex::default();
        let app = |target: &str| App {
            name: "Paint".to_owned(),
            target: target.to_owned(),
            exact_only: false,
        };
        index.rebuild(vec![app("first"), app("second")], Instant::now());
        assert_eq!(index.len(), 1);
        assert_eq!(index.find("paint").unwrap().target, "first");
    }

    #[test]
    fn goes_stale_after_ten_minutes() {
        let mut index = AppIndex::default();
        let now = Instant::now();
        assert!(index.is_stale(now));
        index.rebuild(Vec::new(), now);
        assert!(!index.is_stale(now + MAX_AGE));
        assert!(index.is_stale(now + MAX_AGE + Duration::from_millis(1)));
    }

    #[test]
    fn edit_distances() {
        assert_eq!(distance("", ""), 0);
        assert_eq!(distance("abc", ""), 3);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("spotify", "spotifi"), 1);
    }

    #[test]
    #[ignore = "prints what this PC's index finds"]
    fn real_index() {
        use crate::Desktop;
        let started = Instant::now();
        let mut index = AppIndex::default();
        index.rebuild(crate::WinDesktop.apps(), started);
        println!(
            "{} apps in {} ms",
            index.len(),
            started.elapsed().as_millis()
        );
        for query in std::env::var("APPS").unwrap_or_default().split(',') {
            println!(
                "{query}: {:?}",
                index.find(query).map(|a| (&a.name, &a.target))
            );
        }
    }
}
