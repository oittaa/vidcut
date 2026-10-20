pub use crate::model::{Assigned, Speakers};
use crate::transcript::Word;

pub fn voice(media: &str, stream: u32, speaker: u32) -> String {
    format!("{media}/{stream}/{speaker}")
}

pub fn heard_person(run: &str, speaker: u32) -> String {
    format!("{run}/{speaker}")
}

impl Speakers {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.links.is_empty() && self.assigned.is_empty()
    }

    pub fn assigned_to(&self, media: &str, stream: u32, at: f64) -> Option<String> {
        self.assigned
            .iter()
            .rev()
            .find(|a| a.media == media && a.stream == stream && a.start <= at && at < a.end)
            .map(|a| a.person.clone())
    }

    pub fn word(&self, media: &str, stream: u32, run: Option<&str>, w: &Word) -> Option<String> {
        let at = (w.start + w.end) / 2.0;
        match (w.speaker, run) {
            (Some(n), Some(run)) => Some(self.person(media, stream, run, n, at)),
            _ => self.assigned_to(media, stream, at),
        }
    }

    pub fn person(&self, media: &str, stream: u32, run: &str, speaker: u32, at: f64) -> String {
        if let Some(a) = self.assigned_to(media, stream, at) {
            return a;
        }
        self.links
            .get(&voice(media, stream, speaker))
            .cloned()
            .unwrap_or_else(|| heard_person(run, speaker))
    }

    pub fn name(&self, person: &str) -> String {
        let number = person
            .rsplit('/')
            .next()
            .filter(|n| n.parse::<u32>().is_ok());
        match (self.names.get(person), number) {
            (Some(n), _) => n.clone(),
            (None, Some(n)) => format!("Speaker {n}"),
            (None, None) => person.to_string(),
        }
    }

    pub fn assign(&mut self, a: Assigned) {
        self.assigned.retain(|o| {
            o.media != a.media || o.stream != a.stream || o.end <= a.start || o.start >= a.end
        });
        if !a.person.is_empty() {
            self.assigned.push(a);
        }
    }
}
