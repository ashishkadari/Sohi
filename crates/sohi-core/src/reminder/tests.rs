use super::*;

/// A Tuesday.
const NOW: &str = "2026-11-24 18:00:00";

fn at(text: &str) -> Stamp {
    text.parse().unwrap()
}

fn clock(text: &str) -> Clock {
    text.parse().unwrap()
}

fn add(list: &mut Reminders, text: &str, when: When, repeat: Repeat) -> Reminder {
    list.add(at(NOW), text, when, repeat).unwrap()
}

#[test]
fn the_moment_is_worked_out() {
    let day = |text: &str| text.parse().unwrap();
    for (when, repeat, want) in [
        (When::In(90), Repeat::Once, "2026-11-24 18:01:30"),
        (
            When::At(clock("19"), Day::Next),
            Repeat::Once,
            "2026-11-24 19:00:00",
        ),
        (
            When::At(clock("18"), Day::Next),
            Repeat::Once,
            "2026-11-25 18:00:00",
        ),
        (
            When::At(clock("9"), Day::Next),
            Repeat::Daily,
            "2026-11-25 09:00:00",
        ),
        (
            When::At(clock("9"), day("2026-12-01")),
            Repeat::Once,
            "2026-12-01 09:00:00",
        ),
    ] {
        let r = add(&mut Reminders::default(), "x", when, repeat);
        assert_eq!(r.at.to_string(), want);
    }
    for (time, text, want) in [
        ("9", "Tomorrow", "2026-11-25 09:00:00"),
        ("19", "today", "2026-11-24 19:00:00"),
        ("19", "tuesday", "2026-11-24 19:00:00"),
        ("9", "tuesday", "2026-12-01 09:00:00"),
        ("9", "Friday", "2026-11-27 09:00:00"),
        ("9", "", "2026-11-25 09:00:00"),
    ] {
        let when = When::At(clock(time), day(text));
        let r = add(&mut Reminders::default(), "x", when, Repeat::Once);
        assert_eq!(r.at.to_string(), want, "{text}");
    }
    assert!("someday".parse::<Day>().is_err());
    // Friday evening: the next weekday is Monday.
    let friday = at("2026-11-27 18:00");
    let when = When::At(clock("9"), Day::Next);
    let r = Reminders::default().add(friday, "x", when, Repeat::Weekdays);
    assert_eq!(r.unwrap().at, at("2026-11-30 09:00"));
}

#[test]
fn nonsense_is_refused() {
    let mut list = Reminders::default();
    let mut add = |text: &str, when| list.add(at(NOW), text, when, Repeat::Once);
    assert_eq!(add(" \n ", When::In(60)), Err(Refused::NoText));
    assert_eq!(add(&"x".repeat(121), When::In(60)), Err(Refused::TooLong));
    assert_eq!(add("x", When::In(0)), Err(Refused::Past));
    assert_eq!(add("x", When::In(-60)), Err(Refused::Past));
    assert_eq!(add("x", When::In(i64::MAX)), Err(Refused::TooFar));
    let past = When::At(clock("9"), "2026-11-24".parse().unwrap());
    assert_eq!(add("x", past), Err(Refused::Past));
    for _ in 0..MAX_COUNT {
        add("x", When::In(60)).unwrap();
    }
    assert_eq!(add("x", When::In(60)), Err(Refused::TooMany));
}

#[test]
fn the_text_is_one_line() {
    let mut list = Reminders::default();
    let r = add(
        &mut list,
        " call\r\n\tmum \u{7} ",
        When::In(9),
        Repeat::Once,
    );
    assert_eq!(r.text, "call mum");
}

#[test]
fn the_next_one_comes_first() {
    let mut list = Reminders::default();
    add(&mut list, "later", When::In(600), Repeat::Once);
    add(&mut list, "soon", When::In(60), Repeat::Once);
    assert_eq!(list.next_at(), Some(at("2026-11-24 18:01")));
    let ids: Vec<_> = list.all().iter().map(|r| (r.id, r.text.as_str())).collect();
    assert_eq!(ids, [(2, "soon"), (1, "later")]);
    assert_eq!(list.cancel(2).unwrap().text, "soon");
    assert_eq!(list.cancel(2), None);
    assert_eq!(list.next_at(), Some(at("2026-11-24 18:10")));
}

#[test]
fn each_is_due_once() {
    let mut list = Reminders::default();
    add(&mut list, "a", When::In(60), Repeat::Once);
    add(&mut list, "b", When::In(61), Repeat::Once);
    assert_eq!(list.take_due(at("2026-11-24 18:00:59")), []);
    assert_eq!(list.take_due(at("2026-11-24 18:01:00")).len(), 1);
    assert_eq!(list.take_due(at("2026-11-24 18:01:00")), []);
    assert_eq!(list.all().len(), 1);
}

#[test]
fn what_repeats_moves_on() {
    for (repeat, now, want) in [
        (Repeat::Daily, "2026-11-25 09:00:00", "2026-11-26 09:00:00"),
        // The PC was off for days.
        (Repeat::Daily, "2026-12-03 08:00:00", "2026-12-03 09:00:00"),
        (Repeat::Daily, "2026-12-03 10:00:00", "2026-12-04 09:00:00"),
        // A Friday and a Saturday.
        (
            Repeat::Weekdays,
            "2026-11-27 09:00:00",
            "2026-11-30 09:00:00",
        ),
        (
            Repeat::Weekdays,
            "2026-11-28 12:00:00",
            "2026-11-30 09:00:00",
        ),
        (Repeat::Weekly, "2026-11-25 09:00:05", "2026-12-02 09:00:00"),
        (Repeat::Weekly, "2026-12-20 09:00:00", "2026-12-23 09:00:00"),
    ] {
        let mut list = Reminders::default();
        add(&mut list, "x", When::At(clock("9"), Day::Next), repeat);
        assert_eq!(list.take_due(at(now)).len(), 1);
        assert_eq!(list.next_at(), Some(at(want)), "{repeat:?} {now}");
        assert_eq!(list.take_due(at(now)), []);
    }
}

#[test]
fn they_are_announced() {
    let mut list = Reminders::default();
    add(&mut list, "call mum", When::In(60), Repeat::Once);
    add(&mut list, "Did you eat?", When::In(70), Repeat::Once);
    let now = at("2026-11-24 18:01:15");
    let due = list.take_due(now);
    assert_eq!(announce(&due[..1], now), "Reminder: call mum.");
    assert_eq!(announce(&due, now), "Reminders: call mum. Did you eat?");
    let late = at("2026-11-25 08:00");
    assert_eq!(
        announce(&due, late),
        "Reminders: call mum (yesterday at 18:01). Did you eat (yesterday at 18:01)?"
    );
    assert_eq!(due[0].describe(at(NOW)), "1: call mum, today at 18:01");
}

#[test]
fn a_file_is_tidied() {
    let one = |id, text: &str| Reminder {
        id,
        at: at(NOW),
        repeat: Repeat::Once,
        text: text.to_owned(),
    };
    let long = "y".repeat(500);
    let list = Reminders(vec![one(7, "a\nb"), one(7, " "), one(0, &long)]).checked();
    let got: Vec<_> = list.all().iter().map(|r| (r.id, r.text.len())).collect();
    assert_eq!(got, [(1, 3), (3, MAX_TEXT)]);
    let many = Reminders(vec![one(1, "x"); 500]);
    assert_eq!(many.checked().all().len(), MAX_COUNT);
}
