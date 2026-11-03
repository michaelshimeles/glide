//! Captions: group transcript words into short on-screen lines.

use crate::project::recording::Word;

/// Characters per caption line before breaking.
pub const MAX_CHARS: usize = 42;
/// A pause this long starts a new line.
const PAUSE_MS: f64 = 700.0;
/// Lines closer than this stay up through the gap.
const GAP_FILL_MS: f64 = 200.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub words: Vec<Word>,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ")
    }
}

pub fn lines(words: &[Word]) -> Vec<Line> {
    let mut out: Vec<Line> = vec![];
    for w in words.iter().filter(|w| !w.text.trim().is_empty()) {
        let start_new = match out.last() {
            None => true,
            Some(l) => {
                let len = l.text().chars().count() + 1 + w.text.trim().chars().count();
                len > MAX_CHARS || w.start - l.end > PAUSE_MS
            }
        };
        if start_new {
            out.push(Line { start: w.start, end: w.end, words: vec![w.clone()] });
        } else if let Some(l) = out.last_mut() {
            l.end = w.end;
            l.words.push(w.clone());
        }
    }
    for i in 1..out.len() {
        if out[i].start - out[i - 1].end <= GAP_FILL_MS {
            out[i - 1].end = out[i].start;
        }
    }
    out
}

/// Replace the words of one caption line with edited `text`, spreading the
/// new words over the line's original time span in proportion to their
/// length. An empty text removes the line.
pub fn edit_line(words: &mut Vec<Word>, line: &Line, text: &str) {
    let (Some(first), Some(last)) = (line.words.first(), line.words.last()) else { return };
    let Some(i0) = words.iter().position(|w| w.start == first.start && w.text == first.text) else { return };
    let Some(i1) = words.iter().rposition(|w| w.start == last.start && w.text == last.text) else { return };
    if i1 < i0 {
        return;
    }
    let (a, b) = (first.start, last.end.max(first.start));
    let new: Vec<&str> = text.split_whitespace().collect();
    let total: usize = new.iter().map(|w| w.chars().count() + 1).sum();
    let mut t = a;
    let replacement: Vec<Word> = new
        .iter()
        .map(|w| {
            let d = (b - a) * (w.chars().count() + 1) as f64 / total.max(1) as f64;
            let word = Word { start: t, end: t + d, text: (*w).to_string() };
            t += d;
            word
        })
        .collect();
    words.splice(i0..=i1, replacement);
}

pub fn line_at(lines: &[Line], t: f64) -> Option<&Line> {
    lines.iter().find(|l| t >= l.start && t <= l.end)
}

/// The words of `line` to show at time `t` when captions carry `per` words at
/// a time (0 = the whole line), and the index of the spoken word among them.
/// The first group starts with the line and the last ends with it, so the
/// caption stays up through the pauses between words.
pub fn focus(line: &Line, t: f64, per: usize) -> (Line, usize) {
    let spoken = line.words.iter().rposition(|w| w.start <= t).unwrap_or(0);
    if per == 0 || per >= line.words.len() {
        return (line.clone(), spoken);
    }
    let from = spoken / per * per;
    let to = (from + per).min(line.words.len());
    let words = line.words[from..to].to_vec();
    let start = if from == 0 { line.start } else { words[0].start };
    let end = if to == line.words.len() { line.end } else { words[words.len() - 1].end };
    (Line { start, end, words }, spoken - from)
}

/// How long a caption takes to pop in.
const POP_MS: f64 = 160.0;

/// Size of a caption `since` ms after it appeared: it grows from 75% and
/// overshoots a little before settling at 100%.
pub fn pop_scale(since: f64) -> f64 {
    const C1: f64 = 1.70158;
    let x = (since / POP_MS).clamp(0.0, 1.0) - 1.0;
    let eased = 1.0 + (C1 + 1.0) * x.powi(3) + C1 * x.powi(2);
    0.75 + 0.25 * eased
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    fn w(start: f64, end: f64, t: &str) -> Word {
        Word { start, end, text: t.into() }
    }

    #[test]
    fn edit_line_replaces_words_within_the_span() {
        let mut words = vec![w(0.0, 300.0, "Helo"), w(300.0, 600.0, "wrld."), w(5000.0, 5400.0, "Next")];
        let ls = lines(&words);
        assert_eq!(ls.len(), 2);
        edit_line(&mut words, &ls[0], "Hello there, world.");
        let texts: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, vec!["Hello", "there,", "world.", "Next"]);
        assert_eq!(words[0].start, 0.0);
        assert!((words[2].end - 600.0).abs() < 1e-9);
        assert!(words[1].start > words[0].start && words[2].start > words[1].start);
        let ls = lines(&words);
        edit_line(&mut words, &ls[1], "  ");
        assert_eq!(words.len(), 3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: f64, end: f64, t: &str) -> Word {
        Word { start, end, text: t.into() }
    }

    #[test]
    fn breaks_on_pause_and_length() {
        let words = [w(0.0, 200.0, "Hello"), w(250.0, 500.0, "world"), w(2000.0, 2300.0, "again")];
        let ls = lines(&words);
        assert_eq!(ls.len(), 2);
        assert_eq!(ls[0].text(), "Hello world");
        assert!(line_at(&ls, 300.0).is_some());
        assert!(line_at(&ls, 1500.0).is_none());
    }

    #[test]
    fn focus_shows_a_few_words_and_marks_the_spoken_one() {
        let words: Vec<_> = (0..5).map(|i| w(i as f64 * 300.0, i as f64 * 300.0 + 250.0, &format!("w{i}"))).collect();
        let line = &lines(&words)[0];
        // Words w0 w1 w2, then w3 w4.
        let (shown, active) = focus(line, 650.0, 3);
        assert_eq!((shown.text().as_str(), active), ("w0 w1 w2", 2));
        assert_eq!(shown.start, line.start);
        // In the gap after w2 the group stays up.
        assert_eq!(focus(line, 880.0, 3).0.text(), "w0 w1 w2");
        let (shown, active) = focus(line, 1300.0, 3);
        assert_eq!((shown.text().as_str(), active), ("w3 w4", 1));
        assert_eq!((shown.start, shown.end), (900.0, line.end));
        // 0 words per line, or more than the line has, keeps the whole line.
        assert_eq!(focus(line, 650.0, 0).0.text(), "w0 w1 w2 w3 w4");
        assert_eq!(focus(line, 650.0, 9).1, 2);
    }

    #[test]
    fn pop_grows_from_three_quarters_and_settles() {
        assert!((pop_scale(0.0) - 0.75).abs() < 1e-9);
        assert!((pop_scale(POP_MS) - 1.0).abs() < 1e-9);
        assert!((pop_scale(5000.0) - 1.0).abs() < 1e-9);
        assert!(pop_scale(POP_MS * 0.6) > 1.0, "overshoots on the way");
    }
}
