pub use crate::model::{Assigned, Speakers};

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

    pub fn person(&self, media: &str, stream: u32, run: &str, speaker: u32, at: f64) -> String {
        if let Some(a) = self
            .assigned
            .iter()
            .rev()
            .find(|a| a.media == media && a.stream == stream && a.start <= at && at < a.end)
        {
            return a.person.clone();
        }
        self.links
            .get(&voice(media, stream, speaker))
            .cloned()
            .unwrap_or_else(|| heard_person(run, speaker))
    }

    pub fn name(&self, person: &str) -> String {
        match self.names.get(person) {
            Some(n) => n.clone(),
            None => format!("Speaker {}", person.rsplit('/').next().unwrap_or(person)),
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
