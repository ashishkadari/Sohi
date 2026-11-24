//! The one frame timer, shared by the head and full mode: it ticks as
//! often as whoever needs it most.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use sohi_win::FrameTimer;

/// Who wants frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// The head.
    Head,
    /// Full mode.
    Full,
}

/// The shared timer.
pub struct Ticker {
    timer: Rc<FrameTimer>,
    wants: Cell<[Option<Duration>; 2]>,
}

impl Ticker {
    /// A stopped ticker.
    pub fn new(timer: Rc<FrameTimer>) -> Rc<Self> {
        Rc::new(Self {
            timer,
            wants: Cell::default(),
        })
    }

    /// Asks for a frame every `period`, or for none.
    pub fn set(&self, who: Who, period: Option<Duration>) {
        let before = self.wants.get();
        let mut wants = before;
        wants[who as usize] = period;
        self.wants.set(wants);
        let now = shortest(wants);
        if now == shortest(before) {
            return;
        }
        match now {
            Some(period) => self.timer.start(period),
            None => self.timer.stop(),
        }
    }
}

fn shortest(wants: [Option<Duration>; 2]) -> Option<Duration> {
    wants.into_iter().flatten().min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_ticks_for_whoever_needs_it() {
        let timer = Rc::new(FrameTimer::new().unwrap());
        let ticker = Ticker::new(Rc::clone(&timer));
        let ms = Duration::from_millis;
        ticker.set(Who::Head, Some(ms(33)));
        ticker.set(Who::Full, Some(ms(16)));
        assert_eq!(shortest(ticker.wants.get()), Some(ms(16)));
        ticker.set(Who::Full, None);
        assert_eq!(shortest(ticker.wants.get()), Some(ms(33)));
        assert!(timer.is_running());
        ticker.set(Who::Head, None);
        assert!(!timer.is_running());
    }
}
